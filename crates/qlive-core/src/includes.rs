//! `{{< include >}}`, expanded exactly as Quarto expands it.
//!
//! A master document that includes its chapters fixes their order, and — because
//! Quarto inlines them into **one document** — they share one kernel natively:
//! `quarto render master.qmd` works on its own, unlike a chain of separate documents.
//! qlive has to see the same text Quarto feeds the engine, so this follows Quarto's
//! implementation (`processMarkdownIncludes`, `standaloneInclude` in quarto-cli)
//! rather than the documentation:
//!
//! * Only **block** includes count: a line that is nothing but `{{< include f >}}`.
//! * In the master, only markdown is scanned; lines inside executable code cells are
//!   left alone. The master's include line is replaced *with* its newline.
//! * In an included file, every line is scanned, nested includes are expanded
//!   recursively, and the nested include line is cut *without* its newline.
//! * Every included file's content is followed by `\n` if it ends with a newline,
//!   otherwise by `\n\n`.
//! * **Every path resolves against the master document's directory** — nested ones
//!   too, not against the file containing them. A leading `/` means the project root
//!   (the master's directory for a single file).
//! * Circular includes and missing files are errors.
//! * An included file's front matter is not stripped; pandoc merges it into the
//!   metadata, as it would under Quarto.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// The expanded text, and every file it was built from — the master first, then the
/// included files in the order they were first included. Watch all of them.
#[derive(Debug, Clone)]
pub struct Expanded {
    pub text: String,
    pub files: Vec<PathBuf>,
    pub spans: Vec<SourceSpan>,
    pub edges: Vec<IncludeEdge>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IncludeEdge { pub parent: PathBuf, pub file: PathBuf }

#[derive(Debug, Clone)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
    pub file: PathBuf,
    /// 0-based source line at the span's beginning.
    pub line: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
}

impl Expanded {
    pub fn location_at_line(&self, line: usize) -> Option<SourceLocation> {
        let byte: usize = self.text.split_inclusive('\n').take(line).map(str::len).sum();
        let span = self.spans.iter().find(|s| s.start <= byte && byte < s.end)?;
        let offset = self.text[span.start..byte].bytes().filter(|b| *b == b'\n').count();
        Some(SourceLocation { file: span.file.clone(), line: span.line + offset })
    }
}

/// Expand every include in `source`, the text of the document at `master`.
pub fn expand(source: &str, master: &Path) -> Result<Expanded> {
    expand_with(source, master, |path| Ok(std::fs::read_to_string(path)?))
}

/// Use editor buffers for included files without writing them to disk.
pub fn expand_with<F: FnMut(&Path) -> Result<String>>(source: &str, master: &Path, read: F) -> Result<Expanded> {
    let base = master
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", master.display()))?
        .to_path_buf();
    let root = project_root(master).unwrap_or_else(|| base.clone());
    let mut ctx = Expansion {
        base,
        root,
        stack: vec![normalise(master)],
        files: vec![normalise(master)],
        spans: Vec::new(),
        master: normalise(master),
        read,
        edges: Vec::new(),
    };

    let mut out = String::with_capacity(source.len());
    let mut segment: Vec<(usize, &str)> = Vec::new();
    let mut fence: Option<String> = None;

    for (line_number, line) in lines_keeping_newlines(source).into_iter().enumerate() {
        match &fence {
            Some(marker) => {
                ctx.append(&mut out, line, master, line_number);
                if line.trim_start().starts_with(marker.as_str()) && line.trim() == marker.trim() {
                    fence = None;
                }
            }
            None => {
                if let Some(marker) = executable_fence(line) {
                    ctx.flush(&mut segment, &mut out)?;
                    ctx.append(&mut out, line, master, line_number);
                    fence = Some(marker);
                } else {
                    segment.push((line_number, line));
                }
            }
        }
    }
    ctx.flush(&mut segment, &mut out)?;

    Ok(Expanded {
        text: out,
        files: ctx.files,
        spans: ctx.spans,
        edges: ctx.edges,
    })
}

struct Expansion<F> {
    base: PathBuf,
    root: PathBuf,
    /// Files being expanded right now, for circularity.
    stack: Vec<PathBuf>,
    files: Vec<PathBuf>,
    spans: Vec<SourceSpan>,
    master: PathBuf,
    read: F,
    edges: Vec<IncludeEdge>,
}

impl<F: FnMut(&Path) -> Result<String>> Expansion<F> {
    fn append(&mut self, out: &mut String, text: &str, file: &Path, line: usize) {
        if text.is_empty() { return; }
        let start = out.len();
        out.push_str(text);
        self.spans.push(SourceSpan { start, end: out.len(), file: file.to_path_buf(), line });
    }
    /// Expand includes in a run of markdown lines from the master.
    fn flush(&mut self, segment: &mut Vec<(usize, &str)>, out: &mut String) -> Result<()> {
        // Quarto skips a whole markdown cell that disables shortcodes.
        let disabled = segment
            .iter()
            .any(|(_, l)| l.trim_start().starts_with("```") && l.replace(' ', "").contains("{shortcodes=false}"));
        for (line_number, line) in segment.drain(..) {
            match (disabled, block_include(line)) {
                (false, Some(target)) => self.retrieve(&target, out)?,
                _ => self.append(out, line, &self.master.clone(), line_number),
            }
        }
        Ok(())
    }

