//! What Quarto's Jupyter engine hands pandoc, generated from our own cell outputs.
//!
//! Quarto replaces every executed cell with a fenced div before pandoc sees the text.
//! The warm path has to produce exactly that markdown, so this module reproduces the
//! format captured from a real render (`QUARTO_CAPTURE_RENDER_COMMAND`), byte for byte:
//!
//! ~~~text
//! ::: {#23dc591a .cell execution_count=2}
//! ``` {.python .cell-code}
//! print("hello", x)
//! ```
//!
//! ::: {.cell-output .cell-output-stdout}
//! ```
//! hello 21
//! ```
//! :::
//! :::
//! ~~~
//!
//! Each cell, markdown or code, is followed by a blank line pair; front matter is cut
//! from the first markdown cell, leaving the newline that ended it. Figures are written
//! to `<stem>_files/figure-html/cell-<N>-output-<M>.png`, where `N` counts *all*
//! notebook cells from 1, and their size comes from the output's metadata — IPython's
//! retina figures carry half their pixel size there.
//!
//! Only the common output kinds are covered. Anything this gets wrong is caught by the
//! live preview's self-check, which compares this output with Quarto's own and falls
//! back to cold renders on any difference.

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use qlive_core::cells::{Cell, CellKind, CellOutputs};
use qlive_core::frontmatter;
use serde_json::Value;
use std::path::Path;

/// Cell options Quarto reads from leading `#| key: value` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellOptions {
    pub echo: bool,
    pub output: bool,
    pub eval: bool,
    pub include: bool,
}

impl Default for CellOptions {
    fn default() -> Self {
        Self {
            echo: true,
            output: true,
            eval: true,
            include: true,
        }
    }
}

/// Parse leading `#|` option lines; unknown options are ignored.
pub fn cell_options(source: &str) -> CellOptions {
    let mut options = CellOptions::default();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("#|") else {
            break;
        };
        let Some((key, value)) = rest.split_once(':') else {
            continue;
        };
        let flag = !matches!(value.trim(), "false" | "no" | "False");
        match key.trim() {
            "echo" => options.echo = flag,
            "output" => options.output = flag,
            "eval" => options.eval = flag,
            "include" => options.include = flag,
            _ => {}
        }
    }
    options
}

/// The code without its leading `#|` option lines, as Quarto displays it.
pub fn display_code(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .take_while(|l| l.trim_start().starts_with("#|"))
        .count();
    lines[start..].join("\n")
}

/// Generate the engine markdown. `outputs` is indexed like `cells`. `stem` names the
/// figure directory (`<stem>_files`) and `out_dir` is where it is created. Figures
/// are written only when `write_figures` is set — cells reused from cache already
/// have theirs on disk.
pub fn to_markdown(
    cells: &[Cell],
    outputs: &[Option<CellOutputs>],
    stem: &str,
    out_dir: &Path,
    write_figures: &[bool],
) -> Result<String> {
    let mut out = String::new();
    let mut seen_markdown = false;

    for (index, cell) in cells.iter().enumerate() {
        match &cell.kind {
            CellKind::Markdown => {
                if !seen_markdown {
                    seen_markdown = true;
                    out.push_str(&strip_front_matter(&cell.source));
                } else {
                    out.push_str(&cell.source);
                }
                out.push_str("\n\n");
            }
            CellKind::Code { language } => {
                let options = cell_options(&cell.source);
                if !options.include {
                    continue;
                }
                let result = outputs.get(index).and_then(|o| o.as_ref());
                let write = write_figures.get(index).copied().unwrap_or(true);
                out.push_str(&code_cell(
                    cell, language, options, result, index + 1, stem, out_dir, write,
                )?);
                out.push_str("\n\n");
            }
        }
    }
    Ok(out)
}

