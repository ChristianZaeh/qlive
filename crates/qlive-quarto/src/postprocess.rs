//! Body-only ports of Quarto 1.8.27's htmlFormatPostprocessor and bootstrapHtmlPostprocessor.
//! Keep their order: copying and annotations, then Bootstrap classes and section ids.
//! This covers part of M7, not the entire after-pandoc pipeline.

use kuchiki::{traits::TendrilSink, NodeRef};

mod appendix;
pub use appendix::Appendix;

#[derive(Debug, Clone)]
pub struct Options {
    pub appendix: Option<Appendix>,
    pub bootstrap: bool,
    pub annotations: String,
    pub anchors: bool,
    pub code_copy: bool,
    pub copy_title: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            appendix: None,
            bootstrap: true,
            annotations: "default".into(),
            anchors: true,
            code_copy: true,
            copy_title: "Copy to Clipboard".into(),
        }
    }
}

impl Options {
    /// Language strings live in Quarto's captured filter params, not in pandoc metadata.
    pub fn from_capture(metadata: &str, env: &[(String, String)]) -> anyhow::Result<Self> {
        use base64::Engine;
        let mut options = Self::from_metadata(metadata)?;
        if let Some((_, encoded)) = env.iter().find(|(key, _)| key == "QUARTO_FILTER_PARAMS") {
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
            let params: serde_json::Value = serde_json::from_slice(&bytes)?;
            if let Some(appendix) = &mut options.appendix {
                appendix.language(&params["language"]);
                appendix.results_file = params["results-file"].as_str().map(Into::into);
            }
            if let Some(bootstrap) = params["has-bootstrap"].as_bool() {
                options.bootstrap = bootstrap;
            }
            if let Some(title) = params["language"]["copy-button-tooltip"].as_str() {
                options.copy_title = title.to_owned();
            }
        }
        Ok(options)
    }

    /// Quarto's captured, resolved pandoc metadata, including language strings.
    pub fn from_metadata(metadata: &str) -> anyhow::Result<Self> {
        let value: serde_yaml::Value = serde_yaml::from_str(metadata)?;
        let minimal = value["minimal"].as_bool() == Some(true);
        let enabled = |key: &str| match &value[key] {
            serde_yaml::Value::Bool(v) => *v,
            serde_yaml::Value::String(v) => !v.is_empty(),
            _ => !minimal,
        };
        Ok(Self {
            appendix: Appendix::from_metadata(&value),
            bootstrap: !matches!(value["theme"].as_str(), Some("none" | "pandoc")),
            annotations: match &value["code-annotations"] {
                serde_yaml::Value::Bool(false) => "false".into(),
                serde_yaml::Value::String(style) => style.clone(),
                _ => "default".into(),
            },
            anchors: enabled("anchor-sections"),
            code_copy: enabled("code-copy"),
            copy_title: value["language"]["copy-button-tooltip"]
                .as_str()
                .unwrap_or("Copy to Clipboard")
                .into(),
        })
    }

    /// Metadata-only appendix content is stable until front matter triggers a cold render.
    /// Dynamic footnotes, references and authored appendices are never copied here.
    pub fn with_cold_page(mut self, page: &str) -> Self {
        if let Some(appendix) = &mut self.appendix { appendix.capture_static(page); }
        self
    }
}

fn fragment(html: &str) -> NodeRef {
    // A full HTML5 document parse gives normal table/void/raw-text semantics.
    // Pandoc's body fragment belongs in <body>; no user-controlled wrapper id.
    kuchiki::parse_html().one(format!(
        "<!doctype html><html><head></head><body>{html}</body></html>"
    ))
}

fn nodes(root: &NodeRef, selector: &str) -> Vec<NodeRef> {
    root.select(selector)
        .expect("static CSS selector")
        .map(|n| n.as_node().clone())
        .collect()
}

