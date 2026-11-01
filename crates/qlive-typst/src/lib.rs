//! Typst, the fast PDF path.
//!
//! M0 drives Quarto's bundled `typst.exe` as a subprocess, because that is enough to
//! *measure* the incremental number the design depends on: a cold one-shot compile is
//! process start and font loading, while an edit under `typst watch` is a rebuild.
//!
//! M4 replaces this with the `typst` crate used as a library, pinned to the version
//! Quarto bundles (0.13), so that `comemo`'s incremental cache survives between edits
//! in-process and pages can be rendered to SVG individually.
//!
//! ## Why the output file, not typst's own numbers
//!
//! `typst watch` prints `compiled successfully in 8.96 ms`, which is tempting to
//! parse — and unreliable. When its output is not a terminal it still repaints the
//! whole status block, rewinding the stream rather than appending, so a reader sees
//! a few hundred bytes total no matter how many rebuilds happened, and a line-based
//! reader can wait forever. [`TypstWatch::time_edit`] therefore times the source
//! write until the output file changes. That also measures the right thing: it
//! includes typst's file-watch latency, which is part of what a human waits for.
//! [`TypstWatch::reported_ms`] still reports typst's own figure when a line happens
//! to arrive, as the pure-typesetting component of that total.

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant, SystemTime};