/// Cut the YAML block and keep exactly what followed it, starting with the newline
/// that ended the closing fence — if there was one. A markdown cell that is *only*
/// front matter (a code cell right after it) has no such newline and leaves nothing;
/// emitting one anyway was caught by the live self-check against Quarto.
fn strip_front_matter(source: &str) -> String {
    match frontmatter::split(source) {
        (Some(_), body) => {
            let consumed = &source[..source.len() - body.len()];
            if consumed.ends_with('\n') {
                format!("\n{body}")
            } else {
                body.to_string()
            }
        }
        (None, body) => body.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn code_cell(
    cell: &Cell,
    language: &str,
    options: CellOptions,
    result: Option<&CellOutputs>,
    notebook_index: usize,
    stem: &str,
    out_dir: &Path,
    write_figures: bool,
) -> Result<String> {
    let id = cell_id(cell);
    let mut out = match result.and_then(|r| r.execution_count) {
        Some(n) => format!("::: {{#{id} .cell execution_count={n}}}\n"),
        None => format!("::: {{#{id} .cell}}\n"),
    };

    if options.echo {
        out.push_str(&format!(
            "``` {{.{language} .cell-code}}\n{}\n```\n",
            display_code(&cell.source)
        ));
    }

    if options.output {
        if let Some(result) = result {
            for (i, output) in result.outputs.iter().enumerate() {
                let figure = format!("cell-{notebook_index}-output-{}", i + 1);
                if let Some(block) = output_block(output, stem, &figure, out_dir, write_figures)? {
                    out.push('\n');
                    out.push_str(&block);
                }
            }
        }
    }

    out.push_str(":::\n");
    Ok(out)
}

/// A stable id per cell, from its key, so an unchanged cell keeps its DOM id across
/// saves. Quarto's own ids are random.
pub fn cell_id(cell: &Cell) -> String {
    cell.key
        .as_deref()
        .map(|k| k.chars().take(8).collect())
        .unwrap_or_else(|| "00000000".to_string())
}

fn output_block(
    output: &Value,
    stem: &str,
    figure: &str,
    out_dir: &Path,
    write: bool,
) -> Result<Option<String>> {
    let kind = output.get("output_type").and_then(Value::as_str).unwrap_or("");
    Ok(match kind {
        "stream" => {
            let name = output.get("name").and_then(Value::as_str).unwrap_or("stdout");
            let text = text_of(output.get("text"));
            Some(format!(
                "::: {{.cell-output .cell-output-{name}}}\n```\n{}\n```\n:::\n",
                text.trim_end_matches('\n')
            ))
        }
        "execute_result" => {
            let count = output
                .get("execution_count")
                .and_then(Value::as_i64)
                .map(|n| format!(" execution_count={n}"))
                .unwrap_or_default();
            display(output, stem, figure, out_dir, write)?.map(|content| {
                format!("::: {{.cell-output .cell-output-display{count}}}\n{content}:::\n")
            })
        }
        "display_data" => display(output, stem, figure, out_dir, write)?
            .map(|content| format!("::: {{.cell-output .cell-output-display}}\n{content}:::\n")),
        "error" => {
            let name = output.get("ename").and_then(Value::as_str).unwrap_or("Error");
            let value = output.get("evalue").and_then(Value::as_str).unwrap_or("");
            Some(format!(
                "::: {{.cell-output .cell-output-error}}\n```\n{name}: {value}\n```\n:::\n"
            ))
        }
        _ => None,
    })
}

/// The richest representation we know how to emit, in Quarto's order of preference
/// for HTML.
fn display(
    output: &Value,
    stem: &str,
    figure: &str,
    out_dir: &Path,
    write: bool,
) -> Result<Option<String>> {
    let Some(data) = output.get("data") else {
        return Ok(None);
    };

    if let Some(png) = data.get("image/png") {
        let bytes = STANDARD
            .decode(text_of(Some(png)).replace(['\n', '\r'], ""))
            .context("image/png output is not base64")?;
        let relative = format!("{stem}_files/figure-html/{figure}.png");
        if write {
            let path = out_dir.join(&relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, &bytes)?;
        }
        let (width, height) = figure_size(output, &bytes);
        return Ok(Some(format!("![]({relative}){{width={width} height={height}}}\n")));
    }
    if let Some(markdown) = data.get("text/markdown") {
        return Ok(Some(format!("{}\n", text_of(Some(markdown)))));
    }
    if let Some(html) = data.get("text/html") {
        return Ok(Some(format!("```{{=html}}\n{}\n```\n", text_of(Some(html)))));
    }
    if let Some(plain) = data.get("text/plain") {
        return Ok(Some(format!("```\n{}\n```\n", text_of(Some(plain)))));
    }
    Ok(None)
}

/// From the output metadata when present (IPython's retina figures report half their
/// pixel size there), else from the PNG header.
fn figure_size(output: &Value, bytes: &[u8]) -> (u64, u64) {
    let meta = output.get("metadata").and_then(|m| m.get("image/png"));
    let width = meta.and_then(|m| m.get("width")).and_then(Value::as_u64);
    let height = meta.and_then(|m| m.get("height")).and_then(Value::as_u64);
    match (width, height) {
        (Some(w), Some(h)) => (w, h),
        _ => png_size(bytes).unwrap_or((0, 0)),
    }
}

fn png_size(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((u64::from(width), u64::from(height)))
}

/// nbformat allows a string or a list of strings.
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// Replace cell ids with a placeholder, for comparing against Quarto's random ones.
pub fn without_cell_ids(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(at) = rest.find("::: {#") {
        let (before, after) = rest.split_at(at + "::: {#".len());
        out.push_str(before);
        let id_len = after.find(' ').unwrap_or(0);
        out.push_str("ID");
        rest = &after[id_len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use qlive_core::cells::from_markdown;
    use serde_json::json;

    /// The document whose real `quarto render` produced `QUARTO_ENGINE_OUTPUT`.
    const DOC: &str = "---\ntitle: Cells live\nformat: html\n---\n\nIntro prose.\n\n\
```{python}\nx = 21\n```\n\n\
```{python}\nprint(\"hello\", x)\n```\n\n\
```{python}\nx * 2\n```\n\n\
```{python}\nimport matplotlib.pyplot as plt\nfig, ax = plt.subplots()\nax.plot([1, 2, 3])\nplt.show()\n```\n\n\
```{python}\n#| echo: false\nprint(\"code hidden\")\n```\n\n\
Closing prose.\n";

    /// Captured byte for byte from Quarto 1.8.27's jupyter engine for `DOC`.
    const QUARTO_ENGINE_OUTPUT: &str = "\n\nIntro prose.\n\n::: {#23dc591a .cell execution_count=1}\n``` {.python .cell-code}\nx = 21\n```\n:::\n\n\n::: {#3f0dd5f1 .cell execution_count=2}\n``` {.python .cell-code}\nprint(\"hello\", x)\n```\n\n::: {.cell-output .cell-output-stdout}\n```\nhello 21\n```\n:::\n:::\n\n\n::: {#4230a48b .cell execution_count=3}\n``` {.python .cell-code}\nx * 2\n```\n\n::: {.cell-output .cell-output-display execution_count=3}\n```\n42\n```\n:::\n:::\n\n\n::: {#af735d2b .cell execution_count=4}\n``` {.python .cell-code}\nimport matplotlib.pyplot as plt\nfig, ax = plt.subplots()\nax.plot([1, 2, 3])\nplt.show()\n```\n\n::: {.cell-output .cell-output-display}\n![](cells_files/figure-html/cell-5-output-1.png){width=579 height=411}\n:::\n:::\n\n\n::: {#acbce7b5 .cell execution_count=5}\n\n::: {.cell-output .cell-output-stdout}\n```\ncode hidden\n```\n:::\n:::\n\n\nClosing prose.\n\n";

    fn fake_png(width: u32, height: u32) -> String {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        STANDARD.encode(bytes)
    }

    /// The outputs a kernel produces for `DOC`, in nbformat shape.
    fn outputs_for(cells: &[Cell]) -> Vec<Option<CellOutputs>> {
        let per_code = vec![
            CellOutputs { outputs: vec![], execution_count: Some(1) },
            CellOutputs {
                outputs: vec![json!({"output_type": "stream", "name": "stdout", "text": "hello 21\n"})],
                execution_count: Some(2),
            },
            CellOutputs {
                outputs: vec![json!({"output_type": "execute_result", "execution_count": 3,
                                     "data": {"text/plain": "42"}, "metadata": {}})],
                execution_count: Some(3),
            },
            CellOutputs {
                outputs: vec![json!({"output_type": "display_data",
                                     "data": {"image/png": fake_png(1158, 822), "text/plain": "<Figure>"},
                                     "metadata": {"image/png": {"width": 579, "height": 411}}})],
                execution_count: Some(4),
            },
            CellOutputs {
                outputs: vec![json!({"output_type": "stream", "name": "stdout", "text": "code hidden\n"})],
                execution_count: Some(5),
            },
        ];
        let mut code = per_code.into_iter();
        cells.iter().map(|c| if c.is_code() { code.next() } else { None }).collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("qlive-engine-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reproduces_quartos_engine_output_byte_for_byte() {
        let cells = from_markdown(DOC);
        let outputs = outputs_for(&cells);
        let writes = vec![true; cells.len()];
        let ours = to_markdown(&cells, &outputs, "cells", &scratch("bytes"), &writes).unwrap();
        assert_eq!(
            without_cell_ids(&ours),
            without_cell_ids(QUARTO_ENGINE_OUTPUT),
            "\n--- ours ---\n{ours}\n--- quarto ---\n{QUARTO_ENGINE_OUTPUT}"
        );
    }

    #[test]
    fn front_matter_followed_directly_by_a_cell_leaves_no_extra_newline() {
        // Captured from Quarto 1.8.27: the engine output starts "\n\n::: {#… .cell".
        let doc = "---\ntitle: T\nformat: html\n---\n\n```{python}\nx = 1\n```\n";
        let cells = from_markdown(doc);
        let outputs: Vec<Option<CellOutputs>> = cells
            .iter()
            .map(|c| c.is_code().then(|| CellOutputs { outputs: vec![], execution_count: Some(2) }))
            .collect();
        let writes = vec![false; cells.len()];
        let ours = to_markdown(&cells, &outputs, "t", &scratch("fm-cell"), &writes).unwrap();
        assert!(
            without_cell_ids(&ours).starts_with("\n\n::: {#ID .cell execution_count=2}\n"),
            "{ours:?}"
        );
    }

    #[test]
    fn writes_figures_where_the_markdown_points() {
        let dir = scratch("figures");
        let cells = from_markdown(DOC);
        let writes = vec![true; cells.len()];
        to_markdown(&cells, &outputs_for(&cells), "cells", &dir, &writes).unwrap();
        let figure = dir.join("cells_files/figure-html/cell-5-output-1.png");
        assert!(figure.exists());
        assert_eq!(png_size(&std::fs::read(figure).unwrap()), Some((1158, 822)));
    }

    #[test]
    fn reused_cells_do_not_rewrite_their_figures() {
        let dir = scratch("reuse");
        let cells = from_markdown(DOC);
        let writes = vec![false; cells.len()];
        to_markdown(&cells, &outputs_for(&cells), "cells", &dir, &writes).unwrap();
        assert!(!dir.join("cells_files").exists());
    }

    #[test]
    fn falls_back_to_png_header_size_without_metadata() {
        let output = json!({"metadata": {}});
        let bytes = STANDARD.decode(fake_png(640, 480)).unwrap();
        assert_eq!(figure_size(&output, &bytes), (640, 480));
    }

    #[test]
    fn reads_cell_options() {
        let opts = cell_options("#| echo: false\n#| output: true\nprint(1)");
        assert!(!opts.echo && opts.output && opts.eval && opts.include);
        assert_eq!(cell_options("print(1)"), CellOptions::default());
        // Options only count at the top of the cell.
        assert!(cell_options("x = 1\n#| echo: false").echo);
    }

    #[test]
    fn option_lines_are_not_displayed() {
        assert_eq!(display_code("#| label: fig-a\n#| echo: true\nplot()"), "plot()");
    }

    #[test]
    fn ids_are_stable_per_cell_and_comparable_modulo_ids() {
        let a = from_markdown(DOC);
        let b = from_markdown(&DOC.replace("Intro prose.", "Rewritten intro."));
        let ids = |cells: &[Cell]| cells.iter().filter(|c| c.is_code()).map(cell_id).collect::<Vec<_>>();
        assert_eq!(ids(&a), ids(&b), "prose edits must not change cell ids");
        assert_eq!(without_cell_ids("::: {#abc .cell x}"), "::: {#ID .cell x}");
    }
}
