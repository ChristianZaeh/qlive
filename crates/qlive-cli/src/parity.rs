//! `qlive parity <dir>` — does the warm path produce what Quarto produces?
//!
//! For every eligible document: render it with Quarto (captured), render the body
//! through qlive's warm pipeline, and compare against the content of Quarto's
//! `<main>`. Built for Quarto's own test corpus (`tests/docs/smoke-all`).
//!
//! Each document lands in exactly one bucket, and only one bucket is a real problem:
//!
//! * `pass`     — the warm output equals Quarto's.
//! * `fallback` — the self-check would turn the warm path off, so qlive cold-renders:
//!                slower, never wrong.
//! * **`fail`** — the self-check passes but the output differs: qlive would show a
//!                wrong preview. This is the bucket that matters.
//! * `error`    — Quarto itself could not render the document as HTML.
//! * `skipped`  — not run at all; see below.
//!
//! **Nothing from the corpus is executed.** Documents with code cells, local filters
//! or extensions, or an engine declaration are skipped: they are third-party code,
//! and the warm path for plain documents does not need them to be tested.

use anyhow::{anyhow, Result};
use qlive_core::{cells, frontmatter, Toolchain};
use qlive_pandoc::LuaWorker;
use qlive_quarto::shell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

enum Outcome {
    Pass,
    Fallback,
    Fail(String),
    Error(String),
}

pub fn run(dir: PathBuf, limit: Option<usize>, report: Option<PathBuf>) -> Result<()> {
    let tools = Toolchain::discover()?;
    let root = qlive_core::absolute(&dir)?;

    let mut all = Vec::new();
    collect(&root, &mut all)?;
    all.sort();

    let mut skipped: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut eligible = Vec::new();
    for doc in all {
        match skip_reason(&doc, &root) {
            Some(reason) => *skipped.entry(reason).or_default() += 1,
            None => eligible.push(doc),
        }
    }
    if let Some(n) = limit {
        eligible.truncate(n);
    }

    println!(
        "qlive parity: {} documents eligible, {} skipped",
        eligible.len(),
        skipped.values().sum::<usize>()
    );

    let started = Instant::now();
    let mut results: Vec<(PathBuf, Outcome)> = Vec::new();
    for (i, doc) in eligible.iter().enumerate() {
        let outcome = check(&tools, doc).unwrap_or_else(|e| Outcome::Error(one_line(&e.to_string())));
        let tag = match &outcome {
            Outcome::Pass => "pass",
            Outcome::Fallback => "fallback",
            Outcome::Fail(_) => "FAIL",
            Outcome::Error(_) => "error",
        };
        println!("[{:>4}/{}] {tag:<8} {}", i + 1, eligible.len(), relative(doc, &root));
        results.push((doc.clone(), outcome));
    }

    let count = |f: fn(&Outcome) -> bool| results.iter().filter(|(_, o)| f(o)).count();
    let passes = count(|o| matches!(o, Outcome::Pass));
    let fallbacks = count(|o| matches!(o, Outcome::Fallback));
    let fails = count(|o| matches!(o, Outcome::Fail(_)));
    let errors = count(|o| matches!(o, Outcome::Error(_)));

    let mut text = String::new();
    text.push_str(&format!(
        "# qlive parity\n\nCorpus: `{}`. {} documents run in {:.0} s.\n\n",
        root.display(),
        results.len(),
        started.elapsed().as_secs_f64()
    ));
    text.push_str("| bucket | documents | meaning |\n|---|---:|---|\n");
    text.push_str(&format!("| pass | {passes} | warm output equals Quarto's |\n"));
    text.push_str(&format!("| fallback | {fallbacks} | self-check turns warm off: slower, never wrong |\n"));
    text.push_str(&format!("| **fail** | **{fails}** | **warm output differs: a wrong preview** |\n"));
    text.push_str(&format!("| error | {errors} | Quarto could not render it as HTML |\n\n"));
    text.push_str("Skipped, never executed:\n\n");
    for (reason, n) in &skipped {
        text.push_str(&format!("- {reason}: {n}\n"));
    }

    let failures: Vec<_> = results
        .iter()
        .filter_map(|(d, o)| match o {
            Outcome::Fail(why) => Some((d, why)),
            _ => None,
        })
        .collect();
    if !failures.is_empty() {
        let mut by_cause: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (_, why) in &failures {
            *by_cause.entry(cause(why)).or_default() += 1;
        }
        let mut causes: Vec<_> = by_cause.into_iter().collect();
        causes.sort_by(|a, b| b.1.cmp(&a.1));
        text.push_str("\n## Failures by cause\n\n| cause | documents |\n|---|---:|\n");
        for (why, n) in causes {
            text.push_str(&format!("| {why} | {n} |\n"));
        }
        text.push_str("\n## Failures\n\n");
        for (doc, why) in &failures {
            text.push_str(&format!("- `{}` [{}]: {why}\n", relative(doc, &root), cause(why)));
        }
    }
    let errored: Vec<_> = results
        .iter()
        .filter_map(|(d, o)| match o {
            Outcome::Error(why) => Some((d, why)),
            _ => None,
        })
        .collect();
    if !errored.is_empty() {
        text.push_str("\n## Errors\n\n");
        for (doc, why) in &errored {
            text.push_str(&format!("- `{}`: {why}\n", relative(doc, &root)));
        }
    }

    println!("\n{text}");
    if let Some(path) = report {
        std::fs::write(&path, &text)?;
        println!("report written to {}", path.display());
    }
    Ok(())
}

