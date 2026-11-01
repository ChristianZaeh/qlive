//! Notebook cells: splitting a document into them, keying them, and deciding which
//! ones actually have to run.
//!
//! The keying rule is what makes a prose edit free: a code cell's key is
//! `hash(its own source + every preceding **code** cell's source)`. Markdown does not
//! participate, so editing a paragraph changes no key at all.
//!
//! The planning rule is what makes it *correct*: the cells that may be reused are
//! exactly the ones forming the common prefix of the document's keys and the keys the
//! kernel has already executed. Anything past that point runs, because the state it
//! would have seen has changed. A cold kernel shares no prefix, so it runs everything.

use crate::blocks::{self, BlockKind};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellKind {
    Markdown,
    Code { language: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub kind: CellKind,
    pub source: String,
    /// Set for code cells only: the cumulative hash described above.
    pub key: Option<String>,
}

impl Cell {
    pub fn is_code(&self) -> bool {
        matches!(self.kind, CellKind::Code { .. })
    }
}

/// What one cell execution produced, as nbformat outputs.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CellOutputs {
    pub outputs: Vec<Value>,
    pub execution_count: Option<i64>,
}

/// Reuse the cached outputs, or run the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reuse,
    Execute,
}

/// Executable cells in source coordinates, shared by editor navigation and execution.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CodeRange {
    /// 0-based opening fence and closing fence lines, inclusive.
    pub start_line: usize,
    pub end_line: usize,
    pub language: String,
}

pub fn code_ranges(source: &str) -> Vec<CodeRange> {
    blocks::split(source).into_iter().filter_map(|block| {
        if block.kind != BlockKind::CodeFence { return None; }
        executable_language(&block.text).map(|language| CodeRange {
            start_line: block.start_line - 1,
            end_line: block.end_line - 1,
            language,
        })
    }).collect()
}

/// Split a Quarto document into notebook cells. Front matter and prose join the
/// markdown cells, exactly as `quarto convert` arranges them.
pub fn from_markdown(source: &str) -> Vec<Cell> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut prose: Vec<String> = Vec::new();

    for block in blocks::split(source) {
        match (&block.kind, executable_language(&block.text)) {
            (BlockKind::CodeFence, Some(language)) => {
                flush(&mut prose, &mut cells);
                cells.push(Cell {
                    kind: CellKind::Code { language },
                    source: fence_body(&block.text),
                    key: None,
                });
            }
            _ => prose.push(block.text),
        }
    }
    flush(&mut prose, &mut cells);

    assign_keys(&mut cells);
    cells
}

/// Give every code cell its cumulative key.
pub fn assign_keys(cells: &mut [Cell]) {
    let mut cumulative = String::new();
    for cell in cells.iter_mut() {
        if cell.is_code() {
            cumulative.push_str(&cell.source);
            cumulative.push('\n');
            cell.key = Some(blake3::hash(cumulative.as_bytes()).to_hex().to_string());
        } else {
            cell.key = None;
        }
    }
}

/// Keys across several documents run in order in one kernel, so that variables
/// carry from one document to the next. A cell's key covers every code cell before
/// it in *any* earlier document as well: editing chapter 1 invalidates chapter 2,
/// whose cells saw different state; editing chapter 2 leaves chapter 1 alone. For a
/// single document this is exactly [`assign_keys`].
pub fn chain_keys(docs: &mut [Vec<Cell>]) {
    let mut cumulative = String::new();
    for cells in docs.iter_mut() {
        for cell in cells.iter_mut() {
            if cell.is_code() {
                cumulative.push_str(&cell.source);
                cumulative.push('\n');
                cell.key = Some(blake3::hash(cumulative.as_bytes()).to_hex().to_string());
            } else {
                cell.key = None;
            }
        }
    }
}

/// The code cells' keys, in document order.
pub fn code_keys(cells: &[Cell]) -> Vec<String> {
    cells
        .iter()
        .filter_map(|cell| cell.key.clone())
        .collect()
}