fn has_class(node: &NodeRef, class: &str) -> bool {
    node.as_element().is_some_and(|el| {
        el.attributes
            .borrow()
            .get("class")
            .is_some_and(|s| s.split_ascii_whitespace().any(|c| c == class))
    })
}

fn add_class(node: &NodeRef, class: &str) {
    if has_class(node, class) {
        return;
    }
    if let Some(el) = node.as_element() {
        let mut attrs = el.attributes.borrow_mut();
        let old = attrs.get("class").unwrap_or("");
        let new = if old.is_empty() {
            class.into()
        } else {
            format!("{old} {class}")
        };
        attrs.insert("class", new);
    }
}

fn inner_html(node: &NodeRef) -> String {
    node.children().map(|child| child.to_string()).collect()
}

fn attr(node: &NodeRef, key: &str) -> Option<String> {
    node.as_element()?
        .attributes
        .borrow()
        .get(key)
        .map(str::to_owned)
}

fn set_attr(node: &NodeRef, key: &str, value: &str) {
    node.as_element()
        .unwrap()
        .attributes
        .borrow_mut()
        .insert(key, value.to_owned());
}

fn element(html: &str) -> NodeRef {
    nodes(&fragment(html), "body")[0].first_child().unwrap()
}

fn previous_element(node: &NodeRef) -> Option<NodeRef> {
    node.preceding_siblings().find(|n| n.as_element().is_some())
}

fn bootstrap_classes(doc: &NodeRef) {
    for (selector, class) in [
        ("blockquote", "blockquote"),
        ("figure", "figure"),
        ("figure img", "figure-img"),
        (
            ".column-margin .cell-output-display img:not(.img-fluid)",
            "img-fluid",
        ),
    ] {
        for node in nodes(doc, selector) {
            add_class(&node, class);
        }
    }
    let table_classes = |table: &NodeRef, computational: bool| {
        add_class(table, "table");
        if computational {
            for class in ["table-sm", "table-striped", "small"] {
                add_class(table, class);
            }
        }
    };
    for row in nodes(doc, "tbody > tr:first-child.odd") {
        if let Some(table) = row.parent().and_then(|p| p.parent()) {
            if let Some(el) = table.as_element() {
                el.attributes.borrow_mut().remove("style");
                let computational = table
                    .ancestors()
                    .any(|n| has_class(&n, "cell") && !has_class(&n, "plain"));
                table_classes(&table, computational);
            }
        }
    }
    for table in nodes(doc, "table.dataframe") {
        table
            .as_element()
            .unwrap()
            .attributes
            .borrow_mut()
            .remove("border");
        table_classes(&table, true);
        for row in nodes(&table, "tr") {
            row.as_element()
                .unwrap()
                .attributes
                .borrow_mut()
                .remove("style");
        }
        if let Some(prev) = previous_element(&table) {
            if prev.as_element().unwrap().name.local.as_ref() == "style" {
                prev.detach();
            }
        }
    }
    for table in nodes(doc, "table.data-frame") {
        table_classes(&table, true);
    }
}

