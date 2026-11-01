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
use std::sync::mpsc::{channel, Receiver};

const WORKER_LUA: &str = include_str!("worker.lua");

/// One render through the warm worker.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub out: String,
    /// What the worker measured for the pipeline alone (CPU time, inside Lua).
    pub ms: f64,
    /// Anything pandoc or Quarto's filters wrote to stderr meanwhile. Quarto reports
    /// FATAL filter errors here, and a cold `pandoc` still exits 0 after one, so this
    /// is where failure shows up.
    pub diagnostics: Vec<String>,
}

impl Rendered {
    pub fn has_fatal(&self) -> bool {
        self.diagnostics.iter().any(|line| line.contains("FATAL"))
    }
}

/// A long-lived `pandoc lua` process.
pub struct LuaWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Receiver<String>,
    next_id: u64,
}

impl LuaWorker {
    /// Plain pandoc: read + write, no filters.
    pub fn start(pandoc: &Path) -> Result<Self> {
        Self::spawn(pandoc, &[], None)
    }

    /// Quarto's own pipeline — its reader, `main.lua` and writer options — hosted
    /// warm. `env` and `cwd` come from a captured real render (`qlive_quarto::capture`);
    /// without them `init.lua` cannot load.
    pub fn start_quarto(
        pandoc: &Path,
        env: &[(String, String)],
        cwd: &Path,
        format: &str,
        citeproc: bool,
    ) -> Result<Self> {
        let mut worker = Self::spawn(pandoc, env, Some(cwd))?;
        worker
            .call(json!({"op": "quarto_init", "format": format, "citeproc": citeproc}))
            .context("loading Quarto's pipeline into the worker")?;
        Ok(worker)
    }

    fn spawn(pandoc: &Path, env: &[(String, String)], cwd: Option<&Path>) -> Result<Self> {
        let script = script_path()?;
        std::fs::write(&script, WORKER_LUA)
            .with_context(|| format!("writing {}", script.display()))?;

        let mut command = Command::new(pandoc);
        command
            .arg("lua")
            .arg(&script)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let mut child = command
            .spawn()
            .map_err(|e| anyhow!("could not start `pandoc lua`: {e}"))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let (tx, stderr) = channel::<String>();
        if let Some(err) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            });
        }

        let mut worker = Self {
            child,
            stdin,
            stdout,
            stderr,
            next_id: 0,
        };
        worker.ping().context("pandoc lua worker did not answer")?;
        Ok(worker)
    }

    /// Render through Quarto's pipeline. `meta` is the YAML Quarto passes with
    /// `--metadata-file`; the output is a body fragment, not a standalone page — the
    /// page shell comes from the cold render.
    pub fn render_quarto(&mut self, text: &str, meta: &str) -> Result<Rendered> {
        let response = self.call(json!({
            "op": "quarto_render",
            "text": text,
            "meta": meta,
        }))?;
        Ok(self.rendered(&response))
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
        Ok(self.rendered(&response))
    }

    fn rendered(&self, response: &Value) -> Rendered {
        Rendered {
            out: response
                .get("out")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ms: response.get("ms").and_then(Value::as_f64).unwrap_or(0.0),
            diagnostics: self.drain_stderr(),
        }
    }

    fn drain_stderr(&self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(line) = self.stderr.try_recv() {
            if !line.trim().is_empty() {
                lines.push(line);
            }
        }
        lines
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
                .unwrap_or("unknown error")
                .to_string();
            let stderr = self.drain_stderr();
            return Err(if stderr.is_empty() {
                anyhow!("pandoc lua worker: {err}")
            } else {
                anyhow!("pandoc lua worker: {err}\nstderr: {}", stderr.join(" | "))
            });
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

    /// The whole chain: capture a real render, host Quarto's pipeline warm, and get
    /// Quarto-only markup back, twice, from one process.
    #[test]
    #[ignore = "runs a real quarto render; cargo test -- --ignored"]
    fn quarto_pipeline_renders_a_callout_warm() {
        let tools = Toolchain::discover().expect("quarto and pandoc");
        let dir = std::env::temp_dir().join("qlive-quarto-warm-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("callout.qmd");
        std::fs::write(
            &doc,
            "---\ntitle: Warm\nformat: html\n---\n\n## Heading\n\n\
             ::: {.callout-note}\nOnly Quarto filters understand this.\n:::\n",
        )
        .unwrap();

        let cap = qlive_quarto::capture::capture(&tools.quarto, &doc, Some("html")).expect("capture");
        let mut worker =
            LuaWorker::start_quarto(&tools.pandoc, &cap.env, &cap.cwd, &cap.format, cap.citeproc).expect("start");

        let first = worker.render_quarto(&cap.input, &cap.metadata).expect("render");
        assert!(first.out.contains("callout-titled"), "no Quarto callout in {:?}", first.out);
        assert!(!first.has_fatal(), "{:?}", first.diagnostics);

        let edited = cap.input.replace("Only Quarto", "Still only Quarto");
        let second = worker.render_quarto(&edited, &cap.metadata).expect("second render");
        assert!(second.out.contains("Still only Quarto"));
        assert!(second.out.contains("callout-titled"));
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
