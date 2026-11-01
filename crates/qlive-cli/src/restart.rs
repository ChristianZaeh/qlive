//! Restart cache: what one cold render taught us, so the next `qlive live` skips it.
//!
//! The cold render exists to learn Quarto's environment, metadata and page shell for
//! this document. Those depend on the front matter, project and directory metadata,
//! the files they name, the Quarto version and qlive itself, never on the body, so an
//! entry keyed on those stays valid however the body changes. A cached start renders
//! the current body warm; it never shows the cached page's stale content.

use anyhow::{anyhow, Context, Result};
use qlive_core::Toolchain;
use qlive_quarto::Capture;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Bump when the entry layout or what a capture means changes.
const LAYOUT: &str = "1";
/// Entries unused this long are removed when a new one is stored.
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

pub struct Entry {
    pub capture: Capture,
    pub page: String,
    pub page_file: PathBuf,
}

fn root() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("qlive").join("restart"))
}

/// Everything a cold render's result depends on except the body.
pub fn key(tools: &Toolchain, doc: &Path, mode: &str, front_matter: Option<&str>, source_metadata: &[(PathBuf, String)]) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut part = |label: &str, bytes: &[u8]| {
        hasher.update(label.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    part("layout", LAYOUT.as_bytes());
    // Any rebuild of qlive may change what a capture means.
    let exe = std::env::current_exe()?;
    part("qlive", stamp(&exe).as_bytes());
    part("quarto", stamp(&tools.quarto).as_bytes());
    if let Some(share) = tools.quarto.parent().and_then(Path::parent) {
        part("quarto-version", &std::fs::read(share.join("share").join("version")).unwrap_or_default());
    }
    part("doc", doc.to_string_lossy().as_bytes());
    part("mode", mode.as_bytes());
    part("profile", std::env::var("QUARTO_PROFILE").unwrap_or_default().as_bytes());

    let dir = doc.parent().ok_or_else(|| anyhow!("document has no parent directory"))?;
    let mut yaml: Vec<(PathBuf, String)> = Vec::new();
    yaml.push((dir.to_path_buf(), front_matter.unwrap_or_default().to_owned()));
    for (file, front) in source_metadata {
        yaml.push((file.parent().unwrap_or(dir).to_path_buf(), front.clone()));
    }
    for ancestor in dir.ancestors() {
        for name in ["_quarto.yml", "_quarto.yaml", "_metadata.yml", "_metadata.yaml", "_brand.yml", "_brand.yaml"] {
            if let Ok(text) = std::fs::read_to_string(ancestor.join(name)) {
                yaml.push((ancestor.to_path_buf(), text));
            }
        }
    }
    for (base, text) in &yaml {
        part("yaml", text.as_bytes());
        for file in named_files(base, text) {
            part("file", file.to_string_lossy().as_bytes());
            part("content", &std::fs::read(&file).unwrap_or_default());
        }
    }
    Ok(hasher.finalize().to_hex()[..32].to_owned())
}

fn stamp(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let modified = meta.as_ref().and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}|{}|{modified}", path.display(), meta.map(|m| m.len()).unwrap_or(0))
}

/// Existing files a YAML text names relative to its directory: themes, CSS, CSL,
/// bibliographies, templates, includes. Tokenizing beats modelling every key.
fn named_files(base: &Path, text: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = text
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '[' | ']' | '{' | '}'))
        .map(|token| token.trim_end_matches(':'))
        .filter(|token| token.contains('.') && !token.contains("://") && token.len() < 260)
        .map(|token| base.join(token))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files.dedup();
    files
}

pub fn load(key: &str) -> Option<Entry> {
    let folder = root()?.join(key);
    let value: Value = serde_json::from_slice(&std::fs::read(folder.join("entry.json")).ok()?).ok()?;
    let capture = capture_from(&value["capture"])?;
    let page_file = PathBuf::from(value["page_file"].as_str()?);
    let page = value["page"].as_str()?.to_owned();
    if !resources_exist(&page, page_file.parent()?) { return None; }
    // The capture directory belongs to the document, not the entry: a later cold
    // render with other front matter replaces it, so put this entry's copy back.
    if capture.dir.exists() { std::fs::remove_dir_all(&capture.dir).ok()?; }
    copy_dir(&folder.join("capture"), &capture.dir).ok()?;
    let _ = filetime_touch(&folder.join("entry.json"));
    Some(Entry { capture, page, page_file })
}

