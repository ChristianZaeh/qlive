//! Absolute paths that other programs will accept.
//!
//! `Path::canonicalize` is the obvious choice and the wrong one on Windows: it
//! returns an extended-length path (`\\?\C:\…`). Passing one to Quarto gets you
//!
//! ```text
//! ERROR: The filename, directory name, or volume label syntax is incorrect.
//! (os error 123): stat '?\C:\Users\…\bench.md'
//! ```
//!
//! because Deno's `statSync` cannot parse the verbatim prefix. Every path qlive hands
//! to a subprocess goes through [`absolute`] instead.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Absolute, lexically normalised, and free of Windows' `\\?\` prefix. The path must
/// exist — this is also qlive's "can I open your document?" check.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        return Err(anyhow!("{} does not exist", path.display()));
    }
    let abs = std::path::absolute(path)
        .map_err(|e| anyhow!("cannot resolve {}: {e}", path.display()))?;
    Ok(strip_verbatim(&abs))
}

/// `\\?\C:\x` -> `C:\x`, `\\?\UNC\server\share` -> `\\server\share`, others unchanged.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_verbatim_prefix() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\dev\qlive\doc.qmd")),
            PathBuf::from(r"C:\dev\qlive\doc.qmd")
        );
    }

    #[test]
    fn strips_verbatim_unc() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\nas\share\doc.qmd")),
            PathBuf::from(r"\\nas\share\doc.qmd")
        );
    }

    #[test]
    fn leaves_ordinary_paths_alone() {
        assert_eq!(
            strip_verbatim(Path::new(r"C:\dev\qlive\doc.qmd")),
            PathBuf::from(r"C:\dev\qlive\doc.qmd")
        );
        assert_eq!(
            strip_verbatim(Path::new("/home/me/doc.qmd")),
            PathBuf::from("/home/me/doc.qmd")
        );
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(absolute(Path::new("definitely-not-here-9f3a.qmd")).is_err());
    }
}
