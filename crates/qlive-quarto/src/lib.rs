//! Cold renders. Quarto stays the authority.
//!
//! qlive shells out to `quarto render` once per session to get the real output —
//! template, theme CSS, JS, resources, filters, bibliography — and then only ships
//! patches against it. Anything qlive cannot do confidently comes back here.

pub mod capture;
pub mod shell;

pub use capture::Capture;
pub use shell::Shell;

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ColdRender {
    /// The file Quarto produced.
    pub output: PathBuf,
    /// Wall time of the whole `quarto render` call, Deno startup included.
    pub duration: Duration,
}

/// Render a notebook that already carries its outputs, without touching a kernel.
///
/// This is the seam the per-cell executor is built on: we own execution, Quarto owns
/// markdown, filters, themes and formats. Measured at ~1.9 s with zero cells executed,
/// against ~6.4 s when Quarto runs them itself.
pub fn render_no_execute(quarto: &Path, doc: &Path, to: Option<&str>) -> Result<ColdRender> {
    render_with(quarto, doc, to, &["--no-execute"])
}

/// Run `quarto render <doc> [--to <fmt>]` in the document's own directory.
pub fn render(quarto: &Path, doc: &Path, to: Option<&str>) -> Result<ColdRender> {
    render_with(quarto, doc, to, &[])
}

fn render_with(quarto: &Path, doc: &Path, to: Option<&str>, extra: &[&str]) -> Result<ColdRender> {
    // Not canonicalize(): on Windows that yields a `\\?\` path, which Quarto's Deno
    // runtime cannot stat.
    let doc = qlive_core::absolute(doc)?;

    let mut cmd = Command::new(quarto);
    cmd.arg("render").arg(&doc);
    if let Some(fmt) = to {
        cmd.arg("--to").arg(fmt);
    }
    cmd.args(extra);
    if let Some(dir) = doc.parent() {
        cmd.current_dir(dir);
    }

    let started = Instant::now();
    let out = cmd
        .output()
        .map_err(|e| anyhow!("could not run quarto: {e}"))?;
    let duration = started.elapsed();

    if !out.status.success() {
        return Err(anyhow!(
            "quarto render failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Ok(ColdRender {
        output: output_path(&doc, to),
        duration,
    })
}

/// Where Quarto puts the result for a single-file render.
pub fn output_path(doc: &Path, to: Option<&str>) -> PathBuf {
    let ext = match to.unwrap_or("html") {
        "html" | "html4" | "html5" => "html",
        "pdf" | "typst" | "latex" | "pdflatex" | "beamer" => "pdf",
        "docx" => "docx",
        "revealjs" => "html",
        other => other,
    };
    doc.with_extension(ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_extension_follows_format() {
        assert_eq!(
            output_path(Path::new("a/b.qmd"), None),
            PathBuf::from("a/b.html")
        );
        assert_eq!(
            output_path(Path::new("a/b.qmd"), Some("typst")),
            PathBuf::from("a/b.pdf")
        );
        assert_eq!(
            output_path(Path::new("a/b.qmd"), Some("docx")),
            PathBuf::from("a/b.docx")
        );
    }
}
