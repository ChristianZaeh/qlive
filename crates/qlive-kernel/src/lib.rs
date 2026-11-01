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

        let mut kernel = Self {
            child,
            stdin,
            stdout,
            stderr,
            next_id: 0,
            executed: Vec::new(),
        };

        let boot = kernel
            .call(json!({"op": "start"}))
            .context("booting the Jupyter kernel")?;
        let _ = boot;
        Ok(kernel)
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
