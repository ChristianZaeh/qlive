//! Capture Quarto's own pandoc invocation, so a warm process can replay it.
//!
//! Quarto has this built in: with `QUARTO_CAPTURE_RENDER_COMMAND=<dir>`, `quarto
//! render` writes `<dir>/render-command.json` — pandoc binary, args, cwd, env — just
//! before running pandoc. That gives us the 72 filter params, the defaults file and
//! the metadata file from Quarto itself instead of reconstructing them.
//!
//! Replaying it needs four corrections, each learned from a failure:
//!
//! * **Quarto's process env**, not just the two variables it passes pandoc: without
//!   `QUARTO_SHARE_PATH`, `init.lua` cannot find its own modules.
//! * **Placeholders for session files.** `include-*-body` and `notebook-context` live
//!   in Quarto's session temp dir, which is deleted after every render and is not
//!   copied by the capture. They only feed the page shell, which the hot path takes
//!   from the cold render anyway.
//! * **A writable results file and dependency file**, for the same reason.
//! * **Scrubbing.** The capture writes the *entire* process environment to disk
//!   under `ourEnv` — tokens included. Only `QUARTO_*` survives here, and the file
//!   on disk is rewritten without the rest.

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Everything a warm worker needs to replay Quarto's pandoc call.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Environment for the worker: Quarto's `QUARTO_*`, `LUA_CPATH`, and rebased
    /// filter params.
    pub env: Vec<(String, String)>,
    /// Where Quarto ran pandoc: the document's directory.
    pub cwd: PathBuf,
    /// The preprocessed markdown Quarto handed pandoc.
    pub input: String,
    /// The metadata file Quarto handed pandoc (YAML).
    pub metadata: String,
    /// The pandoc output format, e.g. `html`.
    pub format: String,
    /// The capture directory, holding placeholders and results.
    pub dir: PathBuf,
}

/// Render `doc` once with Quarto, capturing its pandoc invocation.
pub fn capture(quarto: &Path, doc: &Path, to: Option<&str>) -> Result<Capture> {
    let doc = qlive_core::absolute(doc)?;
    let cwd = doc
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?
        .to_path_buf();

    let dir = std::env::temp_dir()
        .join("qlive-capture")
        .join(short_hash(&doc.to_string_lossy()));
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut cmd = Command::new(quarto);
    cmd.arg("render")
        .arg(&doc)
        .current_dir(&cwd)
        .env("QUARTO_CAPTURE_RENDER_COMMAND", &dir);
    if let Some(fmt) = to {
        cmd.arg("--to").arg(fmt);
    }
    let out = cmd.output().map_err(|e| anyhow!("could not run quarto: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "quarto render failed during capture:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let command_file = dir.join("render-command.json");
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(&command_file)
            .with_context(|| format!("reading {}", command_file.display()))?,
    )?;

    // Never leave the full process environment on disk.
    std::fs::write(&command_file, serde_json::to_string_pretty(&scrubbed(&raw))?)?;

    let env = prepare_env(&raw, &dir)?;
    let args = string_list(&raw["args"]);
    let input = read_arg_file(&args, |a| a.contains("quarto-input") && a.ends_with(".md"))?;
    let metadata = read_flag_file(&args, "--metadata-file")?;
    let defaults = read_flag_file(&args, "--defaults")?;
    let format = yaml_scalar(&defaults, "to").unwrap_or_else(|| "html".to_string());

    Ok(Capture {
        env,
        cwd,
        input,
        metadata,
        format,
        dir,
    })
}

/// The worker environment for a captured command. Pure, so it can be tested
/// without Quarto.
pub fn prepare_env(raw: &Value, dir: &Path) -> Result<Vec<(String, String)>> {
    let mut env: Vec<(String, String)> = Vec::new();

    if let Some(ours) = raw.get("ourEnv").and_then(Value::as_object) {
        for (key, value) in ours {
            if is_kept(key) {
                if let Some(v) = value.as_str() {
                    env.push((key.clone(), v.to_string()));
                }
            }
        }
    }
    if let Some(passed) = raw.get("env").and_then(Value::as_object) {
        for (key, value) in passed {
            if let Some(v) = value.as_str() {
                env.retain(|(k, _)| k != key);
                env.push((key.clone(), v.to_string()));
            }
        }
    }

    let encoded = env
        .iter()
        .find(|(k, _)| k == "QUARTO_FILTER_PARAMS")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| anyhow!("captured command has no QUARTO_FILTER_PARAMS"))?;
    let mut params: Value = serde_json::from_slice(
        &STANDARD
            .decode(encoded.as_bytes())
            .context("QUARTO_FILTER_PARAMS is not base64")?,
    )
    .context("QUARTO_FILTER_PARAMS is not JSON")?;
    rewrite_params(&mut params, dir)?;
    let rewritten = STANDARD.encode(serde_json::to_vec(&params)?);

    let deps = dir.join("deps.json");
    std::fs::write(&deps, "")?;

    set(&mut env, "QUARTO_FILTER_PARAMS", rewritten);
    set(&mut env, "QUARTO_FILTER_DEPENDENCY_FILE", path_str(&deps));
    env.sort();
    Ok(env)
}