/// Render with Quarto, render warm, compare.
fn check(tools: &Toolchain, doc: &Path) -> Result<Outcome> {
    let page_file = qlive_quarto::output_path(doc, Some("html"));
    let files_dir = doc.with_file_name(format!(
        "{}_files",
        doc.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    ));
    // Only clean up what this run created.
    let page_existed = page_file.exists();
    let files_existed = files_dir.exists();

    let outcome = compare(tools, doc, &page_file);

    if !page_existed {
        let _ = std::fs::remove_file(&page_file);
    }
    if !files_existed {
        let _ = std::fs::remove_dir_all(&files_dir);
    }
    outcome
}

fn compare(tools: &Toolchain, doc: &Path, page_file: &Path) -> Result<Outcome> {
    let capture = qlive_quarto::capture::capture_with(&tools.quarto, doc, Some("html"), &["--no-execute"])?;
    let page = std::fs::read_to_string(page_file)
        .map_err(|e| anyhow!("no HTML output at {}: {e}", page_file.display()))?;
    let quartos = shell::main_content(&page).ok_or_else(|| anyhow!("Quarto's page has no <main>"))?;

    let raw = std::fs::read_to_string(doc)?;
    let source = qlive_core::includes::expand(&raw, doc)?.text;
    let (_, body) = frontmatter::split(&source);
    if !frontmatter::same_body(body, &capture.input) {
        return Ok(Outcome::Fallback);
    }

    let mut worker = LuaWorker::start_quarto(&tools.pandoc, &capture.env, &capture.cwd, &capture.format, capture.citeproc)?;
    let rendered = worker.render_quarto(body, &capture.metadata)?;
    if rendered.has_fatal() {
        return Ok(Outcome::Error(format!(
            "FATAL from Quarto's filters in the warm path: {}",
            one_line(&rendered.diagnostics.join(" | "))
        )));
    }
    let options = qlive_quarto::postprocess::Options::from_capture(&capture.metadata, &capture.env)?.with_cold_page(&page);
    let ours = qlive_quarto::postprocess::process(&rendered.out, &options);

    let (a, b) = (shell::normalize_html(&quartos), shell::normalize_html(&ours));
    Ok(match first_difference(&a, &b) {
        None => Outcome::Pass,
        Some(diff) => Outcome::Fail(diff),
    })
}

/// Where the two first diverge, with a little context, on one line.
fn first_difference(quartos: &str, ours: &str) -> Option<String> {
    if quartos == ours {
        return None;
    }
    let at = quartos
        .char_indices()
        .zip(ours.chars())
        .find(|((_, a), b)| a != b)
        .map(|((i, _), _)| i)
        .unwrap_or_else(|| quartos.len().min(ours.len()));
    let excerpt = |s: &str| {
        let start = s.floor_char_boundary(at.saturating_sub(40));
        let end = s.ceil_char_boundary((at + 60).min(s.len()));
        s[start..end].replace('`', "'")
    };
    Some(format!("Quarto: `…{}…` qlive: `…{}…`", excerpt(quartos), excerpt(ours)))
}

