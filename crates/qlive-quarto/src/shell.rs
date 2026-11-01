//! Quarto's page shell, with a hole where the body goes.
//!
//! The cold render gives the full page: template, Bootstrap layout, dependencies,
//! scripts, and the title block. The warm worker gives only the body — pandoc's
//! fragment output, without the template. Splitting the cold page around the content
//! of `<main>` lets every warm render be dropped back into the real page:
//!
//! ```text
//! … <main class="content" id="quarto-document-content">
//!     <header id="title-block-header"> … </header>    ← kept: from the template
//!     ⟨warm body goes here⟩                            ← replaced on every edit
//! </main> …                                            ← kept: scripts, footer
//! ```
//!
//! The title block stays with the shell because pandoc's template generates it from
//! the metadata; a title change is a front-matter edit, which takes the cold path.

use anyhow::{anyhow, Result};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Shell {
    prefix: String,
    suffix: String,
}

impl Shell {
    /// Split a cold-rendered Quarto page.
    pub fn from_page(page: &str) -> Result<Self> {
        let main_open = page
            .find("<main")
            .ok_or_else(|| anyhow!("page has no <main>; not a Quarto HTML page?"))?;
        let main_open_end = page[main_open..]
            .find('>')
            .map(|i| main_open + i + 1)
            .ok_or_else(|| anyhow!("unterminated <main> tag"))?;
        let main_close = page
            .rfind("</main>")
            .ok_or_else(|| anyhow!("page has no </main>"))?;
        if main_close < main_open_end {
            return Err(anyhow!("</main> before <main> ends"));
        }

        // Keep the title block if the template put one at the top of <main>.
        let inner = &page[main_open_end..main_close];
        let keep_until = match inner.find("<header id=\"title-block-header\"") {
            Some(start) => match inner[start..].find("</header>") {
                Some(end) => main_open_end + start + end + "</header>".len(),
                None => return Err(anyhow!("unterminated title block")),
            },
            None => main_open_end,
        };

        Ok(Self {
            prefix: page[..keep_until].to_string(),
            suffix: page[main_close..].to_string(),
        })
    }

    /// The full page with `body` in place of the old content.
    pub fn page(&self, body: &str) -> String {
        let mut out = String::with_capacity(self.prefix.len() + body.len() + self.suffix.len() + 2);
        out.push_str(&self.prefix);
        out.push('\n');
        out.push_str(body);
        out.push('\n');
        out.push_str(&self.suffix);
        out
    }
}

/// Reproduce the one postprocessing step of Quarto's that changes body markup:
/// headings inside a section get `class="anchored" data-anchor-id="<section id>"`,
/// which anchor.js uses for the link icons. Verified as the only content difference
/// between the warm body and Quarto's `<main>`, besides attribute quote style.
pub fn add_heading_anchors(body: &str) -> String {
    static SECTION_HEADING: OnceLock<Regex> = OnceLock::new();
    let re = SECTION_HEADING.get_or_init(|| {
        Regex::new(r#"<section id="([^"]+)"([^>]*)>(\s*)<h([1-6])>"#).expect("valid regex")
    });
    re.replace_all(body, |caps: &regex::Captures| {
        format!(
            r#"<section id="{id}"{attrs}>{gap}<h{level} class="anchored" data-anchor-id="{id}">"#,
            id = &caps[1],
            attrs = &caps[2],
            gap = &caps[3],
            level = &caps[4],
        )
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = concat!(
        "<html><head><title>T</title></head><body>\n",
        "<div id=\"quarto-content\">\n",
        "<main class=\"content\" id=\"quarto-document-content\">\n",
        "\n",
        "<header id=\"title-block-header\" class=\"quarto-title-block default\">\n",
        "<div class=\"quarto-title\"><h1 class=\"title\">T</h1></div>\n",
        "</header>\n",
        "\n",
        "<section id=\"old\" class=\"level2\"><h2>Old</h2><p>old body</p></section>\n",
        "\n",
        "</main>\n",
        "<!-- /main column -->\n",
        "<script>after()</script>\n",
        "</body></html>\n",
    );

    #[test]
    fn keeps_the_shell_and_title_block_and_replaces_the_body() {
        let shell = Shell::from_page(PAGE).unwrap();
        let page = shell.page("<p>new body</p>");
        assert!(page.contains("<title>T</title>"), "head kept");
        assert!(page.contains("id=\"title-block-header\""), "title block kept");
        assert!(page.contains("<p>new body</p>"), "new body inserted");
        assert!(!page.contains("old body"), "old body removed");
        assert!(page.contains("<script>after()</script>"), "after-body script kept");
    }

    #[test]
    fn body_lands_inside_main_after_the_title_block() {
        let page = Shell::from_page(PAGE).unwrap().page("<p>X</p>");
        let header_end = page.find("</header>").unwrap();
        let body = page.find("<p>X</p>").unwrap();
        let main_end = page.find("</main>").unwrap();
        assert!(header_end < body && body < main_end);
    }

    #[test]
    fn works_without_a_title_block() {
        let page = "<body><main class=\"content\"><p>old</p></main><script>s</script></body>";
        let out = Shell::from_page(page).unwrap().page("<p>new</p>");
        assert!(out.contains("<main class=\"content\">\n<p>new</p>\n</main>"));
        assert!(!out.contains("old"));
    }

    #[test]
    fn rejects_pages_without_main() {
        assert!(Shell::from_page("<html><body>no main</body></html>").is_err());
    }

    #[test]
    fn anchors_headings_the_way_quartos_postprocessor_does() {
        let body = "<section id=\"heading\" class=\"level2\">\n<h2>Heading</h2>\n<p>x</p>\n</section>";
        let out = add_heading_anchors(body);
        assert!(out.contains(r#"<h2 class="anchored" data-anchor-id="heading">Heading</h2>"#), "{out}");
        assert!(out.contains(r#"<section id="heading" class="level2">"#), "section untouched");
    }

    #[test]
    fn leaves_headings_outside_sections_alone() {
        let body = "<h2>Loose</h2><p>x</p>";
        assert_eq!(add_heading_anchors(body), body);
    }

    #[test]
    fn anchors_nested_sections_independently() {
        let body = "<section id=\"a\" class=\"level2\"><h2>A</h2>\
                    <section id=\"b\" class=\"level3\"><h3>B</h3></section></section>";
        let out = add_heading_anchors(body);
        assert!(out.contains(r#"<h2 class="anchored" data-anchor-id="a">"#));
        assert!(out.contains(r#"<h3 class="anchored" data-anchor-id="b">"#));
    }
}
