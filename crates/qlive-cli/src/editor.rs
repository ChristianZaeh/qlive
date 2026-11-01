//! Stateless native editor operations. The VS Code adapter only applies returned edits.
use anyhow::{anyhow, Result};
use qlive_core::cells;
use serde_json::{json, Value};
use std::io::Read;

pub fn run() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().take(8 * 1024 * 1024).read_to_string(&mut input)?;
    println!("{}", request(serde_json::from_str(&input)?)?);
    Ok(())
}

fn request(input: Value) -> Result<Value> {
    let source = input["text"].as_str().unwrap_or("");
    let ranges = cells::code_ranges(source);
    let line = input["line"].as_u64().unwrap_or(0) as usize;
    match input["op"].as_str() {
        Some("cells") => Ok(serde_json::to_value(ranges)?),
        Some("navigate") => {
            let target = match input["direction"].as_str() {
                Some("next") => ranges.iter().find(|range| range.start_line > line),
                Some("previous") => ranges.iter().rev().find(|range| range.start_line + 1 < line),
                _ => return Err(anyhow!("expected next or previous direction")),
            };
            Ok(json!({"line": target.map(|r| (r.start_line + 1).min(source.lines().count().saturating_sub(1)))}))
        }
        Some("insert-cell") => {
            let language = input["language"].as_str().filter(|s| !s.is_empty())
                .or_else(|| ranges.first().map(|r| r.language.as_str())).unwrap_or("python");
            if !language.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+')) {
                return Err(anyhow!("invalid cell language"));
            }
            let target_line = ranges.iter().find(|r| r.start_line <= line && r.end_line >= line)
                .map(|r| r.end_line + 1).unwrap_or(line);
            let byte = source.split_inclusive('\n').take(target_line).map(str::len).sum::<usize>().min(source.len());
            let offset = source[..byte].encode_utf16().count();
            let prefix = if byte > 0 && !source[..byte].ends_with('\n') { "\n" } else { "" };
            let opening = format!("{prefix}\n```{{{language}}}\n");
            Ok(json!({"offset": offset, "text": format!("{opening}\n```\n"),
                "cursorOffset": offset + opening.encode_utf16().count()}))
        }
        Some("new-document") => {
            let title = serde_json::to_string(input["title"].as_str().unwrap_or("Untitled"))?;
            Ok(json!({"text": format!("---\ntitle: {title}\nformat: html\n---\n\n## Introduction\n\n")}))
        }
        _ => Err(anyhow!("unknown editor operation")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_and_navigation_use_executable_fences_only() {
        let source = "# Text\n\n```python\nexample\n```\n\n```{python}\nx = 1\n```\n\n~~~{r}\ny <- 1\n~~~\n";
        let ranges = request(json!({"op": "cells", "text": source})).unwrap();
        assert_eq!(ranges.as_array().unwrap().len(), 2);
        assert_eq!(ranges[0]["start_line"], 6);
        assert_eq!(request(json!({"op": "navigate", "text": source, "line": 7, "direction": "next"})).unwrap()["line"], 11);
        assert_eq!(request(json!({"op": "navigate", "text": source, "line": 11, "direction": "previous"})).unwrap()["line"], 7);
    }

    #[test]
    fn insertion_uses_utf16_and_stays_outside_the_current_cell() {
        let source = "Emoji 🦀\n\n```{python}\nx = 1\n```";
        let edit = request(json!({"op": "insert-cell", "text": source, "line": 3})).unwrap();
        assert_eq!(edit["offset"].as_u64().unwrap() as usize, source.encode_utf16().count());
        let combined = format!("{source}{}", edit["text"].as_str().unwrap());
        assert_eq!(cells::code_ranges(&combined).len(), 2);
        assert_eq!(edit["cursorOffset"].as_u64().unwrap() as usize,
            source.encode_utf16().count() + "\n\n```{python}\n".len());
    }
}