/// Compile once through a fresh process. The baseline, not the hot path.
pub fn one_shot(typst: &Path, input: &Path, output: &Path) -> Result<Duration> {
    let started = Instant::now();
    let out = Command::new(typst)
        .arg("compile")
        .arg(input)
        .arg(output)
        .output()
        .map_err(|e| anyhow!("could not run typst: {e}"))?;
    let elapsed = started.elapsed();
    if !out.status.success() {
        return Err(anyhow!(
            "typst compile failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(elapsed)
}

/// A long-lived `typst watch` process.
pub struct TypstWatch {
    child: Child,
    lines: Receiver<String>,
    source: PathBuf,
    output: PathBuf,
}

impl TypstWatch {
    pub fn start(typst: &Path, input: &Path, output: &Path) -> Result<Self> {
        let mut child = Command::new(typst)
            .arg("watch")
            .arg(input)
            .arg(output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("could not start `typst watch`: {e}"))?;

        let (tx, lines) = channel::<String>();
        if let Some(stdout) = child.stdout.take() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            });
        }

        Ok(Self {
            child,
            lines,
            source: input.to_path_buf(),
            output: output.to_path_buf(),
        })
    }

    /// `Some(status)` once the typst process has exited. A watch that quietly died is
    /// indistinguishable from one that never rebuilds, so every timeout reports this.
    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    fn why_stuck(&mut self) -> String {
        match self.exited() {
            Some(status) => format!("typst watch exited ({status})"),
            None => format!(
                "typst watch still running; last said: {}",
                self.last_line().unwrap_or_else(|| "nothing".to_string())
            ),
        }
    }

    /// Wait for the compile typst does on startup: the output has to exist and then
    /// stop changing. Call once before timing edits, or the first measurement races
    /// with the startup write.
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(seen) = fingerprint(&self.output) {
                // Settled when it has not moved for 250 ms.
                let settle = Instant::now() + Duration::from_millis(250);
                let mut stable = true;
                while Instant::now() < settle {
                    std::thread::sleep(Duration::from_millis(10));
                    if fingerprint(&self.output) != Some(seen) {
                        stable = false;
                        break;
                    }
                }
                if stable {
                    return Ok(());
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Err(anyhow!(
            "typst watch produced no output within {timeout:?} — {}",
            self.why_stuck()
        ))
    }

    /// Append `text` to the watched source, and time how long until the output file
    /// changes. Includes typst's file-watch latency by design.
    ///
    /// Retries, because `typst watch` does sometimes miss an edit entirely: observed
    /// on Windows with the process still alive and its last line a successful
    /// startup compile, while the output never moved for a full minute. File watchers
    /// drop events — which is also why M4 wants typst in-process rather than watched.
    pub fn time_edit(&mut self, text: &str, timeout: Duration) -> Result<f64> {
        /// How long one edit gets before we poke the file again.
        const PER_ATTEMPT: Duration = Duration::from_secs(5);

        let deadline = Instant::now() + timeout;
        let mut attempts = 0usize;

        while Instant::now() < deadline {
            attempts += 1;
            let before = fingerprint(&self.output);
            let mut source = std::fs::read_to_string(&self.source)?;
            source.push_str(text);

            let started = Instant::now();
            std::fs::write(&self.source, source)?;

            let give_up = Instant::now() + PER_ATTEMPT;
            while Instant::now() < give_up {
                if fingerprint(&self.output) != before {
                    return Ok(started.elapsed().as_secs_f64() * 1000.0);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        // Both pieces are computed first: `self.output` borrows immutably while
        // `why_stuck` needs &mut, and one expression cannot hold both.
        let output = self.output.display().to_string();
        let why = self.why_stuck();
        Err(anyhow!(
            "typst watch: {output} unchanged after {timeout:?} and {attempts} edits — {why}"
        ))
    }

    /// typst's own reported compile time, if any status line reached us. Best effort:
    /// see the note at the top of this module.
    pub fn reported_ms(&self) -> Option<f64> {
        let mut latest = None;
        while let Ok(line) = self.lines.try_recv() {
            if let Some(ms) = parse_compile_ms(&line) {
                latest = Some(ms);
            }
        }
        latest
    }

    fn last_line(&self) -> Option<String> {
        let mut latest = None;
        while let Ok(line) = self.lines.try_recv() {
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                latest = Some(trimmed);
            }
        }
        latest
    }
}

impl Drop for TypstWatch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Cheap "did this file change" check: modification time plus length.
fn fingerprint(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// `[13:44:54] compiled successfully in 8.96 ms` -> `8.96`
fn parse_compile_ms(line: &str) -> Option<f64> {
    const NEEDLE: &str = "compiled successfully in ";
    let at = line.find(NEEDLE)?;
    let rest = &line[at + NEEDLE.len()..];
    let number: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = number.parse().ok()?;
    // typst switches to seconds for slow compiles.
    let unit = rest[number.len()..].trim_start();
    if unit.starts_with("ms") {
        Some(value)
    } else if unit.starts_with('s') {
        Some(value * 1000.0)
    } else {
        Some(value)
    }
}

/// A minimal document for benchmarking, shaped like Quarto's typst output.
pub fn fixture(sections: usize) -> String {
    let mut out = String::from("#set page(paper: \"a4\")\n#set text(size: 11pt)\n\n= Bench doc\n\n");
    for i in 1..=sections {
        out.push_str(&format!(
            "== Section {i}\n\nParagraph {i} with _emphasis_, `code` and math $a_{i}^2 + b^2 = c^2$.\n\n- item one\n- item two\n\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_milliseconds() {
        assert_eq!(
            parse_compile_ms("[13:44:54] compiled successfully in 8.96 ms"),
            Some(8.96)
        );
    }

    #[test]
    fn parses_seconds_as_milliseconds() {
        assert_eq!(
            parse_compile_ms("[13:44:54] compiled successfully in 1.5 s"),
            Some(1500.0)
        );
    }

    #[test]
    fn ignores_other_lines() {
        assert_eq!(parse_compile_ms("watching fixture.typ"), None);
    }

    #[test]
    fn fingerprint_is_none_for_missing_files() {
        assert!(fingerprint(Path::new("definitely-not-here-4b1c.pdf")).is_none());
    }

    #[test]
    fn fixture_has_a_section_per_request() {
        let text = fixture(3);
        assert_eq!(text.matches("== Section").count(), 3);
        assert!(text.contains("== Section 3"));
    }
}