/// Which of Quarto's postprocessing steps (or what else) a difference comes from,
/// judged from the text around the first divergence.
fn cause(diff: &str) -> &'static str {
    let quartos = diff.split(" qlive: ").next().unwrap_or(diff);
    if quartos.contains("code-copy") || quartos.contains("code-with-copy") {
        "code-copy scaffold (postprocessor)"
    } else if quartos.contains("quarto-appendix") || quartos.contains("appendix") {
        "appendix: citations, footnotes, references (postprocessor)"
    } else if quartos.contains("anchored") || quartos.contains("data-anchor-id") {
        "heading/figure/table anchors (postprocessor)"
    } else if quartos.contains("quarto-float") || quartos.contains("<figure") || quartos.contains("figure") {
        "figure markup (postprocessor)"
    } else if quartos.contains("<table") || quartos.contains("table") {
        "table markup (postprocessor)"
    } else if quartos.contains("callout") {
        "callout markup"
    } else if quartos.contains("quarto-xref") || quartos.contains("tooltip") {
        "cross-reference markup"
    } else {
        "other"
    }
}

/// Why a document is not run, or `None` to run it. Nothing skipped is executed.
fn skip_reason(doc: &Path, root: &Path) -> Option<&'static str> {
    let Ok(raw) = std::fs::read_to_string(doc) else {
        return Some("unreadable");
    };
    // Expanded, so a code cell or filter arriving through an include also counts.
    let source = match qlive_core::includes::expand(&raw, doc) {
        Ok(expanded) => expanded.text,
        Err(_) => return Some("include cannot be resolved"),
    };
    // Quarto executes embedded qmd/ipynb documents as a separate render, even if
    // the parent itself has no executable cells. Never enter that path in a corpus run.
    if source.to_ascii_lowercase().split("{{<").skip(1).any(|s| s.trim_start().strip_prefix("embed")
        .is_some_and(|rest| rest.starts_with(char::is_whitespace) || rest.starts_with('>'))) {
        return Some("embedded documents (may execute code)");
    }
    let nested_code = source.lines().any(|line| {
        let line = line.trim_start();
        let fence = line.chars().take_while(|c| matches!(c, '`' | '~')).count();
        if fence < 3 { return false; }
        let info = line[fence..].trim_start();
        info.starts_with('{') && !info.starts_with("{.") && !info.starts_with("{=") && !info.starts_with("{#")
    });
    if nested_code || cells::from_markdown(&source).iter().any(cells::Cell::is_code) {
        return Some("code cells (third-party code, not executed)");
    }
    let (front, _) = frontmatter::split(&source);
    let front = front.unwrap_or("");
    if front.contains("filters:") || front.contains("shortcodes:") {
        return Some("local filters or shortcodes (third-party Lua, not executed)");
    }
    if ["engine:", "jupyter:", "knitr:", "julia:"].iter().any(|k| front.contains(k)) {
        return Some("declares an engine");
    }
    for dir in doc.ancestors().skip(1) {
        if dir.join("_extensions").is_dir() {
            return Some("local extensions (third-party Lua, not executed)");
        }
        if dir.join("_quarto.yml").is_file() || dir.join("_quarto.yaml").is_file() {
            return Some("inside a Quarto project");
        }
        if dir == root {
            break;
        }
    }
    None
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if path.is_dir() {
            if name.starts_with('.') || name.starts_with('_') || name.ends_with("_files") {
                continue;
            }
            collect(&path, out)?;
        } else if name.ends_with(".qmd") && !name.contains(".qlive.") {
            out.push(path);
        }
    }
    Ok(())
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() > 300 {
        format!("{}…", &flat[..flat.floor_char_boundary(300)])
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_skips_embeds_and_cells_inside_divs_without_rendering() -> Result<()> {
        let root = std::env::temp_dir().join(format!("qlive-skip-test-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("test.qmd");
        for source in ["{{< embed child.qmd >}}", "{{<\nEMBED child.ipynb >}}"] {
            std::fs::write(&path, source)?;
            assert_eq!(skip_reason(&path, &root), Some("embedded documents (may execute code)"));
        }
        std::fs::write(&path, "::: {.callout-note}\n\n```{python}\nraise RuntimeError()\n```\n\n:::\n")?;
        assert_eq!(skip_reason(&path, &root), Some("code cells (third-party code, not executed)"));
        Ok(())
    }

    #[test]
    #[ignore = "renders an authored large document with installed Quarto"]
    fn paragraph_patches_match_full_quarto_and_measure_worker_cost() -> Result<()> {
        let tools = Toolchain::discover()?;
        let root = std::env::temp_dir().join(format!("qlive-incremental-test-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("large.qmd");
        let mut source = "---\ntitle: Paragraph parity\nformat: html\n---\n\n".to_owned();
        for i in 0..300 { source.push_str(&format!("## Section {i}\n\nParagraph {i} with **bold** words and ordinary text.\n\n")); }
        std::fs::write(&path, &source)?;
        let capture = qlive_quarto::capture::capture(&tools.quarto, &path, Some("html"))?;
        let page = std::fs::read_to_string(path.with_extension("html"))?;
        let options = qlive_quarto::postprocess::Options::from_capture(&capture.metadata, &capture.env)?.with_cold_page(&page);
        let mut worker = LuaWorker::start_quarto(&tools.pandoc, &capture.env, &capture.cwd, &capture.format, capture.citeproc)?;
        let initial = worker.render_quarto(frontmatter::split(&source).1, &capture.metadata)?;
        let html = qlive_quarto::postprocess::process(&initial.out, &options);
        let mut document = qlive_quarto::incremental::Document::prepare(&source, &html).expect("eligible fixture");
        let mut partial_ms = Vec::new();
        let mut full_ms = Vec::new();
        for i in 0..5 {
            let edited = source.replace("Paragraph 150 with", &format!("Edited paragraph {i} with"));
            let change = document.change(&edited).expect("one paragraph changed");
            let partial = worker.render_quarto(&change.text, &capture.metadata)?;
            assert!(!partial.has_fatal());
            partial_ms.push(partial.ms);
            document.replace(change, &partial.out).expect("one paragraph returned");
            let full = worker.render_quarto(frontmatter::split(&edited).1, &capture.metadata)?;
            full_ms.push(full.ms);
            let expected = qlive_quarto::postprocess::process(&full.out, &options);
            let expected = qlive_quarto::incremental::Document::prepare(&edited, &expected).unwrap();
            assert_eq!(shell::normalize_html(&document.html), shell::normalize_html(&expected.html));
        }
        partial_ms.sort_by(f64::total_cmp);
        full_ms.sort_by(f64::total_cmp);
        println!("300 sections, 5 edits: paragraph worker median {:.1} ms; full worker median {:.1} ms; identical HTML", partial_ms[2], full_ms[2]);
        Ok(())
    }

    #[test]
    #[ignore = "renders authored fixtures with installed Quarto"]
    fn postprocessor_matches_quarto_features_and_themes() -> Result<()> {
        let tools = Toolchain::discover()?;
        let root = std::env::temp_dir().join(format!("qlive-postprocess-test-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let cases = [
            ("footnotes", "", "A note[^1].\n\n[^1]: Current footnote.\n"),
            ("plain-appendix", "appendix-style: plain\n", "A note[^1].\n\n[^1]: Current footnote.\n"),
            ("no-appendix", "appendix-style: none\n", "A note[^1].\n\n[^1]: Current footnote.\n"),
            ("localized-notes", "lang: de\n", "A note[^1].\n\n[^1]: Current footnote.\n"),
            ("authored-appendix", "", "## Body\n\nText.\n\n## Extra {.appendix}\n\nDetails.\n"),
            ("citation-appendix", "title: Citation fixture\ncitation: true\ndate: 2023-03-01\n", "## Body\n\nText.\n"),
            ("bootstrap", "", "> A quotation\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n![Caption](data:image/png;base64,AA==)\n"),
            ("unthemed", "theme: none\n", "## Heading\n\n> A quotation\n"),
            ("pandoc", "theme: pandoc\n", "## Heading\n\n> A quotation\n"),
            ("annotations", "", "```python\nx = 1 # <1>\n```\n\n1. Explanation\n"),
            ("hover", "code-annotations: hover\n", "```python\nx = 1 # <1>\n```\n\n1. Explanation\n"),
            ("select", "code-annotations: select\n", "```python\nx = 1 # <1>\n```\n\n1. Explanation\n"),
            ("disabled", "code-annotations: false\ncode-copy: false\nanchor-sections: false\n", "## Heading\n\n```python\nx = 1 # <1>\n```\n\n1. Explanation\n"),
        ];
        for (name, metadata, body) in cases {
            let path = root.join(format!("{name}.qmd"));
            std::fs::write(&path, format!("---\nformat: html\n{metadata}---\n\n{body}"))?;
            if matches!(name, "unthemed" | "pandoc") {
                // These formats have no <main>, so the live shell cannot warm-render
                // them yet. Verify the actual captured flag independently.
                let capture = qlive_quarto::capture::capture(&tools.quarto, &path, Some("html"))?;
                let options = qlive_quarto::postprocess::Options::from_capture(&capture.metadata, &capture.env)?;
                assert!(!options.bootstrap, "{name}");
                continue;
            }
            match check(&tools, &path)? {
                Outcome::Pass => (),
                Outcome::Fail(diff) => panic!("{name}: {diff}"),
                Outcome::Error(error) => panic!("{name}: {error}"),
                Outcome::Fallback => panic!("{name}: unexpected fallback"),
            }
        }
        Ok(())
    }
}
