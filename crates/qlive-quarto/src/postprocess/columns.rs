//! Port of Quarto 1.8.27's processColumnElements and the column part of
//! bootstrapHtmlFinalizer: margin content moves next to what it annotates, and every
//! ancestor of a column element joins the page grid (`page-columns page-full`).
//! Body-only: `<main>`'s own classes come from the cold shell.

use super::{add_class, attr, has_class, inner_html, nodes, set_attr};
use kuchiki::NodeRef;

#[derive(Debug, Clone)]
pub struct Columns {
    /// `page-layout` is unset, `article` or `full`.
    pub article: bool,
    pub disable: bool,
    pub margin_refs: bool,
    pub margin_cites: bool,
    pub margin_figcaps: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self { article: true, disable: false, margin_refs: false, margin_cites: false, margin_figcaps: false }
    }
}

impl Columns {
    /// Format metadata first; Quarto mirrors `reference-location` into its filter params.
    pub fn from_values(metadata: &serde_yaml::Value, params: &serde_json::Value) -> Self {
        let get = |key: &str| metadata[key].as_str().or_else(|| params[key].as_str()).map(str::to_owned);
        Self {
            article: matches!(get("page-layout").as_deref(), None | Some("article" | "full")),
            disable: metadata["disable-article-layout"].as_bool() == Some(true),
            margin_refs: get("reference-location").as_deref() == Some("margin"),
            margin_cites: get("citation-location").as_deref() == Some("margin"),
            margin_figcaps: get("fig-cap-location").as_deref() == Some("margin"),
        }
    }
}

const COLUMN_SELECTOR: &str = r#"[class^="column-"], [class*=" column-"], aside:not(.footnotes):not(.sidebar), [class*="margin-caption"], [class*=" margin-caption"], [class*="margin-ref"], [class*=" margin-ref"]"#;

const ALL_COLUMN: &[&str] = &[
    "column-body-outset", "column-body-outset-left", "column-body-outset-right",
    "column-page-inset", "column-page-inset-left", "column-page-inset-right",
    "column-page", "column-page-left", "column-page-right",
    "column-screen-inset", "column-screen-inset-left", "column-screen-inset-right",
    "column-screen", "column-screen-left", "column-screen-right", "column-margin",
];
const REMOVE_MARGIN: &[&str] = &[
    "column-body-outset", "column-body-outset-right", "column-page-inset", "column-page-inset-right",
    "column-page", "column-page-right", "column-screen-inset", "column-screen-inset-right",
    "column-screen", "column-screen-right", "column-margin",
];
const NON_SCREEN: &[&str] = &[
    "column-body-outset", "column-body-outset-left", "column-body-outset-right",
    "column-page-inset", "column-page-inset-left", "column-page-inset-right",
    "column-page", "column-page-left", "column-page-right",
    "column-screen-inset", "column-screen-inset-left", "column-screen-inset-right",
    "column-screen-left", "column-screen-right", "column-margin",
];

fn classes(node: &NodeRef) -> Vec<String> {
    attr(node, "class").unwrap_or_default().split_ascii_whitespace().map(str::to_owned).collect()
}

fn remove_class(node: &NodeRef, class: &str) {
    if !has_class(node, class) { return; }
    let kept: Vec<_> = classes(node).into_iter().filter(|c| c != class).collect();
    set_attr(node, "class", &kept.join(" "));
}

fn tag(node: &NodeRef) -> String {
    node.as_element().map(|e| e.name.local.as_ref().to_ascii_uppercase()).unwrap_or_default()
}

fn parent_element(node: &NodeRef) -> Option<NodeRef> {
    node.parent().filter(|p| p.as_element().is_some())
}

fn next_element(node: &NodeRef) -> Option<NodeRef> {
    node.following_siblings().find(|n| n.as_element().is_some())
}

fn previous_element(node: &NodeRef) -> Option<NodeRef> {
    node.preceding_siblings().find(|n| n.as_element().is_some())
}

fn by_id(doc: &NodeRef, id: &str) -> Option<NodeRef> {
    doc.descendants().find(|n| attr(n, "id").as_deref() == Some(id))
}

/// DOM `parent.insertBefore(node, reference)`: a missing reference appends.
fn insert_before(parent: &NodeRef, node: &NodeRef, reference: Option<NodeRef>) {
    match reference {
        Some(reference) => reference.insert_before(node.clone()),
        None => parent.append(node.clone()),
    }
}

fn create(html: &str) -> NodeRef {
    super::element(html)
}

