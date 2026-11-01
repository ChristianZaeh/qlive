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

/// Compatibility helper for callers that need only anchors.
pub fn add_heading_anchors(body: &str) -> String {
    crate::postprocess::process(body, &crate::postprocess::Options {
        code_copy: false,
        ..Default::default()
    })
}

/// The content of a Quarto page's `<main>`, without the title block — i.e. the part
/// the warm path produces. `None` if the page has no `<main>`.
pub fn main_content(page: &str) -> Option<String> {
    let open = page.find("<main")?;
    let open_end = open + page[open..].find('>')? + 1;
    let close = page.rfind("</main>")?;
    if close < open_end {
        return None;
    }
    static TITLE_BLOCK: OnceLock<Regex> = OnceLock::new();
    let title = TITLE_BLOCK.get_or_init(|| {
        Regex::new(r#"(?s)<header id="title-block-header".*?</header>"#).expect("valid regex")
    });
    Some(title.replace(&page[open_end..close], "").into_owned())
}

/// Compare DOM serialization, ignoring attribute order and ordinary prose
/// whitespace while preserving whitespace in code and other preformatted content.
pub fn normalize_html(html: &str) -> String {
    crate::postprocess::comparison_html(html)
}

/// Version every local image by a hash of its bytes: `src="a.png"` becomes
/// `src="a.png?v=1f2e…"`.
///
/// A re-run cell rewrites its figure in place, at the same path. Swapped into the
/// page under the same URL, the browser shows its cached copy — the new code next to
/// the old plot. Hashing the contents changes the URL exactly when the image changed,
/// so unchanged figures stay cached. This is applied to the HTML sent to the
/// browser, not to the markdown, so the self-check against Quarto is unaffected.
pub fn version_images(body: &str, root: &std::path::Path) -> String {
    static IMG_SRC: OnceLock<Regex> = OnceLock::new();
    let re = IMG_SRC.get_or_init(|| Regex::new(r#"(<img\b[^>]*?\bsrc=")([^"?#]+)(")"#).expect("valid regex"));
    re.replace_all(body, |caps: &regex::Captures| {
        let src = &caps[2];
        let local = !(src.contains("://") || src.starts_with("data:") || src.starts_with('/'));
        match std::fs::read(root.join(src)) {
            Ok(bytes) if local => format!("{}{src}?v={:016x}{}", &caps[1], fnv1a(&bytes), &caps[3]),
            _ => caps[0].to_string(),
        }
    })
    .into_owned()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("qlive-shell-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("figs")).unwrap();
        dir
    }

    #[test]
    fn a_changed_figure_gets_a_new_url_and_an_unchanged_one_keeps_it() {
        let dir = scratch("versions");
        let body = r#"<p><img src="figs/a.png" class="img-fluid"></p>"#;

        std::fs::write(dir.join("figs/a.png"), b"first plot").unwrap();
        let first = version_images(body, &dir);
        assert_eq!(first, version_images(body, &dir), "same bytes, same URL: stays cached");

        std::fs::write(dir.join("figs/a.png"), b"second plot").unwrap();
        let second = version_images(body, &dir);
        assert_ne!(first, second, "new bytes, new URL: the browser must refetch");
        assert!(second.contains(r#"src="figs/a.png?v="#), "{second}");
        assert!(second.contains(r#"class="img-fluid""#), "other attributes kept");
    }

    #[test]
    fn main_content_drops_the_title_block_and_the_shell() {
        let content = main_content(PAGE).unwrap();
        assert!(content.contains("old body"));
        assert!(!content.contains("title-block-header"));
        assert!(!content.contains("<script>"));
    }

    #[test]
    fn h1_is_never_anchored() {
        let body = "<section id=\"top\" class=\"level1\"><h1>Top</h1></section>";
        assert_eq!(normalize_html(&add_heading_anchors(body)), normalize_html(body));
    }

    #[test]
    fn normalisation_ignores_serialisation_only_differences() {
        assert_eq!(
            normalize_html(r#"<iframe allowfullscreen></iframe><kbd aria-hidden="true" >K</kbd>"#),
            normalize_html(r#"<iframe allowfullscreen=""></iframe><kbd aria-hidden="true">K</kbd>"#)
        );
    }

    #[test]
    fn normalisation_ignores_quote_style_and_whitespace_only() {
        assert_eq!(
            normalize_html("<i class='icon'></i>\n\n  <p>a   b</p>"),
            normalize_html("<i class=\"icon\"></i><p>a b</p>")
        );
        assert_ne!(normalize_html("<p>a b</p>"), normalize_html("<p>a c</p>"));
    }

    #[test]
    fn remote_data_and_missing_images_are_left_alone() {
        let dir = scratch("untouched");
        let body = r#"<img src="https://x.org/a.png"><img src="data:image/png;base64,AA"><img src="nope.png">"#;
        assert_eq!(version_images(body, &dir), body);
    }

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
        assert!(out.contains(r#"<section class="level2" id="heading">"#), "section untouched");
    }

    #[test]
    fn anchors_headings_outside_sections_too() {
        let body = "<h2>Loose</h2><p>x</p>";
        assert!(add_heading_anchors(body).contains("class=\"anchored\""));
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
