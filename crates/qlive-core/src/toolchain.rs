//! Finding the binaries qlive reuses.
//!
//! qlive never ships its own pandoc or typst: it borrows the ones Quarto already
//! installed, so that output matches what `quarto render` would produce. On this
//! machine that means:
//!
//! ```text
//! C:\Program Files\Quarto\bin\quarto.exe
//! C:\Program Files\Quarto\bin\tools\pandoc.exe
//! C:\Program Files\Quarto\bin\tools\x86_64\typst.exe
//! ```

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Toolchain {
    pub quarto: PathBuf,
    pub pandoc: PathBuf,
    /// Optional: only the typst output path needs it.
    pub typst: Option<PathBuf>,
    /// Optional: only documents with code cells need it.
    pub python: Option<PathBuf>,
}

impl Toolchain {
    /// Env overrides first (`QLIVE_QUARTO`, `QLIVE_PANDOC`, `QLIVE_TYPST`), then
    /// Quarto's own bundled tools, then PATH.
    pub fn discover() -> Result<Self> {
        let quarto = from_env("QLIVE_QUARTO")
            .or_else(|| which("quarto"))
            .ok_or_else(|| anyhow!("quarto not found on PATH; set QLIVE_QUARTO"))?;

        let tools = bundled_tools(&quarto);

        let pandoc = from_env("QLIVE_PANDOC")
            .or_else(|| tools.as_ref().and_then(|d| existing(&d.join(exe("pandoc")))))
            .or_else(|| which("pandoc"))
            .ok_or_else(|| anyhow!("pandoc not found next to quarto or on PATH; set QLIVE_PANDOC"))?;

        let typst = from_env("QLIVE_TYPST")
            .or_else(|| tools.as_ref().and_then(|d| find_typst(d)))
            .or_else(|| which("typst"));

        let python = from_env("QLIVE_PYTHON")
            .or_else(|| from_env("QUARTO_PYTHON"))
            .or_else(find_python);

        Ok(Self {
            quarto,
            pandoc,
            typst,
            python,
        })
    }

    /// `(name, version line)` for each tool found, for `qlive doctor`.
    pub fn versions(&self) -> Vec<(String, String)> {
        let mut out = vec![
            ("quarto".to_string(), first_line(&self.quarto)),
            ("pandoc".to_string(), first_line(&self.pandoc)),
        ];
        match &self.typst {
            Some(t) => out.push(("typst".to_string(), first_line(t))),
            None => out.push(("typst".to_string(), "not found".to_string())),
        }
        match &self.python {
            Some(p) => out.push(("python".to_string(), first_line(p))),
            None => out.push(("python".to_string(), "not found".to_string())),
        }
        out
    }
}

/// Find a *usable* Python 3.
///
/// On Windows, `python.exe` on PATH is often the Microsoft Store alias stub in
/// `WindowsApps`. Executing it from a non-interactive process fails with
/// `[WinError 5] Access is denied`, so those are skipped explicitly — this is exactly
/// how Quarto's own daemon path fails on a machine with only the stub installed.
fn find_python() -> Option<PathBuf> {
    for stem in ["python3", "python"] {
        let name = exe(stem);
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                if is_store_alias(&dir) {
                    continue;
                }
                if let Some(found) = existing(&dir.join(&name)) {
                    return Some(found);
                }
            }
        }
    }

    // The usual per-user install location, newest first.
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let programs = base.join("Programs").join("Python");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&programs)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    candidates.sort();
    candidates
        .iter()
        .rev()
        .find_map(|dir| existing(&dir.join(exe("python"))))
}

fn is_store_alias(dir: &Path) -> bool {
    dir.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("WindowsApps")
    })
}

fn exe(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

fn existing(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn from_env(key: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(key)?;
    existing(Path::new(&raw))
}

fn which(stem: &str) -> Option<PathBuf> {
    let name = exe(stem);
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| existing(&dir.join(&name)))
}

/// Quarto keeps pandoc, typst, deno and dart-sass under `<quarto bin>/tools`.
fn bundled_tools(quarto: &Path) -> Option<PathBuf> {
    let dir = quarto.parent()?.join("tools");
    if dir.is_dir() {
        Some(dir)
    } else {
        None
    }
}

/// typst lives either directly in `tools/` or in an arch subdirectory.
fn find_typst(tools: &Path) -> Option<PathBuf> {
    let name = exe("typst");
    for sub in ["", "x86_64", "aarch64"] {
        let candidate = if sub.is_empty() {
            tools.join(&name)
        } else {
            tools.join(sub).join(&name)
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn first_line(bin: &Path) -> String {
    match Command::new(bin).arg("--version").output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string(),
        Err(e) => format!("error: {e}"),
    }
}
