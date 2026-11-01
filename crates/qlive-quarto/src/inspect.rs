//! The kernel setup Quarto would run, with the values Quarto would use.
//!
//! Before a document's first cell, Quarto runs `share/jupyter/lang/python/setup.py`
//! in the kernel — a template it fills with `str.format` from the resolved format
//! options (`notebook.py`: `build_kernel_options`, `nb_language_cell`), with
//! `store_history=False` so the document's cells still count from 1. It also sets
//! `QUARTO_FIG_*` environment variables (`set_env_vars`). Running the same template
//! with the same values is what makes our figures identical to Quarto's — a
//! different figure size would change every plot and its markdown.
//!
//! The values come from `quarto inspect`, which prints the resolved options, rather
//! than from Quarto's defaults as remembered by anyone.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What `setup.py`'s `str.format` and the kernel's environment need.
#[derive(Debug, Clone)]
pub struct KernelSetup {
    /// The template, `<quarto>/share/jupyter/lang/python/setup.py`.
    pub script: PathBuf,
    /// Keyword arguments for `str.format`. `run_path` is plain here; the worker
    /// base64-encodes it, as `nb_language_cell` does.
    pub args: Value,
    /// `QUARTO_FIG_*`, as `set_env_vars` sets them.
    pub env: Value,
}

/// Resolve the setup for `doc` rendered to `format` (e.g. `html`).
pub fn kernel_setup(quarto: &Path, doc: &Path, format: &str) -> Result<KernelSetup> {
    let doc = qlive_core::absolute(doc)?;
    let out = Command::new(quarto)
        .arg("inspect")
        .arg(&doc)
        .output()
        .map_err(|e| anyhow!("could not run quarto inspect: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "quarto inspect failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let inspected: Value =
        serde_json::from_slice(&out.stdout).context("quarto inspect did not print JSON")?;

    let run_path = doc
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let (args, env) = from_inspect(&inspected, format, &run_path)?;

    Ok(KernelSetup {
        script: share_dir(quarto)?.join("jupyter/lang/python/setup.py"),
        args,
        env,
    })
}

/// The pure part, so it can be tested on a captured `quarto inspect` payload.
pub fn from_inspect(inspected: &Value, format: &str, run_path: &str) -> Result<(Value, Value)> {
    let formats = inspected
        .get("formats")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("quarto inspect: no formats"))?;
    let resolved = formats
        .get(format)
        .or_else(|| formats.values().next())
        .ok_or_else(|| anyhow!("quarto inspect: format {format} not found"))?;
    let execute = resolved
        .get("execute")
        .ok_or_else(|| anyhow!("quarto inspect: no execute options"))?;

    let get = |key: &str| execute.get(key).cloned().unwrap_or(Value::Null);
    let fig_width = get("fig-width");
    let fig_height = get("fig-height");
    let fig_dpi = get("fig-dpi");
    let fig_format = execute
        .get("fig-format")
        .and_then(Value::as_str)
        .unwrap_or("png")
        .to_string();
    let interactivity = execute
        .get("ipynb-shell-interactivity")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let plotly_connected = execute
        .get("plotly-connected")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let is_shiny = resolved
        .pointer("/metadata/server/type")
        .and_then(Value::as_str)
        == Some("shiny");
    let is_dashboard = resolved
        .pointer("/identifier/base-format")
        .and_then(Value::as_str)
        == Some("dashboard");

    let args = json!({
        "fig_width": fig_width,
        "fig_height": fig_height,
        "fig_format": fig_format,
        "fig_dpi": fig_dpi,
        "interactivity": interactivity,
        "plotly_connected": plotly_connected,
        "is_shiny": is_shiny,
        "is_dashboard": is_dashboard,
        "run_path": run_path,
    });

    // set_env_vars: retina doubles the dpi and renders PNG.
    let dpi = fig_dpi.as_f64().unwrap_or(96.0);
    let (env_dpi, env_format) = if fig_format == "retina" {
        (dpi * 2.0, "png".to_string())
    } else {
        (dpi, fig_format.clone())
    };
    let env = json!({
        "QUARTO_FIG_WIDTH": number_text(&fig_width),
        "QUARTO_FIG_HEIGHT": number_text(&fig_height),
        "QUARTO_FIG_DPI": trim_float(env_dpi),
        "QUARTO_FIG_FORMAT": env_format,
    });

    Ok((args, env))
}

/// `<quarto>/share`, next to the `bin` directory that holds the quarto binary.
pub fn share_dir(quarto: &Path) -> Result<PathBuf> {
    let bin = quarto
        .parent()
        .ok_or_else(|| anyhow!("quarto binary has no parent directory"))?;
    let share = bin
        .parent()
        .ok_or_else(|| anyhow!("quarto bin has no parent directory"))?
        .join("share");
    if share.is_dir() {
        Ok(share)
    } else {
        Err(anyhow!("no share directory at {}", share.display()))
    }
}

/// Python's `str()` of a JSON number: `7` stays `7`, `7.5` stays `7.5`.
fn number_text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        other => other.as_str().unwrap_or("").to_string(),
    }
}

fn trim_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspected(format: &str) -> Value {
        json!({ "formats": { "html": {
            "execute": {
                "fig-width": 7, "fig-height": 5, "fig-format": format, "fig-dpi": 96,
                "ipynb-shell-interactivity": null, "plotly-connected": true
            },
            "identifier": { "base-format": "html" },
            "metadata": {}
        }}})
    }

    #[test]
    fn reads_the_values_quarto_inspect_resolved() {
        let (args, _) = from_inspect(&inspected("retina"), "html", "C:/doc").unwrap();
        assert_eq!(args["fig_width"], 7);
        assert_eq!(args["fig_height"], 5);
        assert_eq!(args["fig_format"], "retina");
        assert_eq!(args["fig_dpi"], 96);
        assert_eq!(args["interactivity"], "");
        assert_eq!(args["is_shiny"], false);
        assert_eq!(args["is_dashboard"], false);
        assert_eq!(args["run_path"], "C:/doc");
    }

    #[test]
    fn retina_doubles_the_dpi_and_renders_png_like_set_env_vars() {
        let (_, env) = from_inspect(&inspected("retina"), "html", "").unwrap();
        assert_eq!(env["QUARTO_FIG_DPI"], "192");
        assert_eq!(env["QUARTO_FIG_FORMAT"], "png");
        assert_eq!(env["QUARTO_FIG_WIDTH"], "7");
    }

    #[test]
    fn other_formats_keep_their_dpi() {
        let (_, env) = from_inspect(&inspected("svg"), "html", "").unwrap();
        assert_eq!(env["QUARTO_FIG_DPI"], "96");
        assert_eq!(env["QUARTO_FIG_FORMAT"], "svg");
    }

    #[test]
    fn missing_format_is_an_error() {
        assert!(from_inspect(&json!({"formats": {}}), "html", "").is_err());
    }
}