fn code_annotations(doc: &NodeRef, style: &str) {
    let mut code_parents = Vec::<NodeRef>::new();
    let mut lists = Vec::<(String, NodeRef)>::new();
    for annotation in nodes(doc, "span[data-code-cell]") {
        let list = annotation.parent().and_then(|p| p.parent());
        if style == "false" {
            if let Some(list) = list {
                add_class(&list, "code-annotation-container-hidden");
            }
            continue;
        }
        let cell = attr(&annotation, "data-code-cell").unwrap_or_default();
        let lines = attr(&annotation, "data-code-lines").unwrap_or_default();
        let number = attr(&annotation, "data-code-annotation");
        let mut code_parent = None;
        if !cell.is_empty() && !lines.is_empty() {
            let target_id = format!("{cell}-{}", lines.split(',').next().unwrap());
            // Compare ids directly: cell identifiers need not be valid CSS selectors.
            if let Some(target) = nodes(doc, "[id]")
                .into_iter()
                .find(|n| attr(n, "id").as_deref() == Some(&target_id))
            {
                let interactive = matches!(style, "hover" | "select");
                let anchor = element(if interactive {
                    "<button class='code-annotation-anchor'></button>"
                } else {
                    "<a class='code-annotation-anchor'></a>"
                });
                set_attr(&anchor, "data-target-cell", &cell);
                set_attr(
                    &anchor,
                    "data-target-annotation",
                    number.as_deref().unwrap_or("null"),
                );
                if !interactive {
                    // Quarto's later link pass serializes these anchors with an empty href.
                    set_attr(&anchor, "href", "");
                    set_attr(&anchor, "onclick", "event.preventDefault();");
                }
                anchor.append(NodeRef::new_text(
                    number.as_deref().filter(|s| !s.is_empty()).unwrap_or("?"),
                ));
                target.insert_before(anchor);
                add_class(&target, "code-annotation-target");
                code_parent = target.parent();
            }
        }
        if let Some(parent) = code_parent {
            if !code_parents.contains(&parent) {
                code_parents.push(parent.clone());
            }
            let id = parent
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| attr(&p, "id"))
                .unwrap_or_default();
            if let Some(list) = list {
                if !id.is_empty() && !lists.iter().any(|(key, _)| key == &id) {
                    lists.push((id, list));
                }
            }
        }
        if let (Some(number), Some(dt)) = (number, annotation.parent()) {
            if let Some(dd) = previous_element(&dt) {
                for child in dd.children() {
                    child.detach();
                }
                dd.append(NodeRef::new_text(number.clone()));
                if !cell.is_empty() {
                    set_attr(&dd, "data-target-cell", &cell);
                    set_attr(&dd, "data-target-annotation", &number);
                }
            }
        }
    }
    for parent in code_parents {
        if let Some(pre) = parent.parent() {
            add_class(&pre, "code-annotated");
        }
        parent.append(element("<div class='code-annotation-gutter-bg'></div>"));
        parent.append(element("<div class='code-annotation-gutter'></div>"));
    }
    for (_, list) in lists {
        if matches!(style, "hover" | "select") {
            add_class(&list, "code-annotation-container-hidden");
        }
        add_class(&list, "code-annotation-container-grid");
    }
}

