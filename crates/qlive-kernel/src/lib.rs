//! A Jupyter kernel that outlives the render.
//!
//! Per-cell caching is not a hashing problem, it is a state problem: cell B uses the
//! `x` that cell A defined, so reusing A's outputs while skipping its execution is
//! only sound while the kernel still holds `x`. This crate owns that kernel.
//!
//! Measured on the development machine: kernel boot ~1.4 s, then **6–17 ms** per
//! cell. Quarto, by comparison, pays ~4.3 s of engine per render and re-executes every
//! cell whenever any cell changes.
//!
//! The kernel itself is driven by a small Python worker (`worker.py`) over a
//! JSON-per-line protocol, because `jupyter_client` already implements the wire
//! protocol correctly. Replacing it with `jupyter-protocol` + `zeromq` in-process is a
//! drop-in change behind [`Kernel`] — it needs no C toolchain, but it does need the
//! HMAC signing and five-socket routing that `jupyter_client` gives us for free.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

const WORKER_PY: &str = include_str!("worker.py");

/// What the analysis found in one cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub defines: Vec<String>,
    pub uses: Vec<String>,
    /// `exec`, `globals()`, star imports, magics, syntax errors: not analysable.
    pub opaque: bool,
}

/// What one cell execution produced.
#[derive(Debug, Clone)]
pub struct CellResult {
    /// nbformat-shaped outputs, ready to drop into a notebook.
    pub outputs: Vec<Value>,
    pub execution_count: Option<i64>,
    /// What the kernel took, as the worker measured it.
    pub ms: f64,
}

pub struct Kernel {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Receiver<String>,
    next_id: u64,
    /// Keys of the cells executed in this kernel, in order. The live state.
    executed: Vec<String>,
}

impl Kernel {
    /// Spawn the worker and boot a kernel. `cwd` is where cells will run, so relative
    /// paths in a document behave the same as under Quarto.
    pub fn start(python: &Path, cwd: &Path) -> Result<Self> {
        let mut kernel = Self::spawn(python, cwd)?;
        kernel
            .call(json!({"op": "start"}))
            .context("booting the Jupyter kernel")?;
        Ok(kernel)
    }

    /// The worker without a kernel: enough for [`Kernel::analyze`], which parses cells
    /// and never executes them.
    pub fn analysis_only(python: &Path) -> Result<Self> {
        Self::spawn(python, &std::env::temp_dir())
    }