fn deep_clone(node: &NodeRef) -> NodeRef {
    match node.as_text() {
        Some(text) => NodeRef::new_text(text.borrow().clone()),
        None => create(&node.to_string()),
    }
}

fn is_in_column_layout(node: &NodeRef, list: &[&str]) -> bool {
    node.ancestors().any(|a| classes(&a).iter().any(|c| list.contains(&c.as_str())))
}

/// Runs last, after the appendix, as Quarto's finalizer does.
pub fn process(doc: &NodeRef, columns: &Columns) {
    for el in nodes(doc, COLUMN_SELECTOR) {
        if is_in_column_layout(&el, NON_SCREEN) {
            for class in ALL_COLUMN { remove_class(&el, class); }
        }
    }
    for el in nodes(doc, ".margin-caption,.margin-ref") {
        if is_in_column_layout(&el, REMOVE_MARGIN) {
            remove_class(&el, "margin-caption");
            remove_class(&el, "margin-ref");
        }
    }
    let (mut refs, mut cites) = (false, false);
    if columns.article {
        margin_captions(doc);
        margin_in_callouts(doc);
        margin_in_tabsets(doc);
        figure_outputs(doc);
        refs = columns.margin_refs;
        cites = columns.margin_cites;
        margin_nodes(doc, refs, columns.margin_figcaps, cites);
        if refs {
            if let Some(footnotes) = by_id(doc, "footnotes") { footnotes.detach(); }
        }
        for node in nodes(doc, COLUMN_SELECTOR) {
            if let Some(parent) = parent_element(&node) { ensure_in_grid(&parent); }
        }
    }
    if columns.disable { strip_column_classes(doc); }
    if refs {
        if let Some(section) = nodes(doc, "section.footnotes").into_iter().next() { section.detach(); }
    }
    if cites {
        if let Some(refs) = nodes(doc, "div#refs").into_iter().next() { refs.detach(); }
    }
}

fn ensure_in_grid(el: &NodeRef) {
    let mut current = Some(el.clone());
    while let Some(el) = current {
        if tag(&el) == "BODY" || tag(&el) == "HTML" || (tag(&el) == "DIV" && attr(&el, "id").as_deref() == Some("quarto-content")) {
            return;
        }
        if !has_class(&el, "quarto-layout-row") { add_class(&el, "page-columns"); }
        add_class(&el, "page-full");
        current = parent_element(&el);
    }
}

fn strip_column_classes(doc: &NodeRef) {
    for el in doc.descendants().filter(|n| n.as_element().is_some()) {
        for class in classes(&el) {
            if class == "margin-caption" || class == "margin-ref" || class.starts_with("column-") || class == "page-columns" || class == "page-full" {
                remove_class(&el, &class);
            }
        }
    }
}

fn find_quarto_figure(el: &NodeRef) -> Option<NodeRef> {
    el.inclusive_ancestors().filter(|n| n.as_element().is_some())
        .find(|n| has_class(n, "quarto-figure") || has_class(n, "quarto-layout-panel") || has_class(n, "quarto-float"))
}

fn margin_captions(doc: &NodeRef) {
    for el in nodes(doc, ".column-margin .margin-caption") { remove_class(&el, "margin-caption"); }
    for figure in nodes(doc, ".margin-caption") {
        if let Some(container) = find_quarto_figure(&figure) {
            let explicit = attr(&container, "data-cap-location");
            if explicit.is_none() || explicit.as_deref() == Some("margin") {
                if has_class(&container, "quarto-layout-panel") {
                    layout_panel_caption(&container);
                } else {
                    figure_caption(&container);
                }
            }
        } else if has_class(&figure, "tbl-parent") {
            if let Some(caption) = nodes(&figure, "div.panel-caption").into_iter().next() {
                add_class(&caption, "margin-caption");
                caption.detach();
                figure.append(caption);
            }
        } else if let Some(table) = nodes(&figure, "table").into_iter().next() {
            table_caption(&table);
        }
        remove_class(&figure, "margin-caption");
    }
}

fn layout_panel_caption(container: &NodeRef) {
    if let Some(figure) = nodes(container, "figure").into_iter().next() {
        if let Some(caption) = figure.children().find(|c| tag(c) == "FIGCAPTION") {
            add_class(&caption, "margin-caption");
            remove_class(container, "margin-caption");
        }
    } else if let Some(caption) = nodes(container, ".panel-caption").into_iter().next() {
        add_class(&caption, "margin-caption");
        remove_class(container, "margin-caption");
    }
}