pub fn process(body: &str, options: &Options) -> String {
    let doc = fragment(body);
    for pre in nodes(&doc, "pre.sourceCode") {
        let Some(parent) = pre.parent() else { continue };
        for class in ["cell-code", "hidden"] {
            if has_class(&pre, class) {
                let el = pre.as_element().unwrap();
                let mut attrs = el.attributes.borrow_mut();
                let classes = attrs
                    .get("class")
                    .unwrap_or("")
                    .split_ascii_whitespace()
                    .filter(|c| *c != class)
                    .collect::<Vec<_>>()
                    .join(" ");
                attrs.insert("class", classes);
                add_class(&parent, class);
            }
        }
        if options.code_copy {
            let scaffold_doc = fragment("<div class=\"code-copy-outer-scaffold\"><button class=\"code-copy-button\"><i class=\"bi\"></i></button></div>");
            let scaffold = nodes(&scaffold_doc, "div").remove(0);
            let button = nodes(&scaffold, "button").remove(0);
            button
                .as_element()
                .unwrap()
                .attributes
                .borrow_mut()
                .insert("title", options.copy_title.clone());
            if pre.ancestors().any(|n| {
                n.as_element().is_some_and(|el| {
                    el.attributes.borrow().get("id") == Some("quarto-embedded-source-code-modal")
                })
            }) {
                button
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow_mut()
                    .insert("data-in-quarto-modal", String::new());
            }
            add_class(&pre, "code-with-copy");
            parent.insert_before(scaffold.clone());
            parent.detach();
            scaffold.prepend(parent.clone());
        }
        if let Some(source) = attr(&parent, "data-code-preview").filter(|s| !s.is_empty()) {
            let frame = element("<iframe></iframe>");
            if let Some(classes) = attr(&pre, "class") { set_attr(&frame, "class", &classes); }
            let source = source.strip_suffix(".qmd").map(|s| format!("{s}.html")).unwrap_or(source);
            set_attr(&frame, "src", &source);
            parent.as_element().unwrap().attributes.borrow_mut().remove("data-code-preview");
            parent.append(frame);
        }
    }
    if options.anchors {
        for node in nodes(
            &doc,
            "h2, h3, h4, h5, h6, .quarto-figure[id], div[id^=tbl-]",
        ) {
            let is_toc =
                node.as_element().unwrap().attributes.borrow().get("id") == Some("toc-title");
            if !is_toc && !has_class(&node, "no-anchor") {
                add_class(&node, "anchored");
            }
        }
    }
    for number in nodes(&doc, ".header-section-number") {
        if previous_element(&number).is_some_and(|n| has_class(&n, "toc-section-number")) { number.detach(); }
    }
    code_annotations(&doc, &options.annotations);
    for table in nodes(&doc, "table[data-quarto-postprocess=true]") {
        if attr(&table, "data-quarto-disable-processing").as_deref() == Some("true") { continue; }
        {
            let el = table.as_element().unwrap();
            let mut attrs = el.attributes.borrow_mut();
            attrs.remove("data-quarto-postprocess");
            attrs.remove("data-quarto-disable-processing");
        }
        for row in nodes(&table, "tr") {
            for cell in row.children().collect::<Vec<_>>() {
                if !cell.as_element().is_some_and(|e| matches!(e.name.local.as_ref(), "th" | "td")) { continue; }
                let replacement = element(if attr(&cell, "data-quarto-table-cell-role").as_deref() == Some("th") { "<table><tbody><tr><th></th></tr></tbody></table>" } else { "<table><tbody><tr><td></td></tr></tbody></table>" });
                let replacement = nodes(&replacement, "th, td").remove(0);
                *replacement.as_element().unwrap().attributes.borrow_mut() = cell.as_element().unwrap().attributes.borrow().clone();
                for child in cell.children() { replacement.append(child); }
                cell.insert_before(replacement);
                cell.detach();
            }
        }
    }
    if options.bootstrap {
        bootstrap_classes(&doc);
    }
    // Quarto deliberately searches descendants, by heading rank rather than DOM order.
    // This runs even when anchor-sections is false, and later nested sections overwrite ids.
    for section in if options.bootstrap {
        nodes(&doc, "section[class^=level]")
    } else {
        Vec::new()
    } {
        let id = section
            .as_element()
            .unwrap()
            .attributes
            .borrow()
            .get("id")
            .unwrap_or("")
            .to_owned();
        for selector in ["h2", "h3", "h4", "h5", "h6"] {
            if let Some(heading) = nodes(&section, selector).first() {
                heading
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow_mut()
                    .insert("data-anchor-id", id);
                break;
            }
        }
    }
    if options.bootstrap {
        if let Some(appendix) = &options.appendix { appendix.process(&doc); }
    }
    inner_html(&nodes(&doc, "body").remove(0))
}

/// Canonicalize serialization (not content) before parity comparison: DOM attribute
/// order, quote style, entities, and void elements are insignificant in HTML.
pub fn canonical_html(html: &str) -> String {
    let doc = fragment(html);
    inner_html(&nodes(&doc, "body").remove(0))
}