    fn spawn(python: &Path, cwd: &Path) -> Result<Self> {
        let script = script_path()?;
        std::fs::write(&script, WORKER_PY)
            .with_context(|| format!("writing {}", script.display()))?;

        let mut child = Command::new(python)
            .arg(&script)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("could not start {}: {e}", python.display()))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);

        // Keep stderr for error messages: a missing jupyter_client otherwise shows up
        // only as a closed pipe.
        let (tx, stderr) = channel::<String>();
        if let Some(err) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            });
        }

        Ok(Self {
            child,
            stdin,
            stdout,
            stderr,
            next_id: 0,
            executed: Vec::new(),
        })
    }

    /// Which names each cell defines and reads, from Python's `ast` in the worker. The
    /// code is parsed, never executed.
    pub fn analyze(&mut self, codes: &[String]) -> Result<Vec<Analysis>> {
        let response = self.call(json!({"op": "analyze", "codes": codes}))?;
        let cells = response
            .get("cells")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("analyze: no cells in the reply"))?;
        let names = |v: &Value, key: &str| -> Vec<String> {
            v.get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        Ok(cells
            .iter()
            .map(|c| Analysis {
                defines: names(c, "defines"),
                uses: names(c, "uses"),
                opaque: c.get("opaque").and_then(Value::as_bool).unwrap_or(true),
            })
            .collect())
    }

    /// Delete names from the kernel's namespace — those only a removed cell defined —
    /// so their readers fail as they would in a fresh run.
    pub fn forget(&mut self, names: &[String]) -> Result<()> {
        self.call(json!({"op": "forget", "names": names}))?;
        Ok(())
    }

    /// Run code without recording it in the prefix history; the dependency planner
    /// keeps its own record of what the kernel holds.
    pub fn run(&mut self, code: &str) -> Result<CellResult> {
        let response = self.call(json!({"op": "execute", "code": code}))?;
        Ok(CellResult {
            outputs: response
                .get("outputs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            execution_count: response.get("execution_count").and_then(Value::as_i64),
            ms: response.get("ms").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    /// Run Quarto's kernel setup (`setup.py`, a `str.format` template) with the given
    /// arguments and `QUARTO_FIG_*` environment. Not part of the document: it is not
    /// recorded as executed, and it does not consume execution counts.
    pub fn setup(&mut self, script: &Path, args: &Value, env: &Value) -> Result<()> {
        self.call(json!({
            "op": "setup",
            "path": script.to_string_lossy(),
            "args": args,
            "env": env,
        }))
        .context("running Quarto's kernel setup")?;
        Ok(())
    }

    /// Run the code cell at `index`, recording its key as this kernel's live state.
    pub fn execute(&mut self, index: usize, key: &str, code: &str) -> Result<CellResult> {
        let response = self.call(json!({"op": "execute", "code": code}))?;
        let outputs = response
            .get("outputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let result = CellResult {
            outputs,
            execution_count: response.get("execution_count").and_then(Value::as_i64),
            ms: response.get("ms").and_then(Value::as_f64).unwrap_or(0.0),
        };
        qlive_core::cells::record_executed(&mut self.executed, index, key);
        Ok(result)
    }

    /// Record a cell as done without running it — `#| eval: false`. Without this the
    /// history would stop matching the document at that cell, and every cell after
    /// it would look dirty on every pass.
    pub fn record_skipped(&mut self, index: usize, key: &str) {
        qlive_core::cells::record_executed(&mut self.executed, index, key);
    }

    /// Keys executed in this kernel, in order.
    pub fn executed(&self) -> &[String] {
        &self.executed
    }

    /// Throw the state away: the next render has to execute from the top.
    pub fn restart(&mut self) -> Result<()> {
        self.call(json!({"op": "restart"}))?;
        self.executed.clear();
        Ok(())
    }

    fn call(&mut self, mut request: Value) -> Result<Value> {
        self.next_id += 1;
        request["id"] = json!(self.next_id);

        let started = Instant::now();
        let line = serde_json::to_string(&request)?;
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        let mut buffer = String::new();
        loop {
            buffer.clear();
            let read = self.stdout.read_line(&mut buffer)?;
            if read == 0 {
                return Err(anyhow!(
                    "kernel worker exited after {:?}{}",
                    started.elapsed(),
                    self.stderr_tail()
                ));
            }
            let text = buffer.trim();
            // The worker's replies are JSON objects; anything else is noise from
            // ipykernel and is ignored.
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
            return Err(anyhow!("kernel worker: {err}{}", self.stderr_tail()));
        }
    }

    fn stderr_tail(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = self.stderr.try_recv() {
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                lines.push(trimmed);
            }
        }
        if lines.is_empty() {
            String::new()
        } else {
            let tail: Vec<&str> = lines.iter().rev().take(3).map(String::as_str).rev().collect();
            format!("\nkernel stderr: {}", tail.join(" | "))
        }
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        // Best effort: ask nicely, then make sure.
        let _ = self.call(json!({"op": "shutdown"}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn script_path() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("qlive-kernel");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("qlive_kernel_worker.py"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The analysis needs a real Python; skip, rather than fail, where there is none.
    fn analyzer() -> Option<Kernel> {
        let python = qlive_core::Toolchain::discover().ok()?.python?;
        Kernel::analysis_only(&python).ok()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn analysis_finds_what_each_cell_defines_and_reads() {
        let Some(mut worker) = analyzer() else {
            eprintln!("no Python found; skipping");
            return;
        };
        // (code, defines, uses)
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("x = 1", &["x"], &[]),
            ("y = x + 1", &["y"], &["x"]),
            ("x = x + 1", &["x"], &["x"]),
            ("x += 1", &["x"], &["x"]),
            ("x = 1\ny = x + 1", &["x", "y"], &[]),
            ("y = x + 1\nx = 1", &["x", "y"], &["x"]),
            // mutation that is tracked
            ("data.append(2)", &["data"], &["data"]),
            ("df.dropna(inplace=True)", &["df"], &["df"]),
            ("plt.rcParams['figure.figsize'] = (4, 3)", &["plt"], &["plt"]),
            ("df['total'] = df['a'] + b", &["df"], &["df", "b"]),
            // calls that must NOT count as mutation
            ("df.head()", &[], &["df"]),
            ("m = np.mean(a)", &["m"], &["np", "a"]),
            ("import numpy as np\nm = np.mean(a)", &["np", "m"], &["a"]),
            // scopes
            ("def f():\n    return g + 1", &["f"], &["g"]),
            ("def f(a):\n    return a + 1", &["f"], &[]),
            ("def f(a):\n    global counter\n    counter = a", &["f", "counter"], &[]),
            ("h = lambda x: x + y", &["h"], &["y"]),
            ("r = [i * k for i in data]", &["r"], &["data", "k"]),
            ("r = {k: v for k, v in pairs if v > t}", &["r"], &["pairs", "t"]),
            ("for i in range(3):\n    total += i", &["i", "total"], &["range", "total"]),
            ("with open(p) as f:\n    text = f.read()", &["f", "text"], &["open", "p"]),
            (
                "class C:\n    k = base\n    def m(self):\n        return self.k + other",
                &["C"],
                &["base", "other"],
            ),
            ("if (n := len(xs)) > 3:\n    big = n", &["n", "big"], &["len", "xs"]),
            ("fig, ax = plt.subplots()\nax.plot(data)\nplt.show()", &["fig", "ax"], &["plt", "data"]),
        ];
        let codes: Vec<String> = cases.iter().map(|c| c.0.to_string()).collect();
        let found = worker.analyze(&codes).expect("analyze");
        for ((code, defines, uses), got) in cases.iter().zip(found) {
            assert!(!got.opaque, "{code:?} should be analysable");
            assert_eq!(got.defines.iter().cloned().collect::<BTreeSet<_>>(), set(defines), "defines of {code:?}");
            assert_eq!(got.uses.iter().cloned().collect::<BTreeSet<_>>(), set(uses), "uses of {code:?}");
        }
    }

    #[test]
    fn unanalysable_cells_are_opaque() {
        let Some(mut worker) = analyzer() else {
            eprintln!("no Python found; skipping");
            return;
        };
        let codes: Vec<String> = ["exec('x = 1')", "%matplotlib inline", "from math import *", "print(globals())"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for (code, got) in codes.iter().zip(worker.analyze(&codes).expect("analyze")) {
            assert!(got.opaque, "{code:?} must be opaque");
        }
    }
}
