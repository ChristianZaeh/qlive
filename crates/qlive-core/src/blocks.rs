//! Source splitting: the unit of incremental work in qlive.
//!
//! A block is the smallest span of source that can be re-rendered on its own.
//! Blocks are separated by blank lines, except for fenced regions (code fences,
//! display math, `:::` divs, YAML front matter) which stay whole even when they
//! contain blank lines.
//!
//! Block ids are deliberately *not* assigned here. Stable identity across edits is
//! the job of the diff layer (M2), which maps old hashes to new ones; a hash alone
//! would collide for two identical paragraphs.

/// What kind of source a block holds. Drives the tier decision later: text-ish
/// kinds can be rendered block-locally (tier A), structural ones force a whole-doc
/// pass (tier B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    FrontMatter,
    Heading,
    Paragraph,
    CodeFence,
    DisplayMath,
    Div,
    Table,
    List,
    Html,
}

/// One re-renderable span of source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    /// 1-based, inclusive.
    pub start_line: usize,
    /// 1-based, inclusive.
    pub end_line: usize,
    /// Position in the document, 0-based.
    pub ordinal: usize,
    /// blake3 of `text`, hex.
    pub hash: String,
}

/// Split markdown into blocks. Never fails: unterminated fences run to end of input.
pub fn split(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut ordinal = 0usize;
    let mut i = 0usize;

    // YAML front matter, only when `---` is the very first line.
    if lines.first().map(|l| l.trim_end() == "---").unwrap_or(false) {
        let mut j = 1;
        while j < lines.len() && lines[j].trim_end() != "---" {
            j += 1;
        }
        let end = j.min(lines.len().saturating_sub(1));
        blocks.push(make(BlockKind::FrontMatter, &lines[0..=end], 1, &mut ordinal));
        i = end + 1;
    }

    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        let kind = classify(lines[i]);
        let end = match kind {
            BlockKind::CodeFence => scan_fence(&lines, i),
            BlockKind::DisplayMath => scan_display_math(&lines, i),
            BlockKind::Div => scan_div(&lines, i),
            BlockKind::Heading => i,
            _ => scan_until_break(&lines, i),
        };
        blocks.push(make(kind, &lines[start..=end], start + 1, &mut ordinal));
        i = end + 1;
    }

    blocks
}

/// Hashes of a document's blocks, in order. Cheap input for the diff layer.
pub fn hashes(source: &str) -> Vec<String> {
    split(source).into_iter().map(|b| b.hash).collect()
}

/// How many blocks differ between two revisions. The premise of the whole project
/// is that this is 1 for an ordinary edit.
pub fn dirty_count(before: &str, after: &str) -> usize {
    let a = hashes(before);
    let b = hashes(after);
    if a.len() != b.len() {
        // Structural change: count it as every block from the first difference on.
        let common = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
        return b.len().saturating_sub(common).max(a.len().saturating_sub(common));
    }
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

fn make(kind: BlockKind, lines: &[&str], start_line: usize, ordinal: &mut usize) -> Block {
    let text = lines.join("\n");
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    let block = Block {
        kind,
        start_line,
        end_line: start_line + lines.len() - 1,
        ordinal: *ordinal,
        hash,
        text,
    };
    *ordinal += 1;
    block
}

fn classify(line: &str) -> BlockKind {
    let t = line.trim_start();
    if t.starts_with("```") || t.starts_with("~~~") {
        BlockKind::CodeFence
    } else if t.starts_with("$$") {
        BlockKind::DisplayMath
    } else if t.starts_with(":::") {
        BlockKind::Div
    } else if t.starts_with('#') {
        BlockKind::Heading
    } else if t.starts_with('|') {
        BlockKind::Table
    } else if is_list_item(t) {
        BlockKind::List
    } else if t.starts_with('<') {
        BlockKind::Html
    } else {
        BlockKind::Paragraph
    }
}

fn is_list_item(t: &str) -> bool {
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return true;
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    let rest = &t[digits..];
    rest.starts_with(". ") || rest.starts_with(") ")
}

/// A fenced code block ends at the next line opening with the same marker.
fn scan_fence(lines: &[&str], start: usize) -> usize {
    let marker = if lines[start].trim_start().starts_with("~~~") {
        "~~~"
    } else {
        "```"
    };
    let mut j = start + 1;
    while j < lines.len() {
        if lines[j].trim_start().starts_with(marker) {
            return j;
        }
        j += 1;
    }
    lines.len() - 1
}

fn scan_display_math(lines: &[&str], start: usize) -> usize {
    let first = lines[start].trim();
    // Single-line form: `$$ a = b $$`.
    if first.len() > 2 && first.ends_with("$$") {
        return start;
    }
    let mut j = start + 1;
    while j < lines.len() {
        if lines[j].contains("$$") {
            return j;
        }
        j += 1;
    }
    lines.len() - 1
}

/// `:::` divs nest. A line of nothing but colons closes one level.
fn scan_div(lines: &[&str], start: usize) -> usize {
    let mut depth = 0usize;
    let mut j = start;
    while j < lines.len() {
        let t = lines[j].trim();
        if t.starts_with(":::") {
            if t.trim_matches(':').trim().is_empty() {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j;
                }
            } else {
                depth += 1;
            }
        }
        j += 1;
    }
    lines.len() - 1
}