    fn retrieve(&mut self, target: &str, out: &mut String) -> Result<()> {
        let path = self.resolve(target);
        if self.stack.contains(&path) {
            return Err(anyhow!("circular include of {target}"));
        }
        self.edges.push(IncludeEdge { parent: self.stack.last().unwrap().clone(), file: path.clone() });
        let source = (self.read)(&path)
            .with_context(|| format!("include {target}: could not read {}", path.display()))?;
        if !self.files.contains(&path) {
            self.files.push(path.clone());
        }
        self.stack.push(path.clone());

        let mut start = 0;
        for (line_start, line_end) in line_ranges(&source) {
            if let Some(nested) = block_include(&source[line_start..line_end]) {
                self.append(out, &source[start..line_start], &path, source[..start].bytes().filter(|b| *b == b'\n').count());
                start = line_end;
                self.retrieve(&nested, out)?;
            }
        }
        if start != source.len() {
            self.append(out, &source[start..], &path, source[..start].bytes().filter(|b| *b == b'\n').count());
        }
        self.append(out, if source.ends_with('\n') { "\n" } else { "\n\n" }, &path, source.lines().count().saturating_sub(1));

        self.stack.pop();
        Ok(())
    }

    fn resolve(&self, target: &str) -> PathBuf {
        let joined = match target.strip_prefix('/') {
            Some(rest) => self.root.join(rest),
            None => self.base.join(target),
        };
        normalise(&joined)
    }
}

/// `{{< include file >}}` alone on a line -> `Some("file")`. Mirrors Quarto's
/// `isBlockShortcode`: exactly `{{< ` and ` >}}`, and the `{{</* */>}}` escape is not
/// a shortcode.
pub fn block_include(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("{{< ")?.strip_suffix(" >}}")?;
    if inner.starts_with("/*") || inner.ends_with("*/") {
        return None;
    }
    let inner = inner.trim();
    let (name, rest) = inner.split_once(char::is_whitespace)?;
    if !name.eq_ignore_ascii_case("include") {
        return None;
    }
    let rest = rest.trim();
    let target = match rest.chars().next()? {
        quote @ ('"' | '\'') => rest[1..].split(quote).next()?.to_string(),
        _ => rest.split_whitespace().next()?.to_string(),
    };
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

/// The opening fence of an executable cell (```` ```{python} ````) -> its marker.
fn executable_fence(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let marker: String = trimmed.chars().take_while(|c| *c == '`' || *c == '~').collect();
    if marker.len() < 3 {
        return None;
    }
    let rest = trimmed[marker.len()..].trim_start();
    if rest.starts_with('{') && !rest.starts_with("{.") && !rest.starts_with("{=") {
        Some(marker)
    } else {
        None
    }
}

/// Lines with their line endings, as Quarto's `mappedLines(…, true)` gives them.
fn lines_keeping_newlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, _) in text.match_indices('\n') {
        lines.push(&text[start..=i]);
        start = i + 1;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Byte ranges of lines *without* their `\r?\n`, as Quarto's `rangedLines` gives
/// them.
fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (i, _) in text.match_indices('\n') {
        let end = if i > start && text.as_bytes()[i - 1] == b'\r' { i - 1 } else { i };
        ranges.push((start, end));
        start = i + 1;
    }
    ranges.push((start, text.len()));
    ranges
}

/// The nearest directory above `doc` holding `_quarto.yml`.
fn project_root(doc: &Path) -> Option<PathBuf> {
    doc.ancestors()
        .skip(1)
        .find(|dir| dir.join("_quarto.yml").is_file() || dir.join("_quarto.yaml").is_file())
        .map(Path::to_path_buf)
}