/// Decide what to do with each code cell, given the keys the kernel has executed.
///
/// `keys` and the returned vector are in document order and the same length.
pub fn plan(keys: &[String], kernel_executed: &[String]) -> Vec<Action> {
    let reusable = keys
        .iter()
        .zip(kernel_executed.iter())
        .take_while(|(wanted, done)| wanted == done)
        .count();

    (0..keys.len())
        .map(|i| {
            if i < reusable {
                Action::Reuse
            } else {
                Action::Execute
            }
        })
        .collect()
}

/// Record that the code cell at `index` just ran in the kernel.
///
/// The history has to stay aligned with cell *positions*, not just accumulate: running
/// cell `i` replaces whatever the kernel previously had at `i` and invalidates
/// everything after it. Appending instead made a re-run cell look permanently dirty,
/// because the history grew longer than the document and the common prefix collapsed.
pub fn record_executed(history: &mut Vec<String>, index: usize, key: &str) {
    history.truncate(index);
    history.push(key.to_string());
}

/// Assemble an nbformat 4.4 notebook. `outputs` is indexed like `cells`; markdown
/// cells ignore it.
pub fn to_notebook(cells: &[Cell], outputs: &[Option<CellOutputs>], kernel_name: &str) -> Value {
    let mut out = Vec::with_capacity(cells.len());

    for (index, cell) in cells.iter().enumerate() {
        match &cell.kind {
            CellKind::Markdown => out.push(json!({
                "cell_type": "markdown",
                "metadata": {},
                "source": source_lines(&cell.source),
            })),
            CellKind::Code { .. } => {
                let result = outputs.get(index).and_then(|o| o.clone()).unwrap_or_default();
                out.push(json!({
                    "cell_type": "code",
                    "execution_count": result.execution_count,
                    "metadata": {},
                    "outputs": result.outputs,
                    "source": source_lines(&cell.source),
                }));
            }
        }
    }

    json!({
        "cells": out,
        "metadata": {
            "kernelspec": {
                "display_name": kernel_name,
                "language": "python",
                "name": kernel_name,
            }
        },
        "nbformat": 4,
        "nbformat_minor": 4,
    })
}

/// ```` ```{python} ```` -> `Some("python")`. A plain fence is not executable.
fn executable_language(fence: &str) -> Option<String> {
    let first = fence.lines().next()?.trim();
    let rest = first
        .strip_prefix("```")
        .or_else(|| first.strip_prefix("~~~"))?
        .trim();
    let inside = rest.strip_prefix('{')?.strip_suffix('}')?;
    let language = inside
        .split([',', ' '])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.')
        .to_string();
    if language.is_empty() {
        None
    } else {
        Some(language)
    }
}

/// Drop the opening and closing fence lines.
fn fence_body(fence: &str) -> String {
    let lines: Vec<&str> = fence.lines().collect();
    if lines.len() <= 2 {
        return String::new();
    }
    let last = lines.len() - 1;
    let closes = lines[last].trim_start().starts_with("```") || lines[last].trim_start().starts_with("~~~");
    let end = if closes { last } else { lines.len() };
    lines[1..end].join("\n")
}

fn flush(prose: &mut Vec<String>, cells: &mut Vec<Cell>) {
    if prose.is_empty() {
        return;
    }
    let source = prose.join("\n\n");
    prose.clear();
    if !source.trim().is_empty() {
        cells.push(Cell {
            kind: CellKind::Markdown,
            source,
            key: None,
        });
    }
}