/// The captured command with `ourEnv` reduced to what [`prepare_env`] keeps.
pub fn scrubbed(raw: &Value) -> Value {
    let mut clean = raw.clone();
    if let Some(ours) = clean.get_mut("ourEnv").and_then(Value::as_object_mut) {
        ours.retain(|key, _| is_kept(key));
    }
    clean
}

/// `QUARTO_*` only, minus the capture switch itself and a pointer into a session
/// directory that no longer exists.
fn is_kept(key: &str) -> bool {
    key.starts_with("QUARTO_")
        && key != "QUARTO_CAPTURE_RENDER_COMMAND"
        && key != "QUARTO_EXECUTE_INFO"
}

fn rewrite_params(params: &mut Value, dir: &Path) -> Result<()> {
    for key in ["include-before-body", "include-after-body", "include-in-header"] {
        if let Some(list) = params.get_mut(key).and_then(Value::as_array_mut) {
            for item in list.iter_mut() {
                if let Some(original) = item.as_str().map(str::to_string) {
                    *item = Value::String(placeholder(dir, &original, "")?);
                }
            }
        }
    }
    if let Some(original) = params
        .get("notebook-context")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        params["notebook-context"] = Value::String(placeholder(dir, &original, "{}")?);
    }
    params["results-file"] = Value::String(path_str(&dir.join("results.json")));
    Ok(())
}

fn placeholder(dir: &Path, original: &str, content: &str) -> Result<String> {
    let name = Path::new(original)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "placeholder".to_string());
    let target = dir.join(format!("ph-{name}"));
    std::fs::write(&target, content)?;
    Ok(path_str(&target))
}