pub fn store(key: &str, capture: &Capture, page: &str, page_file: &Path) -> Result<()> {
    let root = root().ok_or_else(|| anyhow!("no cache directory"))?;
    prune(&root);
    let folder = root.join(key);
    let staging = root.join(format!("{key}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    copy_dir(&capture.dir, &staging.join("capture"))?;
    let entry = json!({"capture": capture_to(capture), "page": page, "page_file": page_file});
    std::fs::write(staging.join("entry.json"), serde_json::to_vec(&entry)?)?;
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::rename(&staging, &folder).with_context(|| format!("storing {}", folder.display()))?;
    Ok(())
}

fn capture_to(capture: &Capture) -> Value {
    json!({
        "env": capture.env, "cwd": capture.cwd, "input": capture.input, "metadata": capture.metadata,
        "format": capture.format, "citeproc": capture.citeproc, "dir": capture.dir,
    })
}

fn capture_from(value: &Value) -> Option<Capture> {
    let env = value["env"].as_array()?.iter()
        .map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned())))
        .collect::<Option<Vec<_>>>()?;
    Some(Capture {
        env,
        cwd: PathBuf::from(value["cwd"].as_str()?),
        input: value["input"].as_str()?.to_owned(),
        metadata: value["metadata"].as_str()?.to_owned(),
        format: value["format"].as_str()?.to_owned(),
        citeproc: value["citeproc"].as_bool()?,
        dir: PathBuf::from(value["dir"].as_str()?),
    })
}

/// Every local stylesheet, script and image the page loads must still be on disk.
/// Ordinary links are not loaded, and may point at documents never rendered.
fn resources_exist(page: &str, dir: &Path) -> bool {
    let pattern = regex::Regex::new(r##"<link\b[^>]*\bhref="([^"#?]+)"|\bsrc="([^"#?]+)""##).unwrap();
    let all = pattern.captures_iter(page).all(|c| {
        let link = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
        link.contains(':') || link.starts_with('/') || dir.join(link.replace("%20", " ")).exists()
    });
    all
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for item in std::fs::read_dir(from)? {
        let item = item?;
        let target = to.join(item.file_name());
        if item.file_type()?.is_dir() { copy_dir(&item.path(), &target)?; } else { std::fs::copy(item.path(), target)?; }
    }
    Ok(())
}

/// Mark an entry as used, so pruning keeps it.
fn filetime_touch(path: &Path) -> std::io::Result<()> {
    std::fs::File::options().append(true).open(path)?.set_modified(std::time::SystemTime::now())
}

fn prune(root: &Path) {
    let Ok(items) = std::fs::read_dir(root) else { return };
    for item in items.flatten() {
        let used = std::fs::metadata(item.path().join("entry.json")).and_then(|m| m.modified()).ok();
        let stale = used.map_or(true, |t| t.elapsed().map_or(false, |age| age > MAX_AGE));
        if stale { let _ = std::fs::remove_dir_all(item.path()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_files_named_in_yaml() {
        let dir = std::env::temp_dir().join(format!("qlive-restart-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("styles")).unwrap();
        std::fs::write(dir.join("styles/custom.scss"), "a{}").unwrap();
        std::fs::write(dir.join("refs.bib"), "@book{}").unwrap();
        let yaml = "format:\n  html:\n    theme: [cosmo, styles/custom.scss]\nbibliography: \"refs.bib\"\ncsl: missing.csl\n";
        let files = named_files(&dir, yaml);
        assert_eq!(files, vec![dir.join("refs.bib"), dir.join("styles/custom.scss")]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_page_resources_invalidate() {
        let dir = std::env::temp_dir().join(format!("qlive-restart-res-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("t_files")).unwrap();
        std::fs::write(dir.join("t_files/a.css"), "").unwrap();
        assert!(resources_exist(r##"<link href="t_files/a.css"><a href="#x"><a href="unrendered.html"><img src="https://x/y.png">"##, &dir));
        assert!(!resources_exist(r#"<script src="t_files/gone.js"></script>"#, &dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
