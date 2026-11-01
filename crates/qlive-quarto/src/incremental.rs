//! Conservative paragraph updates. Anything outside this subset uses the full pipeline.
use kuchiki::{traits::TendrilSink, NodeRef};
use qlive_core::{blocks::{self, Block, BlockKind}, frontmatter};

pub struct Document { blocks: Vec<Block>, pub html: String, ranges: Vec<std::ops::Range<usize>> }
pub struct Change { pub id: String, pub text: String, blocks: Vec<Block> }

pub fn allowed(env: &[(String, String)]) -> bool {
    use base64::Engine;
    let Some((_, encoded)) = env.iter().find(|(key, _)| key == "QUARTO_FILTER_PARAMS") else { return false; };
    let Some(params) = base64::engine::general_purpose::STANDARD.decode(encoded).ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()) else { return false; };
    ["beforeQuartoFilters", "afterQuartoFilters", "entryPoints"].iter().all(|key|
        params["quarto-filters"][key].as_array().is_some_and(|filters| filters.is_empty()))
}

fn safe_text(text: &str) -> bool {
    !text.chars().any(|c| "[]{}<>@$\\|`".contains(c))
        && !text.lines().any(|line| matches!(line.trim(), "---" | "***" | "___"))
}

fn eligible(source: &str) -> Option<Vec<Block>> {
    let (front, body) = frontmatter::split(source);
    if let Some(front) = front {
        let meta: serde_yaml::Value = serde_yaml::from_str(front).ok()?;
        if meta["filters"] != serde_yaml::Value::Null || meta["shortcodes"] != serde_yaml::Value::Null { return None; }
    }
    let blocks = blocks::split(body);
    blocks.iter().all(|block| match block.kind {
        BlockKind::Paragraph | BlockKind::Heading => safe_text(&block.text),
        BlockKind::CodeFence => !block.text.lines().next().unwrap_or("").contains("{="),
        _ => false,
    }).then_some(blocks)
}

fn fragment(html: &str) -> NodeRef {
    kuchiki::parse_html().one(format!("<!doctype html><html><head></head><body>{html}</body></html>"))
}
fn inner(doc: &NodeRef) -> String {
    doc.select_first("body").unwrap().as_node().children().map(|n| n.to_string()).collect()
}
fn paragraphs(doc: &NodeRef) -> Vec<NodeRef> {
    doc.select("p").unwrap().map(|n| n.as_node().clone()).filter(|n| !n.ancestors().any(|parent| {
        parent.as_element().is_some_and(|el| {
            let attrs = el.attributes.borrow();
            matches!(attrs.get("id"), Some("title-block-header" | "quarto-appendix"))
                || attrs.get("class").is_some_and(|s| s.split_ascii_whitespace().any(|c| c == "cell"))
        })
    })).collect()
}

impl Document {
    pub fn prepare(source: &str, html: &str) -> Option<Self> {
        let blocks = eligible(source)?;
        let doc = fragment(html);
        if doc.select("[data-qlive-id]").ok()?.next().is_some() { return None; }
        let nodes = paragraphs(&doc);
        if nodes.len() != blocks.iter().filter(|b| b.kind == BlockKind::Paragraph).count() { return None; }
        for (i, node) in nodes.iter().enumerate() {
            node.as_element().unwrap().attributes.borrow_mut().insert("data-qlive-id", format!("p{i}"));
        }
        let html = inner(&doc);
        let mut ranges = Vec::new();
        for i in 0..nodes.len() {
            // Match a serialized opening tag, never a marker quoted in prose/code.
            // Other paragraph attributes conservatively disable this fast path.
            let marker = html.find(&format!("<p data-qlive-id=\"p{i}\""))?;
            let start = marker;
            let end = marker + html[marker..].find("</p>")? + 4;
            ranges.push(start..end);
        }
        Some(Self { blocks, html, ranges })
    }

    pub fn change(&self, source: &str) -> Option<Change> {
        let blocks = eligible(source)?;
        if self.blocks.len() != blocks.len() { return None; }
        let mut changed = None;
        let mut paragraph = 0;
        for (old, new) in self.blocks.iter().zip(&blocks) {
            if old.kind != new.kind { return None; }
            if old.text != new.text {
                if new.kind != BlockKind::Paragraph || changed.is_some() { return None; }
                changed = Some((format!("p{paragraph}"), new.text.clone()));
            }
            if new.kind == BlockKind::Paragraph { paragraph += 1; }
        }
        let (id, text) = changed?;
        Some(Change { id, text, blocks })
    }

    /// Accept only a single paragraph from the real warm Quarto worker.
    pub fn replace(&mut self, change: Change, rendered: &str) -> Option<String> {
        let parsed = fragment(rendered);
        let children: Vec<_> = parsed.select_first("body").unwrap().as_node().children()
            .filter(|n| !n.as_text().is_some_and(|t| t.borrow().trim().is_empty())).collect();
        if children.len() != 1 || !children[0].as_element().is_some_and(|el| el.name.local.as_ref() == "p") { return None; }
        let replacement = children[0].clone();
        replacement.as_element().unwrap().attributes.borrow_mut().insert("data-qlive-id", change.id.clone());
        let index: usize = change.id.strip_prefix('p')?.parse().ok()?;
        let range = self.ranges.get(index)?.clone();
        let patch = replacement.to_string();
        let delta = patch.len() as isize - range.len() as isize;
        self.html.replace_range(range.clone(), &patch);
        self.ranges[index] = range.start..range.start + patch.len();
        for following in self.ranges.iter_mut().skip(index + 1) {
            following.start = following.start.checked_add_signed(delta)?;
            following.end = following.end.checked_add_signed(delta)?;
        }
        self.blocks = change.blocks;
        Some(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patches_one_paragraph_and_preserves_cells_and_source_ids() {
        let source = "## Heading\n\nFirst paragraph.\n\n```{python}\nx = 1\n```\n\nLast paragraph.\n";
        let html = "<section><h2>Heading</h2><p>First paragraph.</p><div class='cell' data-qlive-source='0'><p>Cell output</p></div><p>Last paragraph.</p></section>";
        let mut doc = Document::prepare(source, html).unwrap();
        let change = doc.change(&source.replace("Last paragraph.", "Edited **paragraph**.")).unwrap();
        assert_eq!(change.id, "p1");
        let patch = doc.replace(change, "<p>Edited <strong>paragraph</strong>.</p>").unwrap();
        assert!(patch.contains("data-qlive-id=\"p1\""));
        assert!(doc.html.contains("data-qlive-source=\"0\"") && doc.html.contains("Cell output"));
        assert!(!doc.html.contains("Last paragraph."));
    }
    #[test]
    fn structural_changes_and_global_features_require_full_render() {
        let source = "## Heading\n\nFirst paragraph.\n\nLast paragraph.\n";
        let doc = Document::prepare(source, "<h2>Heading</h2><p>First paragraph.</p><p>Last paragraph.</p>").unwrap();
        for edited in [source.replace("Heading", "Changed heading"), format!("{source}\nNew paragraph.\n"),
            source.replace("First paragraph.", "A citation [@key]."), source.replace("First paragraph.", "Math $x$."),
            source.replace("paragraph", "sentence")] {
            assert!(doc.change(&edited).is_none(), "{edited}");
        }
        assert!(Document::prepare(source, "<p>Too few nodes</p>").is_none());
    }
}