fn set(env: &mut Vec<(String, String)>, key: &str, value: String) {
    env.retain(|(k, _)| k != key);
    env.push((key.to_string(), value));
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn read_flag_file(args: &[String], flag: &str) -> Result<String> {
    let path = args
        .iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .ok_or_else(|| anyhow!("captured command has no {flag}"))?;
    std::fs::read_to_string(path).with_context(|| format!("reading {path}"))
}

fn read_arg_file(args: &[String], matches: impl Fn(&str) -> bool) -> Result<String> {
    let path = args
        .iter()
        .find(|a| matches(a))
        .ok_or_else(|| anyhow!("captured command has no input file"))?;
    std::fs::read_to_string(path).with_context(|| format!("reading {path}"))
}

/// The value of a top-level `key: value` line. Enough for the defaults file.
fn yaml_scalar(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    yaml.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn short_hash(text: &str) -> String {
    // FNV-1a: stable across runs, no dependency needed for a directory name.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        let params = json!({
            "include-before-body": ["C:/gone/session/before.html"],
            "include-after-body": ["C:/gone/session/a.html", "C:/gone/session/b"],
            "notebook-context": "C:/gone/session/ctx.json",
            "results-file": "C:/gone/session/results",
            "active-filters": {"normalization": true, "crossref": true}
        });
        json!({
            "cmd": "pandoc",
            "args": ["--defaults", "d.yml", "quarto-input1.md", "--metadata-file", "m.yml"],
            "cwd": ".",
            "env": {
                "LUA_CPATH": "",
                "QUARTO_FILTER_PARAMS": STANDARD.encode(params.to_string())
            },
            "ourEnv": {
                "QUARTO_SHARE_PATH": "C:/Program Files/Quarto/share",
                "QUARTO_CAPTURE_RENDER_COMMAND": "C:/somewhere",
                "QUARTO_EXECUTE_INFO": "C:/gone/session/info",
                "CLAUDE_CODE_MESSAGING_TOKEN": "secret",
                "PATH": "C:/Windows"
            }
        })
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("qlive-capture-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn decoded_params(env: &[(String, String)]) -> Value {
        let encoded = &env.iter().find(|(k, _)| k == "QUARTO_FILTER_PARAMS").unwrap().1;
        serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap()
    }

    #[test]
    fn keeps_only_quarto_variables() {
        let env = prepare_env(&sample(), &scratch("keeps")).unwrap();
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"QUARTO_SHARE_PATH"));
        assert!(keys.contains(&"LUA_CPATH"));
        assert!(!keys.contains(&"CLAUDE_CODE_MESSAGING_TOKEN"), "secrets must not survive");
        assert!(!keys.contains(&"PATH"));
        assert!(!keys.contains(&"QUARTO_CAPTURE_RENDER_COMMAND"));
        assert!(!keys.contains(&"QUARTO_EXECUTE_INFO"));
    }

    #[test]
    fn session_files_become_existing_placeholders() {
        let dir = scratch("placeholders");
        let params = decoded_params(&prepare_env(&sample(), &dir).unwrap());
        for key in ["include-before-body", "include-after-body"] {
            for item in params[key].as_array().unwrap() {
                let path = Path::new(item.as_str().unwrap());
                assert!(path.starts_with(&dir), "{key} not rebased: {}", path.display());
                assert!(path.exists(), "{key} placeholder missing");
            }
        }
        let context = Path::new(params["notebook-context"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(context).unwrap(), "{}");
        assert!(Path::new(params["results-file"].as_str().unwrap()).starts_with(&dir));
    }

    #[test]
    fn other_params_pass_through_untouched() {
        let params = decoded_params(&prepare_env(&sample(), &scratch("untouched")).unwrap());
        assert_eq!(params["active-filters"]["crossref"], true);
    }

    #[test]
    fn dependency_file_is_created() {
        let dir = scratch("deps");
        let env = prepare_env(&sample(), &dir).unwrap();
        let deps = &env.iter().find(|(k, _)| k == "QUARTO_FILTER_DEPENDENCY_FILE").unwrap().1;
        assert!(Path::new(deps).exists());
    }

    #[test]
    fn scrubbing_removes_secrets_from_the_stored_command() {
        let clean = scrubbed(&sample());
        let ours = clean["ourEnv"].as_object().unwrap();
        assert!(ours.contains_key("QUARTO_SHARE_PATH"));
        assert!(!ours.contains_key("CLAUDE_CODE_MESSAGING_TOKEN"));
        assert!(!ours.contains_key("PATH"));
    }

    #[test]
    fn reads_the_output_format_from_defaults() {
        assert_eq!(yaml_scalar("standalone: true\nto: html\n", "to").as_deref(), Some("html"));
        assert_eq!(yaml_scalar("to: typst", "to").as_deref(), Some("typst"));
        assert_eq!(yaml_scalar("from: x", "to"), None);
    }
}
