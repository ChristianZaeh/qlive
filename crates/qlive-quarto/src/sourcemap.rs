//! Preview-only source IDs. These annotations are excluded from HTML parity comparisons.
use kuchiki::{traits::TendrilSink, NodeRef};

pub fn annotate(html: &str, count: usize, page: bool) -> (String, bool) {
    let doc = kuchiki::parse_html().one(if page { html.to_owned() } else {
        format!("<!doctype html><html><head></head><body>{html}</body></html>")
    });
    let cells: Vec<NodeRef> = doc.select(".cell").unwrap().map(|n| n.as_node().clone())
        .filter(|n| !n.ancestors().any(|parent| parent.as_element().is_some_and(|e|
            e.attributes.borrow().get("class").is_some_and(|s| s.split_ascii_whitespace().any(|c| c == "cell")))))
        .collect();
    // Avoid guessing if an extension or filter changed the relationship to source cells.
    if cells.len() != count { return (html.to_owned(), false); }
    for (id, cell) in cells.iter().enumerate() {
        cell.as_element().unwrap().attributes.borrow_mut().insert("data-qlive-source", id.to_string());
    }
    let out = if page { doc.to_string() } else {
        doc.select_first("body").unwrap().as_node().children().map(|n| n.to_string()).collect()
    };
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_outer_cells_only_and_refuses_ambiguous_counts() {
        let body = "<div class='cell'><div class='cell'>Nested output</div></div><div class='cell'>Second</div>";
        let (out, mapped) = annotate(body, 2, false);
        assert!(mapped);
        assert_eq!(out.matches("data-qlive-source=").count(), 2);
        assert_eq!(annotate(body, 3, false), (body.to_owned(), false));
    }
}
