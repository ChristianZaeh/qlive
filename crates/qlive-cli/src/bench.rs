//! `qlive bench` — the M0 gate.
//!
//! Prints the table the design rests on, measured on whatever machine it runs on, so
//! the premise ("a block costs single-digit milliseconds, Quarto's floor is process
//! startup") is a reproducible claim rather than a remembered one.

use anyhow::Result;
use qlive_core::{Block, BlockKind, Toolchain};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn run(doc: Option<PathBuf>, iters: usize, sections: usize, pandoc_port: u16) -> Result<()> {
    let tools = Toolchain::discover()?;
    let scratch = std::env::temp_dir().join("qlive-bench");
    std::fs::create_dir_all(&scratch)?;

    let doc = match doc {
        Some(given) => qlive_core::absolute(&given)?,
        None => {
            let path = scratch.join("bench.md");
            std::fs::write(&path, fixture_md(sections))?;
            path
        }
    };

    let source = std::fs::read_to_string(&doc)?;
    let blocks = qlive_core::split(&source);

    println!("# qlive bench\n");
    for (name, version) in tools.versions() {
        println!("- {name}: {version}");
    }
    println!(
        "- document: {} — {} lines, {} blocks, mean {:.0} bytes/block",
        doc.display(),
        source.lines().count(),
        blocks.len(),
        mean_bytes(&blocks)
    );
    println!("- warm iterations: {iters}\n");

    println!("| step | ms |");
    println!("|---|---:|");

    // ---- the baseline everyone pays today -----------------------------------
    let cold = qlive_quarto::render(&tools.quarto, &doc, Some("html"))?;
    row(
        "quarto render --to html (cold, Deno included)",
        cold.duration.as_secs_f64() * 1000.0,
    );

    let one_shots = iters.clamp(1, 5);
    let mut total = 0.0;
    for _ in 0..one_shots {
        let started = Instant::now();
        qlive_pandoc::one_shot(&tools.pandoc, &doc, "markdown", "html")?;
        total += elapsed_ms(started);
    }
    row(
        "pandoc one-shot, whole doc (process start)",
        total / one_shots as f64,
    );

    // ---- warm pandoc ---------------------------------------------------------
    let server = qlive_pandoc::PandocServer::start(&tools.pandoc, pandoc_port)?;

    server.convert(&source, "markdown", "html")?; // warm up
    let mut whole = 0.0;
    for _ in 0..iters {
        let started = Instant::now();
        server.convert(&source, "markdown", "html")?;
        whole += elapsed_ms(started);
    }
    row("pandoc server, whole doc", whole / iters as f64);

    let block = blocks
        .iter()
        .find(|b| b.kind == BlockKind::Paragraph)
        .map(|b| b.text.clone())
        .unwrap_or_else(|| "A paragraph with *emphasis*.".to_string());

    server.convert(&block, "markdown", "html")?; // warm up
    let mut single = 0.0;
    for _ in 0..iters {
        let started = Instant::now();
        server.convert(&block, "markdown", "html")?;
        single += elapsed_ms(started);
    }
    let per_block_http = single / iters as f64;
    row("pandoc server, one block", per_block_http);
    drop(server);

    // ---- warm pandoc lua worker: same pandoc, no HTTP hop --------------------
    let mut worker = qlive_pandoc::LuaWorker::start(&tools.pandoc)?;

    worker.render(&source, "markdown", "html")?; // warm up
    let mut whole_lua = 0.0;
    for _ in 0..iters {
        let started = Instant::now();
        worker.render(&source, "markdown", "html")?;
        whole_lua += elapsed_ms(started);
    }
    row("pandoc lua worker, whole doc", whole_lua / iters as f64);

    worker.render(&block, "markdown", "html")?; // warm up
    let mut single_lua = 0.0;
    for _ in 0..iters {
        let started = Instant::now();
        worker.render(&block, "markdown", "html")?;
        single_lua += elapsed_ms(started);
    }
    let per_block = single_lua / iters as f64;
    row("pandoc lua worker, one block", per_block);
    drop(worker);

    // ---- typst ---------------------------------------------------------------
    match &tools.typst {
        Some(typst) => {
            let typ = scratch.join("bench.typ");
            let pdf = scratch.join("bench.pdf");
            std::fs::write(&typ, qlive_typst::fixture(sections))?;

            let cold_typst = qlive_typst::one_shot(typst, &typ, &pdf)?;
            row(
                "typst compile, one-shot (process start)",
                cold_typst.as_secs_f64() * 1000.0,
            );

            match typst_watch(typst, &typ, &pdf, iters.clamp(1, 5)) {
                Ok((roundtrip, reported)) => {
                    row("typst watch, edit -> new PDF (end to end)", roundtrip);
                    match reported {
                        Some(ms) => row("  of which typst reports as compile", ms),
                        None => println!("|   of which typst reports as compile | not reported |"),
                    }
                }
                Err(e) => println!("| typst watch, edit -> new PDF | n/a — {e} |"),
            }
        }
        None => println!("| typst | not found |"),
    }

    // ---- the premise ---------------------------------------------------------
    let edited = source.replacen("Paragraph 1 ", "Paragraph one ", 1);
    let dirty = if edited == source {
        // Caller supplied their own document: edit the first paragraph block instead.
        let mut copy = source.clone();
        if let Some(first) = blocks.iter().find(|b| b.kind == BlockKind::Paragraph) {
            copy = source.replacen(&first.text, &format!("{} (edited)", first.text), 1);
        }
        qlive_core::dirty_count(&source, &copy)
    } else {
        qlive_core::dirty_count(&source, &edited)
    };

    println!();
    println!("A one-word edit dirties {dirty} of {} blocks.", blocks.len());
    println!(
        "So the hot path has to re-render {dirty} block(s): ~{:.0} ms, against {:.0} ms for\n\
         the cold `quarto render` it replaces.",
        per_block * dirty as f64,
        cold.duration.as_secs_f64() * 1000.0
    );
    println!();
    println!("M0 gate: a save costs one `quarto render`, not a `quarto preview` restart.");
    println!("M2 gate: p50 under 25 ms for a prose edit, output identical to quarto render.");

    Ok(())
}