fn figure_caption(container: &NodeRef) {
    if let Some(caption) = nodes(container, "figcaption").into_iter().next() {
        add_class(&caption, "margin-caption");
        remove_class(container, "margin-caption");
    } else if let Some(caption) = nodes(container, "caption").into_iter().next() {
        if let Some(div) = parent_element(&caption).and_then(|p| parent_element(&p)) {
            add_class(&caption, "hidden");
            div.append(create(&format!("<div class=\"figure-caption margin-caption\">{}</div>", inner_html(&caption))));
            remove_class(container, "margin-caption");
        }
    }
}

fn table_caption(table: &NodeRef) {
    if let Some(caption) = nodes(table, "caption").into_iter().next() {
        let div = create(&format!("<div class=\"quarto-table-caption margin-caption\">{}</div>", inner_html(&caption)));
        if let Some(parent) = parent_element(table) { insert_before(&parent, &div, next_element(table)); }
        caption.detach();
        remove_class(table, "margin-caption");
    }
}

fn collapse_classes(callout: &NodeRef) -> Vec<String> {
    nodes(callout, ".callout-collapse").first().map(classes).unwrap_or_default()
}

fn margin_in_callouts(doc: &NodeRef) {
    for callout in nodes(doc, "div.callout") {
        let collapse = collapse_classes(&callout);
        let simple = has_class(&callout, "callout-style-simple");
        let margins = nodes(&callout, ".callout-body-container .column-margin, .callout-body-container aside:not(.footnotes):not(.sidebar), .callout-body-container .aside:not(.footnotes)");
        for margin in margins.into_iter().rev() {
            for class in &collapse { add_class(&margin, class); }
            add_class(&margin, "callout-margin-content");
            if simple { add_class(&margin, "callout-margin-content-simple"); }
            callout.insert_after(margin);
        }
    }
}

fn margin_in_tabsets(doc: &NodeRef) {
    for tabset in nodes(doc, "div.panel-tabset") {
        let mut moved = Vec::new();
        for (count, tab) in nodes(&tabset, "div.tab-pane").into_iter().enumerate() {
            let id = attr(&tab, "id").unwrap_or_default();
            let margins = nodes(&tab, ".column-margin, aside:not(.footnotes):not(.sidebar), .aside:not(.footnotes)");
            if id.is_empty() { continue; }
            for margin in margins {
                add_class(&margin, "tabset-margin-content");
                add_class(&margin, &format!("{id}-tab-margin-content"));
                if count > 0 { add_class(&margin, "collapse"); }
                moved.push(margin);
            }
        }
        // Quarto tests an array for truthiness, so every tabset gets a container.
        let container = create("<div class=\"tabset-margin-container\"></div>");
        for margin in moved { container.append(margin); }
        tabset.insert_before(container);
    }
}

fn move_column_classes(from: &NodeRef, to: &NodeRef) {
    for class in classes(from).into_iter().filter(|c| c.starts_with("column-")) {
        remove_class(from, &class);
        add_class(to, &class);
    }
}

fn inside_about(el: &NodeRef) -> bool {
    el.ancestors().any(|a| classes(&a).iter().any(|c| c.starts_with("quarto-about-")))
}

