//! The `pandoc lua` worker: a warm pandoc that never pays process startup.
//!
//! Measured on an idle development machine, with pandoc 3.6.3:
//!
//! | path | one block | whole 283-line doc |
//! |---|---:|---:|
//! | one-shot `pandoc.exe` | 74 ms (process start) | 74 ms |
//! | `pandoc server` over HTTP | 1.2 ms | 23.5 ms |
//! | **this worker** | **0.52 ms** | **16 ms** |
//!
//! Protocol: one JSON object per line, in and out. JSON escapes newlines, so every
//! message is exactly one line and Windows text-mode translation cannot corrupt the
//! framing — which is why this is not length-prefixed.
//!
//! ## Quarto fidelity
//!
//! This worker runs *plain* pandoc, so its HTML differs from Quarto's for anything
//! Quarto's filters implement: callouts, crossrefs, shortcodes, bibliography. Driving
//! the real chain from here is possible — probed and confirmed — and needs all of:
//!
//! * `--data-dir <quarto>/share/pandoc/datadir`, whose `init.lua` defines `param` and
//!   the `quarto.*` globals;
//! * `--from <quarto>/share/filters/qmd-reader.lua`, because the filters read metadata
//!   (`quarto_pandoc_reader_opts`) that only Quarto's own reader injects;
//! * `--lua-filter main.lua` with the filters directory resolvable for `require`;
//! * `QUARTO_SHARE_PATH`, `QUARTO_FILTER_DEPENDENCY_FILE`, and `QUARTO_FILTER_PARAMS`
//!   holding base64 JSON with at least `quarto-filters.entryPoints`, `results-file`
//!   and `active-filters`.
//!
//! That params object is an internal contract that moves between Quarto versions, so
//! the robust way to fill it is to capture a real render's invocation rather than
//! enumerate it by hand. Until then, [`LuaWorker`] is the fast path for plain
//! markdown and the measurement baseline for everything else.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const WORKER_LUA: &str = include_str!("worker.lua");

/// One render through the warm worker.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub out: String,
    /// What the worker measured for `pandoc.read` + `pandoc.write` alone.
    pub ms: f64,
}

/// A long-lived `pandoc lua` process.
pub struct LuaWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl LuaWorker {
    pub fn start(pandoc: &Path) -> Result<Self> {
        let script = script_path()?;
        std::fs::write(&script, WORKER_LUA)
            .with_context(|| format!("writing {}", script.display()))?;

        let mut child = Command::new(pandoc)
            .arg("lua")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("could not start `pandoc lua`: {e}"))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);

        let mut worker = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        worker.ping().context("pandoc lua worker did not answer")?;
        Ok(worker)
    }

    /// Round-trip check that the worker is alive.
    pub fn ping(&mut self) -> Result<()> {
        self.call(json!({"op": "ping"}))?;
        Ok(())
    }

    /// Convert one fragment or document. `from`/`to` are pandoc format names.
    pub fn render(&mut self, text: &str, from: &str, to: &str) -> Result<Rendered> {
        let response = self.call(json!({
            "op": "render",
            "from": from,
            "to": to,
            "text": text,
        }))?;
        Ok(Rendered {
            out: response
                .get("out")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ms: response.get("ms").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    fn call(&mut self, mut request: Value) -> Result<Value> {
        self.next_id += 1;
        request["id"] = json!(self.next_id);

        let line = serde_json::to_string(&request)?;
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        let mut buffer = String::new();
        loop {
            buffer.clear();
            if self.stdout.read_line(&mut buffer)? == 0 {
                return Err(anyhow!("pandoc lua worker exited"));
            }
            let text = buffer.trim();
            if !text.starts_with('{') {
                continue;
            }
            let response: Value = serde_json::from_str(text)
                .with_context(|| format!("worker sent invalid JSON: {text}"))?;
            if response.get("ok").and_then(Value::as_bool) == Some(true) {
                return Ok(response);
            }
            let err = response
                .get("err")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(anyhow!("pandoc lua worker: {err}"));
        }
    }
}

impl Drop for LuaWorker {
    fn drop(&mut self) {
        // Closing stdin ends the worker's read loop; then make sure.
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn script_path() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("qlive-pandoc");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("qlive_pandoc_worker.lua"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qlive_core::Toolchain;

    /// Needs a real pandoc, so it skips rather than fails where there is none.
    fn worker() -> Option<LuaWorker> {
        let tools = Toolchain::discover().ok()?;
        LuaWorker::start(&tools.pandoc).ok()
    }

    #[test]
    fn renders_a_block() {
        let Some(mut worker) = worker() else {
            eprintln!("no pandoc found; skipping");
            return;
        };
        let out = worker
            .render("A paragraph with *emphasis*.", "markdown", "html")
            .expect("render");
        assert!(out.out.contains("<em>emphasis</em>"), "got {:?}", out.out);
    }

    #[test]
    fn stays_warm_across_calls() {
        let Some(mut worker) = worker() else {
            eprintln!("no pandoc found; skipping");
            return;
        };
        for i in 1..=5 {
            let out = worker
                .render(&format!("Paragraph {i}."), "markdown", "html")
                .expect("render");
            assert!(out.out.contains(&format!("Paragraph {i}.")));
        }
    }

    #[test]
    fn reports_lua_errors_without_dying() {
        let Some(mut worker) = worker() else {
            eprintln!("no pandoc found; skipping");
            return;
        };
        // An unknown reader is a pandoc.read error inside the worker's pcall.
        assert!(worker.render("text", "not-a-format", "html").is_err());
        // The worker must still be usable afterwards.
        assert!(worker.render("After.", "markdown", "html").is_ok());
    }

    #[test]
    fn renders_typst_too() {
        let Some(mut worker) = worker() else {
            eprintln!("no pandoc found; skipping");
            return;
        };
        let out = worker
            .render("## Heading", "markdown", "typst")
            .expect("render");
        assert!(out.out.contains("Heading"), "got {:?}", out.out);
    }
}