/// Run to the next blank line, or to the start of a block that must begin its own
/// (a heading needs no blank line before it in pandoc markdown).
fn scan_until_break(lines: &[&str], start: usize) -> usize {
    let mut j = start;
    while j + 1 < lines.len() {
        let next = lines[j + 1];
        if next.trim().is_empty() {
            break;
        }
        if matches!(
            classify(next),
            BlockKind::Heading | BlockKind::CodeFence | BlockKind::Div | BlockKind::DisplayMath
        ) {
            break;
        }
        j += 1;
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = concat!(
        "---\n",
        "title: t\n",
        "---\n",
        "\n",
        "## Heading\n",
        "\n",
        "Para one\n",
        "still one.\n",
        "\n",
        "```python\n",
        "x = 1\n",
        "\n",
        "y = 2\n",
        "```\n",
        "\n",
        "$$\n",
        "a = b\n",
        "$$\n",
        "\n",
        "- a\n",
        "- b\n",
    );

    #[test]
    fn splits_into_expected_kinds() {
        let kinds: Vec<BlockKind> = split(DOC).iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::FrontMatter,
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::CodeFence,
                BlockKind::DisplayMath,
                BlockKind::List,
            ]
        );
    }

    #[test]
    fn fence_keeps_its_blank_line() {
        let fence = split(DOC)
            .into_iter()
            .find(|b| b.kind == BlockKind::CodeFence)
            .expect("fence");
        assert!(fence.text.contains("x = 1"));
        assert!(fence.text.contains("y = 2"));
        assert_eq!(fence.end_line - fence.start_line, 4);
    }

    #[test]
    fn heading_without_blank_line_is_its_own_block() {
        let kinds: Vec<BlockKind> = split("Text\n## Head\nMore\n").iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![BlockKind::Paragraph, BlockKind::Heading, BlockKind::Paragraph]
        );
    }

    #[test]
    fn editing_one_paragraph_dirties_one_block() {
        let edited = DOC.replace("Para one", "Para two");
        assert_eq!(dirty_count(DOC, &edited), 1);
    }

    #[test]
    fn nested_divs_stay_one_block() {
        let src = "::: {.callout-note}\nouter\n\n::: {.inner}\ninner\n:::\n\n:::\n\nAfter\n";
        let blocks = split(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Div);
        assert!(blocks[0].text.contains("inner"));
    }

    #[test]
    fn unterminated_fence_does_not_panic() {
        let blocks = split("```\nno end\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::CodeFence);
    }
}