fn figure_outputs(doc: &NodeRef) {
    for column in nodes(doc, r#"[class^="column-"]:not(.column-margin), [class*=" column-"]:not(.column-margin)"#) {
        if !nodes(&column, ".grid").is_empty() { continue; }
        let figures = nodes(&column, "figure img.figure-img");
        if figures.len() == 1 && !inside_about(&figures[0]) {
            move_column_classes(&column, &figures[0]);
        } else {
            let rows = nodes(&column, ".quarto-layout-panel > figure.figure .quarto-layout-row");
            if rows.len() == 1 && !inside_about(&rows[0]) { move_column_classes(&column, &rows[0]); }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Processor { Footnote, Simple, FigCapInCallout, Reference }

impl Processor {
    fn selector(self) -> &'static str {
        match self {
            Self::Footnote => ".footnote-ref",
            Self::Simple => ".column-margin:not(.column-container)",
            Self::FigCapInCallout => ".callout",
            Self::Reference => "a[role='doc-biblioref']",
        }
    }
    fn can(self, el: &NodeRef) -> bool {
        match self {
            Self::Footnote => has_class(el, "footnote-ref"),
            Self::Simple => has_class(el, "column-margin") && !has_class(el, "column-container"),
            Self::FigCapInCallout => !nodes(el, "figcaption").is_empty(),
            Self::Reference => attr(el, "role").as_deref() == Some("doc-biblioref"),
        }
    }
}

fn margin_nodes(doc: &NodeRef, refs: bool, figcaps: bool, cites: bool) {
    let mut processors = vec![Processor::Simple];
    if figcaps { processors.push(Processor::FigCapInCallout); }
    if refs { processors.insert(0, Processor::Footnote); }
    if cites { processors.push(Processor::Reference); }
    let selector = processors.iter().map(|p| p.selector()).collect::<Vec<_>>().join(", ");
    for el in nodes(doc, &selector) {
        if let Some(processor) = processors.iter().find(|p| p.can(&el)) {
            match processor {
                Processor::Footnote => footnote(doc, &el),
                Processor::Simple => simple(&el),
                Processor::FigCapInCallout => figcap_in_callout(&el),
                Processor::Reference => reference(doc, &el),
            }
        }
        remove_class(&el, "column-margin");
    }
}

fn simple(el: &NodeRef) {
    remove_class(el, "column-margin");
    match parent_element(el) {
        Some(parent) if tag(&parent) == "DD" => {
            let container = create("<div></div>");
            el.detach();
            container.append(el.clone());
            if let Some(grand) = parent_element(&parent) { insert_before(&grand, &container, parent.next_sibling()); }
            add_to_margin(&container, &container);
        }
        _ => add_to_margin(el, el),
    }
}

fn footnote(doc: &NodeRef, el: &NodeRef) {
    let Some(target) = attr(el, "href") else { return };
    let Some(contents) = by_id(doc, target.get(1..).unwrap_or("")) else { return };
    if let Some(back) = nodes(&contents, ".footnote-back").into_iter().next() { back.detach(); }
    if let Some(first) = contents.first_child().filter(|f| f.as_element().is_some()) {
        first.prepend(NodeRef::new_text("\u{a0}"));
        if let Some(mark) = el.first_child() { first.prepend(deep_clone(&mark)); }
    }
    const INVALID: &[&str] = &["SPAN", "EM", "STRONG", "DEL", "H1", "H2", "H3", "H4", "H5", "H6"];
    let valid = el.ancestors().filter(|a| a.as_element().is_some()).find(|a| !INVALID.contains(&tag(a).as_str()));
    let anchor = valid.unwrap_or_else(|| el.clone());
    if tag(&contents) == "LI" {
        let container = create("<div></div>");
        if let Some(id) = attr(&contents, "id") { set_attr(&container, "id", &id); }
        for child in contents.children().collect::<Vec<_>>() { container.append(child); }
        add_to_margin(&anchor, &container);
    } else {
        add_to_margin(&anchor, &contents);
    }
}

fn reference(doc: &NodeRef, el: &NodeRef) {
    let Some(target) = attr(el, "href") else { return };
    let Some(contents) = by_id(doc, target.get(1..).unwrap_or("")) else { return };
    let caption = el.ancestors().find(|a| tag(a) == "FIGCAPTION");
    if let Some(caption) = caption {
        if has_class(&caption, "margin-caption") { caption.append(deep_clone(&contents)); } else { add_to_margin(&caption, &contents); }
    } else if let Some(parent) = el.ancestors().filter(|a| a.as_element().is_some()).find(|a| tag(a) != "SPAN") {
        add_to_margin(&parent, &contents);
    }
}

fn figcap_in_callout(el: &NodeRef) {
    let collapse = collapse_classes(el);
    let simple = has_class(el, "callout-style-simple");
    for figure in nodes(el, "figure").into_iter().rev() {
        let caption = nodes(&figure, "figcaption").into_iter().next();
        let id = attr(&figure, "id").filter(|id| !id.is_empty())
            .or_else(|| parent_element(&figure).and_then(|p| attr(&p, "id"))).unwrap_or_default();
        let caption_id = format!("{id}-caption");
        set_attr(&figure, "aria-labelledby", &caption_id);
        if let Some(caption) = caption {
            caption.detach();
            let div = create("<div class=\"margin-figure-caption column-margin\"></div>");
            set_attr(&div, "id", &caption_id);
            for class in &collapse { add_class(&div, class); }
            add_class(&div, "callout-margin-content");
            if simple { add_class(&div, "callout-margin-content-simple"); }
            for child in caption.children().collect::<Vec<_>>() { div.append(child); }
            if let Some(parent) = parent_element(el) { insert_before(&parent, &div, next_element(el)); }
        }
    }
}

fn is_container(el: &NodeRef) -> bool {
    tag(el) == "DIV" && has_class(el, "column-container") && has_class(el, "column-margin")
}

fn already_in_margin(el: &NodeRef) -> bool {
    el.inclusive_ancestors().filter(|a| a.as_element().is_some()).any(|a| {
        has_class(&a, "column-margin") || (has_class(&a, "aside") && !has_class(&a, "footnotes")) || has_class(&a, "margin-caption")
    })
}

fn margin_container() -> NodeRef {
    create("<div class=\"no-row-height column-margin column-container\"></div>")
}

/// The outermost OL/UL/TABLE ancestor below `<main>` (here: below `<body>`).
fn outermost_list(el: &NodeRef) -> Option<NodeRef> {
    el.ancestors().take_while(|a| tag(a) != "MAIN").filter(|a| ["OL", "UL", "TABLE"].contains(&tag(a).as_str())).last()
}

fn container_for(el: &NodeRef) -> NodeRef {
    let parent = parent_element(el);
    if let Some(parent) = parent.as_ref().filter(|p| already_in_margin(p)) { return parent.clone(); }
    if let Some(next) = next_element(el).filter(is_container) { return next; }
    if let Some(previous) = previous_element(el).filter(is_container) { return previous; }
    if let Some(callout) = el.ancestors().find(|a| has_class(a, "callout")) {
        let container = margin_container();
        if let Some(holder) = callout.parent() { insert_before(&holder, &container, next_element(&callout)); }
        return container;
    }
    if let Some(list) = outermost_list(el) {
        if let Some(next) = next_element(&list).filter(is_container) { return next; }
        let container = margin_container();
        if let Some(holder) = list.parent() { insert_before(&holder, &container, next_element(&list)); }
        return container;
    }
    if let Some(parent) = parent.filter(|p| ["P", "BLOCKQUOTE"].contains(&tag(p).as_str())) {
        if parent_element(&parent).is_some_and(|g| tag(&g) == "DIV") {
            if let Some(next) = next_element(&parent).filter(is_container) { return next; }
        }
        let container = margin_container();
        let wrapper = create("<div></div>");
        parent.insert_before(wrapper.clone());
        parent.detach();
        wrapper.append(parent);
        wrapper.append(container.clone());
        return container;
    }
    let container = margin_container();
    if let Some(holder) = el.parent() { insert_before(&holder, &container, next_element(el)); }
    container
}

fn add_to_margin(el: &NodeRef, content: &NodeRef) {
    let container = container_for(el);
    container.append(content.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{fragment, inner_html};

    fn run(html: &str, columns: Columns) -> String {
        let doc = fragment(html);
        process(&doc, &columns);
        inner_html(&nodes(&doc, "body").remove(0))
    }

    #[test]
    fn every_tabset_gets_a_margin_container() {
        let out = run(r#"<div class="panel-tabset"><div class="tab-pane" id="a"><p>x</p></div></div>"#, Columns::default());
        assert!(out.starts_with(r#"<div class="tabset-margin-container"></div><div class="panel-tabset">"#), "{out}");
    }

    #[test]
    fn margin_footnotes_follow_their_table_and_pull_ancestors_into_the_grid() {
        let html = r##"<div id="tbl-x" class="quarto-float"><figure><div><table><tbody><tr><td>1<a href="#fn1" class="footnote-ref"><sup>1</sup></a></td></tr></tbody></table></div></figure></div><section id="footnotes" class="footnotes"><ol><li id="fn1"><p>Note<a href="#fnref1" class="footnote-back">↩︎</a></p></li></ol></section>"##;
        let out = run(html, Columns { margin_refs: true, ..Columns::default() });
        assert!(out.contains(r#"</table><div class="no-row-height column-margin column-container"><div id="fn1"><p><sup>1</sup>&nbsp;Note</p></div></div>"#), "{out}");
        assert!(out.contains(r#"<div class="quarto-float page-columns page-full" id="tbl-x">"#), "{out}");
        assert!(!out.contains("footnotes"), "{out}");
    }

    #[test]
    fn other_page_layouts_skip_margins_but_still_strip_nesting() {
        let html = r#"<div class="column-page"><div class="column-margin">m</div></div>"#;
        let out = run(html, Columns { article: false, ..Columns::default() });
        assert_eq!(out, r#"<div class="column-page"><div class="">m</div></div>"#);
    }
}