/// nbformat stores source as a list of lines, each keeping its newline.
fn source_lines(source: &str) -> Vec<String> {
    let mut lines: Vec<String> = source.split('\n').map(|l| format!("{l}\n")).collect();
    if let Some(last) = lines.last_mut() {
        if last == "\n" {
            lines.pop();
        } else {
            *last = last.trim_end_matches('\n').to_string();
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = concat!(
        "---\n",
        "title: Two cells\n",
        "---\n",
        "\n",
        "Intro prose.\n",
        "\n",
        "```{python}\n",
        "x = 21\n",
        "```\n",
        "\n",
        "Middle prose.\n",
        "\n",
        "```{python}\n",
        "y = x * 2\n",
        "print(y)\n",
        "```\n",
        "\n",
        "Trailing prose.\n",
    );

    #[test]
    fn splits_like_quarto_convert() {
        let cells = from_markdown(DOC);
        let kinds: Vec<&CellKind> = cells.iter().map(|c| &c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &CellKind::Markdown,
                &CellKind::Code { language: "python".into() },
                &CellKind::Markdown,
                &CellKind::Code { language: "python".into() },
                &CellKind::Markdown,
            ]
        );
        // Front matter rides along in the first markdown cell, as Quarto does it.
        assert!(cells[0].source.starts_with("---\ntitle: Two cells\n---"));
        assert_eq!(cells[1].source, "x = 21");
        assert_eq!(cells[3].source, "y = x * 2\nprint(y)");
    }

    #[test]
    fn a_plain_fence_is_not_a_cell() {
        let cells = from_markdown("Text\n\n```\nnot executable\n```\n");
        assert!(cells.iter().all(|c| !c.is_code()));
    }

    #[test]
    fn prose_edits_change_no_keys() {
        let before = code_keys(&from_markdown(DOC));
        let edited = DOC.replace("Middle prose.", "Middle prose, rewritten entirely.");
        let after = code_keys(&from_markdown(&edited));
        assert_eq!(before, after, "editing prose must not invalidate any cell");
    }

    #[test]
    fn editing_the_last_cell_changes_only_its_key() {
        let before = code_keys(&from_markdown(DOC));
        let edited = DOC.replace("y = x * 2", "y = x * 3");
        let after = code_keys(&from_markdown(&edited));
        assert_eq!(before[0], after[0], "the untouched first cell keeps its key");
        assert_ne!(before[1], after[1], "the edited cell gets a new key");
    }

    #[test]
    fn editing_the_first_cell_invalidates_the_second() {
        let before = code_keys(&from_markdown(DOC));
        let edited = DOC.replace("x = 21", "x = 22");
        let after = code_keys(&from_markdown(&edited));
        assert_ne!(before[0], after[0]);
        assert_ne!(
            before[1], after[1],
            "the later cell saw different state, so its key must change too"
        );
    }

    #[test]
    fn cold_kernel_executes_everything() {
        let keys = code_keys(&from_markdown(DOC));
        assert_eq!(plan(&keys, &[]), vec![Action::Execute, Action::Execute]);
    }

    #[test]
    fn matching_kernel_reuses_everything() {
        let keys = code_keys(&from_markdown(DOC));
        assert_eq!(plan(&keys, &keys), vec![Action::Reuse, Action::Reuse]);
    }

    #[test]
    fn last_cell_edit_executes_only_the_last_cell() {
        let live = code_keys(&from_markdown(DOC));
        let edited = code_keys(&from_markdown(&DOC.replace("y = x * 2", "y = x * 3")));
        assert_eq!(plan(&edited, &live), vec![Action::Reuse, Action::Execute]);
    }

    #[test]
    fn first_cell_edit_executes_both() {
        let live = code_keys(&from_markdown(DOC));
        let edited = code_keys(&from_markdown(&DOC.replace("x = 21", "x = 22")));
        assert_eq!(plan(&edited, &live), vec![Action::Execute, Action::Execute]);
    }

    #[test]
    fn rerunning_a_cell_replaces_its_history_entry() {
        let live = code_keys(&from_markdown(DOC));
        let edited = code_keys(&from_markdown(&DOC.replace("y = x * 2", "y = x * 3")));

        // The kernel has run both cells as they originally were.
        let mut history = live.clone();
        // Now cell 1 is edited and re-runs.
        record_executed(&mut history, 1, &edited[1]);

        assert_eq!(history.len(), 2, "history must not grow past the cell count");
        assert_eq!(history, edited);
        // And the very next pass, with nothing changed, must execute nothing.
        assert_eq!(plan(&edited, &history), vec![Action::Reuse, Action::Reuse]);
    }

    #[test]
    fn rerunning_an_early_cell_discards_the_later_history() {
        let live = code_keys(&from_markdown(DOC));
        let mut history = live.clone();
        record_executed(&mut history, 0, "a-new-key");
        assert_eq!(history, vec!["a-new-key".to_string()]);
    }

    const CHAPTER_2: &str = "---\ntitle: Two\n---\n\n```{python}\nz = y + 1\n```\n";

    fn chained(first: &str, second: &str) -> (Vec<String>, Vec<String>) {
        let mut docs = vec![from_markdown(first), from_markdown(second)];
        chain_keys(&mut docs);
        (code_keys(&docs[0]), code_keys(&docs[1]))
    }

    #[test]
    fn a_single_document_chain_matches_assign_keys() {
        let mut docs = vec![from_markdown(DOC)];
        chain_keys(&mut docs);
        assert_eq!(code_keys(&docs[0]), code_keys(&from_markdown(DOC)));
    }

    #[test]
    fn editing_an_earlier_document_invalidates_the_later_one() {
        let (a1, b1) = chained(DOC, CHAPTER_2);
        let (a2, b2) = chained(&DOC.replace("x = 21", "x = 22"), CHAPTER_2);
        assert_ne!(a1, a2);
        assert_ne!(b1, b2, "chapter 2 saw different state, so its key must change");
    }

    #[test]
    fn editing_a_later_document_leaves_the_earlier_one_alone() {
        let (a1, b1) = chained(DOC, CHAPTER_2);
        let (a2, b2) = chained(DOC, &CHAPTER_2.replace("y + 1", "y + 2"));
        assert_eq!(a1, a2, "chapter 1 must stay cached");
        assert_ne!(b1, b2);
    }

    #[test]
    fn chain_plan_runs_only_the_edited_later_cell() {
        let (a, b) = chained(DOC, CHAPTER_2);
        let live: Vec<String> = a.iter().chain(b.iter()).cloned().collect();
        let (a2, b2) = chained(DOC, &CHAPTER_2.replace("y + 1", "y + 2"));
        let edited: Vec<String> = a2.iter().chain(b2.iter()).cloned().collect();
        assert_eq!(
            plan(&edited, &live),
            vec![Action::Reuse, Action::Reuse, Action::Execute]
        );
    }

    #[test]
    fn notebook_has_the_expected_shape() {
        let cells = from_markdown(DOC);
        let mut outputs = vec![None; cells.len()];
        outputs[3] = Some(CellOutputs {
            outputs: vec![json!({"output_type": "stream", "name": "stdout", "text": "42\n"})],
            execution_count: Some(2),
        });

        let nb = to_notebook(&cells, &outputs, "python3");
        assert_eq!(nb["nbformat"], 4);
        assert_eq!(nb["nbformat_minor"], 4);
        assert_eq!(nb["cells"].as_array().unwrap().len(), 5);
        assert_eq!(nb["cells"][1]["cell_type"], "code");
        assert_eq!(nb["cells"][3]["outputs"][0]["text"], "42\n");
        assert_eq!(nb["cells"][3]["execution_count"], 2);
        assert_eq!(nb["metadata"]["kernelspec"]["name"], "python3");
    }

    #[test]
    fn source_lines_keep_newlines_but_not_a_trailing_one() {
        assert_eq!(source_lines("a\nb"), vec!["a\n".to_string(), "b".to_string()]);
        assert_eq!(source_lines("solo"), vec!["solo".to_string()]);
    }
}
