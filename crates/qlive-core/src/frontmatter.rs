//! YAML front matter, split from the body.
//!
//! The warm path renders the body only: Quarto passes front matter to pandoc
//! separately, as a metadata file. So a front-matter edit cannot be served warm — it
//! changes the template, the title block, format options — and has to take the cold
//! path. Splitting the two is how the live preview tells the cases apart.

/// `(front matter, body)`. Front matter is the YAML between an opening `---` on the
/// first line and the next `---` or `...` line, without the fences. Without one, the
/// whole source is the body. Handles `\r\n`.
pub fn split(source: &str) -> (Option<&str>, &str) {
    let mut lines = LineSpans::new(source);
    let Some(first) = lines.next() else {
        return (None, source);
    };
    if first.text(source) != "---" {
        return (None, source);
    }
    for line in lines {
        let text = line.text(source);
        if text == "---" || text == "..." {
            return (Some(&source[first.next..line.start]), &source[line.next..]);
        }
    }
    // Unterminated: not front matter.
    (None, source)
}

/// Whether two document bodies are the same once line endings and surrounding blank
/// lines are ignored. Used to check that Quarto fed pandoc exactly the body we see —
/// engine preprocessing (executed cells, includes) breaks this, and then the warm
/// path must not be used.
pub fn same_body(a: &str, b: &str) -> bool {
    normalise(a) == normalise(b)
}

fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    /// End of the line's text, before any `\r\n` or `\n`.
    end: usize,
    /// Start of the following line.
    next: usize,
}

impl Span {
    fn text<'a>(&self, source: &'a str) -> &'a str {
        source[self.start..self.end].trim_end()
    }
}

struct LineSpans<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> LineSpans<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, position: 0 }
    }
}

impl Iterator for LineSpans<'_> {
    type Item = Span;

    fn next(&mut self) -> Option<Span> {
        if self.position >= self.source.len() {
            return None;
        }
        let start = self.position;
        let rest = &self.source[start..];
        let (end, next) = match rest.find('\n') {
            Some(i) => {
                let end = if i > 0 && rest.as_bytes()[i - 1] == b'\r' { i - 1 } else { i };
                (start + end, start + i + 1)
            }
            None => (self.source.len(), self.source.len()),
        };
        self.position = next;
        Some(Span { start, end, next })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_front_matter_from_body() {
        let (yaml, body) = split("---\ntitle: T\n---\n\n## Body\n");
        assert_eq!(yaml, Some("title: T\n"));
        assert_eq!(body, "\n## Body\n");
    }

    #[test]
    fn handles_windows_line_endings() {
        let (yaml, body) = split("---\r\ntitle: T\r\n---\r\nBody\r\n");
        assert_eq!(yaml, Some("title: T\r\n"));
        assert_eq!(body, "Body\r\n");
    }

    #[test]
    fn accepts_the_yaml_document_end_marker() {
        let (yaml, body) = split("---\ntitle: T\n...\nBody");
        assert_eq!(yaml, Some("title: T\n"));
        assert_eq!(body, "Body");
    }

    #[test]
    fn no_front_matter_means_all_body() {
        assert_eq!(split("## Just a body\n"), (None, "## Just a body\n"));
        assert_eq!(split(""), (None, ""));
    }

    #[test]
    fn unterminated_front_matter_is_not_front_matter() {
        let source = "---\ntitle: never closed\n";
        assert_eq!(split(source), (None, source));
    }

    #[test]
    fn a_rule_later_in_the_document_is_not_front_matter() {
        let source = "Intro\n\n---\n\nMore\n";
        assert_eq!(split(source), (None, source));
    }

    #[test]
    fn same_body_ignores_line_endings_and_surrounding_blank_lines() {
        assert!(same_body("\n\n## H\r\n\r\nText\r\n", "## H\n\nText"));
        assert!(!same_body("## H\n\nText", "## H\n\nOther"));
    }
}