/// Preserve whitespace in preformatted/raw-text elements; normalize ordinary
/// prose whitespace as the existing corpus comparator does.
pub fn comparison_html(html: &str) -> String {
    let doc = fragment(html);
    for node in doc.descendants() {
        let Some(text) = node.as_text() else { continue };
        let preserve = node.ancestors().any(|n| {
            n.as_element().is_some_and(|e| {
                matches!(
                    e.name.local.as_ref(),
                    "pre" | "code" | "textarea" | "script" | "style"
                )
            })
        });
        if preserve {
            continue;
        }
        let old = text.borrow().clone();
        let mut normalized = String::new();
        let mut space = false;
        for ch in old.chars() {
            if ch.is_ascii_whitespace() {
                if !space {
                    normalized.push(' ');
                }
                space = true;
            } else {
                normalized.push(ch);
                space = false;
            }
        }
        *text.borrow_mut() = if normalized.trim().is_empty() {
            String::new()
        } else {
            normalized
        };
    }
    inner_html(&nodes(&doc, "body").remove(0)).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_classes_follow_table_origin_and_theme() {
        let body = "<blockquote>Q</blockquote><figure><img src='x'></figure><div class='cell'><table style='width:100%'><tbody><tr class='odd'><td>X</td></tr></tbody></table></div><div class='cell plain'><table><tbody><tr class='odd'><td>Y</td></tr></tbody></table></div><div><style>ignored</style>\n<table class='dataframe' border='1'><tr style='text-align:right'><td>Z</td></tr></table></div>";
        let doc = fragment(&process(body, &Options::default()));
        assert_eq!(
            nodes(&doc, "blockquote.blockquote, figure.figure, img.figure-img").len(),
            3
        );
        assert_eq!(nodes(&doc, "table.table").len(), 3);
        assert_eq!(nodes(&doc, "table.table-sm.table-striped.small").len(), 2);
        assert!(nodes(&doc, "table[style], table[border], tr[style], style").is_empty());
        let unthemed = Options::from_metadata("theme: none").unwrap();
        let doc = fragment(&process(body, &unthemed));
        assert!(nodes(&doc, ".blockquote, .figure-img, table.table").is_empty());
    }

    #[test]
    fn annotations_resolve_literal_ids_and_build_gutters() {
        let body = "<div id='cell:1'><pre><code><span id='cell:1-3'>x</span></code></pre></div><dl><dt>old</dt><dd><span data-code-cell='cell:1' data-code-lines='3,4' data-code-annotation='1'></span>Explanation</dd></dl>";
        for style in ["default", "hover", "select", "false"] {
            let options = Options {
                annotations: style.into(),
                ..Default::default()
            };
            let doc = fragment(&process(body, &options));
            if style == "false" {
                assert!(nodes(&doc, ".code-annotation-anchor, .code-annotated").is_empty());
                assert_eq!(nodes(&doc, "dl.code-annotation-container-hidden").len(), 1);
                continue;
            }
            assert_eq!(
                nodes(&doc, "pre.code-annotated code > .code-annotation-gutter").len(),
                1
            );
            assert_eq!(
                nodes(&doc, "pre.code-annotated code > .code-annotation-gutter-bg").len(),
                1
            );
            let anchor = nodes(&doc, ".code-annotation-anchor").remove(0);
            assert_eq!(attr(&anchor, "data-target-cell").as_deref(), Some("cell:1"));
            assert_eq!(anchor.text_contents(), "1");
            assert_eq!(
                nodes(&doc, "dt[data-target-annotation='1']")[0].text_contents(),
                "1"
            );
            assert_eq!(nodes(&doc, "dl.code-annotation-container-grid").len(), 1);
            assert_eq!(
                nodes(&doc, "button.code-annotation-anchor").len(),
                usize::from(style != "default")
            );
            assert_eq!(
                nodes(&doc, "dl.code-annotation-container-hidden").len(),
                usize::from(style != "default")
            );
        }
    }

    #[test]
    fn anchors_use_dom_descendants_and_preserve_attributes() {
        let result = process(
            r#"<section class="level2" id="outer"><div><h3 title="a > b">Early</h3><h2 class="custom">Rank wins</h2></div><section id="inner" class="level3"><h3>Nested</h3></section></section><h2 id="toc-title">Toc</h2><h2 class="no-anchor">No</h2><div id="tbl-one"></div><div class="quarto-figure" id="fig-one"></div>"#,
            &Options::default(),
        );
        let doc = fragment(&result);
        let h2 = nodes(&doc, "h2.custom").remove(0);
        assert!(has_class(&h2, "anchored"));
        assert_eq!(
            h2.as_element()
                .unwrap()
                .attributes
                .borrow()
                .get("data-anchor-id"),
            Some("outer")
        );
        assert!(!has_class(&nodes(&doc, "#toc-title")[0], "anchored"));
        assert!(!has_class(&nodes(&doc, ".no-anchor")[0], "anchored"));
        assert!(has_class(&nodes(&doc, "#tbl-one")[0], "anchored"));
        assert!(has_class(&nodes(&doc, "#fig-one")[0], "anchored"));
        assert_eq!(
            nodes(&doc, "#inner h3")[0]
                .as_element()
                .unwrap()
                .attributes
                .borrow()
                .get("data-anchor-id"),
            Some("inner")
        );
    }

    #[test]
    fn copy_scaffold_hoists_classes_and_escapes_localized_title() {
        let options = Options {
            copy_title: "Copy \"this\" & that".into(),
            ..Options::default()
        };
        let out = process("<div class='sourceCode'><pre class='sourceCode cell-code hidden'><code>a &lt; b\n  c</code></pre></div>", &options);
        let doc = fragment(&out);
        assert_eq!(
            nodes(
                &doc,
                ".code-copy-outer-scaffold > div.cell-code.hidden > pre.code-with-copy"
            )
            .len(),
            1
        );
        assert_eq!(
            nodes(&doc, "button")[0]
                .as_element()
                .unwrap()
                .attributes
                .borrow()
                .get("title"),
            Some(options.copy_title.as_str())
        );
        assert_eq!(nodes(&doc, "code")[0].text_contents(), "a < b\n  c");
    }

    #[test]
    fn feature_options_do_not_disable_section_ids_or_hoisting() {
        let options =
            Options::from_metadata("minimal: true\ncode-copy: false\nanchor-sections: false\n")
                .unwrap();
        let out = process("<section id='s' class='level2'><h2>H</h2></section><div><pre class='sourceCode hidden'>x</pre></div>", &options);
        assert!(!out.contains("anchored"));
        assert!(!out.contains("code-copy"));
        assert!(out.contains("data-anchor-id=\"s\""));
        assert!(out.contains("<div class=\"hidden\">"));
        let options = Options::from_metadata(
            "minimal: true\ncode-copy: hover\nlanguage:\n  copy-button-tooltip: Kopieren\n",
        )
        .unwrap();
        assert!(options.code_copy);
        assert!(!options.anchors);
        assert_eq!(options.copy_title, "Kopieren");
    }

    #[test]
    fn canonicalization_keeps_text_and_code_whitespace() {
        assert_eq!(
            canonical_html("<img title='a &amp; b' src='x'>"),
            canonical_html("<img src=\"x\" title=\"a & b\" />")
        );
        assert_ne!(
            canonical_html("<pre>x  y</pre>"),
            canonical_html("<pre>x y</pre>")
        );
        assert_ne!(
            comparison_html("<pre>x  y</pre>"),
            comparison_html("<pre>x y</pre>")
        );
        assert_ne!(
            comparison_html("<code>x\ny</code>"),
            comparison_html("<code>x y</code>")
        );
    }

    #[test]
    fn embedded_source_copy_button_has_modal_marker() {
        let out = process("<div id='quarto-embedded-source-code-modal'><div><pre class='sourceCode'>x</pre></div></div>", &Options::default());
        let doc = fragment(&out);
        assert_eq!(nodes(&doc, "button[data-in-quarto-modal]").len(), 1);
    }
}