/// Generous budget: typst's own rebuild is single-digit milliseconds, but its first
/// compile builds a font cache, and on a loaded machine that can take a while.
const TYPST_BUDGET: Duration = Duration::from_secs(60);

/// Returns (mean end-to-end ms, typst's own last reported compile ms).
fn typst_watch(typst: &Path, typ: &Path, pdf: &Path, rounds: usize) -> Result<(f64, Option<f64>)> {
    let mut watch = qlive_typst::TypstWatch::start(typst, typ, pdf)?;
    watch.wait_ready(TYPST_BUDGET)?;

    let mut total = 0.0;
    for i in 0..rounds {
        total += watch.time_edit(&format!("\nEdit {i}\n"), TYPST_BUDGET)?;
    }
    Ok((total / rounds as f64, watch.reported_ms()))
}

/// 40 sections is ~283 lines, the document the design note's numbers came from.
fn fixture_md(sections: usize) -> String {
    let mut out = String::from("---\ntitle: Bench doc\n---\n");
    for i in 1..=sections {
        out.push_str(&format!(
            "\n## Section {i}\n\nParagraph {i} with *emphasis*, `code`, a [link](https://example.com) and math $a_{i}^2 + b^2 = c^2$.\n\n- item one\n- item two\n"
        ));
    }
    out
}

fn mean_bytes(blocks: &[Block]) -> f64 {
    if blocks.is_empty() {
        return 0.0;
    }
    blocks.iter().map(|b| b.text.len()).sum::<usize>() as f64 / blocks.len() as f64
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn row(label: &str, ms: f64) {
    println!("| {label} | {ms:.1} |");
}
