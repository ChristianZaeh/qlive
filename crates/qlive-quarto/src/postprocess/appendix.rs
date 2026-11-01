//! Quarto 1.8.27 processDocumentAppendix: rebuild dynamic sections, reuse cold metadata.
use super::*;

#[derive(Debug, Clone)]
pub struct Appendix {
    style: String,
    margin_cites: bool,
    margin_notes: bool,
    references_title: String,
    footnotes_title: String,
    static_sections: Vec<String>,
    pub(super) results_file: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_metadata_does_not_freeze_dynamic_notes_or_references() {
        let cold = "<main class='content'><div id='quarto-appendix'><section id='quarto-citation'>Static citation</section><section role='doc-endnotes'>STALE NOTE</section><section id='quarto-bibliography'><div id='refs'>STALE REF</div></section></div></main>";
        let options = Options::from_metadata("").unwrap().with_cold_page(cold);
        let body = "<div id='refs'>CURRENT REF</div><section role='doc-endnotes'><hr><ol><li>CURRENT NOTE</li></ol></section>";
        let out = process(body, &options);
        assert!(out.contains("CURRENT REF") && out.contains("CURRENT NOTE") && out.contains("Static citation"));
        assert!(!out.contains("STALE") && !out.contains("<hr"));
        let doc = fragment(&out);
        assert_eq!(nodes(&doc, "#quarto-appendix > section").len(), 3);
        assert_eq!(nodes(&doc, "#quarto-bibliography > h2")[0].text_contents(), "References");
        let removed = process("<p>No more notes or references.</p>", &options);
        assert!(!removed.contains("doc-endnotes") && !removed.contains("quarto-bibliography"));
        assert!(removed.contains("Static citation"));
    }

    #[test]
    fn no_empty_appendix_and_respect_disabled_book_and_margin_modes() {
        let body = "<section role='doc-endnotes'><hr><ol><li>Note</li></ol></section>";
        for metadata in ["appendix-style: none", "appendix-style: false", "book: true", "reference-location: margin"] {
            let out = process(body, &Options::from_metadata(metadata).unwrap());
            assert!(!out.contains("quarto-appendix"), "{metadata}: {out}");
        }
        let out = process("<p>Body</p>", &Options::from_metadata("").unwrap());
        assert!(!out.contains("quarto-appendix"));
        let out = process(body, &Options::from_metadata("appendix-style: plain").unwrap());
        assert!(out.contains("<div id=\"quarto-appendix\">"));
    }
}

impl Appendix {
    pub(super) fn from_metadata(value: &serde_yaml::Value) -> Option<Self> {
        if value["book"] != serde_yaml::Value::Null
            && value["book"] != serde_yaml::Value::Bool(false) { return None; }
        if value["appendix-style"] == serde_yaml::Value::Bool(false)
            || value["appendix-style"].as_str() == Some("none") { return None; }
        Some(Self {
            style: if value["appendix-style"].as_str() == Some("plain") { "plain" } else { "default" }.into(),
            margin_cites: value["citation-location"].as_str() == Some("margin"),
            margin_notes: value["reference-location"].as_str() == Some("margin"),
            references_title: value["language"]["section-title-references"].as_str().unwrap_or("References").into(),
            footnotes_title: value["language"]["section-title-footnotes"].as_str().unwrap_or("Footnotes").into(),
            static_sections: Vec::new(),
            results_file: None,
        })
    }

    pub(super) fn language(&mut self, language: &serde_json::Value) {
        if let Some(title) = language["section-title-references"].as_str() { self.references_title = title.into(); }
        if let Some(title) = language["section-title-footnotes"].as_str() { self.footnotes_title = title.into(); }
    }

    pub(super) fn capture_static(&mut self, page: &str) {
        let doc = kuchiki::parse_html().one(page);
        self.static_sections = nodes(&doc, "#quarto-appendix > #quarto-reuse, #quarto-appendix > #quarto-copyright, #quarto-appendix > #quarto-citation")
            .iter().map(ToString::to_string).collect();
        // reference-location is a pandoc writer option, and may be absent from metadata.
        if !nodes(&doc, "[role=doc-noteref]").is_empty()
            && nodes(&doc, "#quarto-appendix section[role=doc-endnotes]").is_empty() {
            self.margin_notes = true;
        }
    }

    fn heading(container: &NodeRef, title: &str) {
        let heading = element("<h2 class='anchored quarto-appendix-heading'></h2>");
        if title != "none" {
            let parsed = fragment(title);
            for child in nodes(&parsed, "body")[0].children() { heading.append(child); }
        }
        container.prepend(heading);
        if let Some(hr) = nodes(container, "hr").first() { hr.detach(); }
    }

    pub(super) fn process(&self, doc: &NodeRef) {
        let appendix = element("<div id='quarto-appendix'></div>");
        if self.style != "plain" { add_class(&appendix, &self.style); }
        let mut sections = Vec::new();
        let positioned_refs = self.results_file.as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .is_some_and(|v| v["inputTraits"]["positioned-refs"].as_bool() == Some(true));
        if !self.margin_cites && !positioned_refs {
            if let Some(refs) = nodes(doc, "#refs").first() {
                let mut title = self.references_title.clone();
                if let Some(parent) = refs.parent() {
                    let is_section = parent.as_element().is_some_and(|e| e.name.local.as_ref() == "section");
                    if is_section && parent.children().filter(|n| n.as_element().is_some()).count() == 2 {
                        if let Some(heading) = nodes(&parent, "h1, h2, h3, h4, h5, h6").first() {
                            let existing = heading.text_contents();
                            if !existing.is_empty() { title = existing; }
                            heading.detach();
                        }
                    }
                }
                let section = element("<section id='quarto-bibliography' role='doc-bibliography' class='quarto-appendix-contents'></section>");
                section.append(refs.clone());
                Self::heading(&section, &title);
                sections.push(section);
            }
        }
        if !self.margin_notes {
            let notes = nodes(doc, "section[role=doc-endnotes]");
            if notes.len() == 1 {
                Self::heading(&notes[0], &self.footnotes_title);
                sections.push(notes[0].clone());
            }
        }
        for html in &self.static_sections { sections.push(element(html)); }
        let mut authored = Vec::new();
        for section in nodes(doc, "section.appendix") {
            let heading = element("<h2 class='quarto-appendix-heading'></h2>");
            if let Some(old) = nodes(&section, "h1, h2, h3, h4, h5, h6").first() {
                for child in old.children() { heading.append(child); }
                old.detach();
                if attr(&section, "id").is_some_and(|id| !id.is_empty()) { add_class(&heading, "anchored"); }
            }
            // Match Quarto's class insertion order.
            if has_class(&heading, "anchored") { set_attr(&heading, "class", "anchored quarto-appendix-heading"); }
            let contents = element("<div class='quarto-appendix-contents'></div>");
            for child in section.children() { contents.append(child); }
            section.append(heading);
            section.append(contents);
            authored.push(section);
        }
        authored.extend(sections);
        for section in authored { appendix.append(section); }
        if appendix.children().any(|n| n.as_element().is_some()) {
            nodes(doc, "body")[0].append(appendix);
        }
    }
}