fn normalise(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    crate::paths::strip_verbatim(&absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("qlive-include-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn replaces_the_include_line_with_the_file_and_one_newline() {
        let dir = scratch("basic");
        write(&dir, "ch1.qmd", "Chapter one.\n");
        let master = write(&dir, "master.qmd", "Intro.\n\n{{< include ch1.qmd >}}\n\nOutro.\n");
        let out = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap();
        // The include line, newline and all, becomes the file plus "\n".
        assert_eq!(out.text, "Intro.\n\nChapter one.\n\n\nOutro.\n");
    }

    #[test]
    fn maps_nested_cells_to_their_actual_source_lines_and_keeps_edges() {
        let dir = scratch("source-map");
        write(&dir, "chapters/one.qmd", "## One\n\n{{< include two.qmd >}}\n");
        let two = write(&dir, "two.qmd", "Nested prose.\n\n```{python}\nvalue = 42\n```\n");
        let source = "Master.\n\n{{< include chapters/one.qmd >}}\n\nEnd.\n";
        let master = write(&dir, "master.qmd", source);
        let out = expand(source, &master).unwrap();
        let cell = crate::cells::code_ranges(&out.text).remove(0);
        let location = out.location_at_line(cell.start_line).unwrap();
        assert_eq!(location.file, two);
        assert_eq!(location.line, 2);
        assert_eq!(out.edges[1].parent, dir.join("chapters/one.qmd"));
        assert_eq!(out.edges[1].file, two);
        let overlay = expand_with(source, &master, |file| {
            if file == two { Ok("Changed unsaved text.\n".into()) } else { Ok(std::fs::read_to_string(file)?) }
        }).unwrap();
        assert!(overlay.text.contains("Changed unsaved text."));
        assert!(std::fs::read_to_string(two).unwrap().contains("value = 42"));
    }

    #[test]
    fn a_file_without_a_final_newline_gets_two() {
        let dir = scratch("no-newline");
        write(&dir, "ch1.qmd", "No final newline");
        let master = write(&dir, "master.qmd", "{{< include ch1.qmd >}}\nAfter.\n");
        let out = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap();
        assert_eq!(out.text, "No final newline\n\nAfter.\n");
    }

    #[test]
    fn the_order_is_the_order_of_the_include_lines() {
        let dir = scratch("order");
        write(&dir, "a.qmd", "A\n");
        write(&dir, "b.qmd", "B\n");
        let master = write(&dir, "master.qmd", "{{< include b.qmd >}}\n{{< include a.qmd >}}\n");
        let out = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap();
        assert_eq!(out.text, "B\n\nA\n\n");
        let names: Vec<_> = out.files.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["master.qmd", "b.qmd", "a.qmd"]);
    }

    #[test]
    fn nested_includes_resolve_against_the_master_not_the_includer() {
        let dir = scratch("nested");
        // Quarto resolves `part.qmd` against the master's directory, so the copy in
        // chapters/ must NOT be the one used.
        write(&dir, "part.qmd", "part from the master's directory\n");
        write(&dir, "chapters/part.qmd", "part next to the includer\n");
        write(&dir, "chapters/ch1.qmd", "Before.\n{{< include part.qmd >}}\nAfter.\n");
        let master = write(&dir, "master.qmd", "{{< include chapters/ch1.qmd >}}\n");
        let out = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap();
        assert!(out.text.contains("part from the master's directory"), "{}", out.text);
        assert!(!out.text.contains("next to the includer"));
        // Nested line cut without its newline: "Before.\n" + part + "\n" + "\nAfter.\n"
        assert_eq!(
            out.text,
            "Before.\npart from the master's directory\n\n\nAfter.\n\n"
        );
    }

    #[test]
    fn includes_inside_executable_cells_are_left_alone() {
        let dir = scratch("cells");
        write(&dir, "ch1.qmd", "should not appear\n");
        let source = "```{python}\n{{< include ch1.qmd >}}\n```\n";
        let master = write(&dir, "master.qmd", source);
        let out = expand(source, &master).unwrap();
        assert_eq!(out.text, source);
    }

    #[test]
    fn plain_code_blocks_are_markdown_to_quarto() {
        // Quarto only treats executable cells as code; a plain fence is part of a
        // markdown cell, so an include line inside it is expanded.
        let dir = scratch("plain-fence");
        write(&dir, "ch1.qmd", "X\n");
        let master = write(&dir, "master.qmd", "```\n{{< include ch1.qmd >}}\n```\n");
        let out = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap();
        assert_eq!(out.text, "```\nX\n\n```\n");
    }

    #[test]
    fn circular_includes_are_an_error() {
        let dir = scratch("circular");
        write(&dir, "a.qmd", "{{< include b.qmd >}}\n");
        write(&dir, "b.qmd", "{{< include a.qmd >}}\n");
        let master = write(&dir, "master.qmd", "{{< include a.qmd >}}\n");
        let err = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap_err();
        assert!(err.to_string().contains("circular"), "{err}");
    }

    #[test]
    fn a_missing_file_names_the_include() {
        let dir = scratch("missing");
        let master = write(&dir, "master.qmd", "{{< include nope.qmd >}}\n");
        let err = expand(&std::fs::read_to_string(&master).unwrap(), &master).unwrap_err();
        assert!(err.to_string().contains("nope.qmd"), "{err}");
    }

    #[test]
    fn recognises_only_block_include_shortcodes() {
        assert_eq!(block_include("{{< include ch1.qmd >}}").as_deref(), Some("ch1.qmd"));
        assert_eq!(block_include("  {{< include \"a b.qmd\" >}}  ").as_deref(), Some("a b.qmd"));
        assert_eq!(block_include("{{< INCLUDE ch1.qmd >}}").as_deref(), Some("ch1.qmd"));
        assert_eq!(block_include("text {{< include ch1.qmd >}}"), None, "not alone on the line");
        assert_eq!(block_include("{{</* include ch1.qmd */>}}"), None, "escaped");
        assert_eq!(block_include("{{< meta title >}}"), None, "another shortcode");
    }
}
