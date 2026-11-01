//! `qlive live` — warm Quarto renders spliced into the cold page, pushed without
//! reload; with code cells, only the cells that changed are run.
//!
//! **Plain documents.** One cold `quarto render` (with capture) yields the page, its
//! shell, and Quarto's pipeline to host warm. A save re-renders the body through
//! Quarto's own reader and filters, adds the heading anchors Quarto's postprocessor
//! would, splices it into the shell and pushes it; the browser swaps `<main>`.
//!
//! **Documents with code cells.** Quarto never executes anything. One kernel —
//! shared, in order, by every document in the chain (`--after`), so variables carry
//! from chapter to chapter — runs only the cells whose state could have changed. The
//! cold render hands Quarto *our* notebook with `--no-execute`; each warm save turns
//! the cell outputs into the markdown Quarto's Jupyter engine would produce and runs
//! that through the warm pipeline.
//!
//! The cold path is taken whenever the warm one could be wrong: the front matter
//! changed, Quarto's filters reported FATAL, or the self-check failed. The self-check
//! compares what we would feed pandoc with what Quarto actually fed it on the cold
//! render — for plain documents the body, for cell documents the engine markdown —
//! and any difference turns the warm path off for that document.

use crate::session::{read_expanded, run_chain, start_session, Chain, Pass, Session};
use anyhow::{anyhow, Result};
use notify::{RecursiveMode, Watcher};
use qlive_core::cells::{self, Cell};
use qlive_core::{frontmatter, Toolchain};
use qlive_pandoc::LuaWorker;
use qlive_quarto::engine;
use qlive_quarto::shell::{self, Shell};
use qlive_quarto::Capture;
use qlive_server::Preview;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Editors touch a file several times per save.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Everything derived from one cold render.
struct Cold {
    capture: Capture,
    shell: Shell,
    page: String,
    /// What `/` serves on disk: the `.html` Quarto wrote.
    page_file: PathBuf,
    front_matter: Option<String>,
    /// `None` when the self-check says the warm path would not match Quarto.
    warm: Option<LuaWorker>,
}

struct Live {
    tools: Toolchain,
    chain: Chain,
    /// `Some` when the target document has code cells.
    session: Option<Session>,
    cold: Cold,
}

pub async fn run(doc: PathBuf, after: Vec<PathBuf>, port: u16) -> Result<()> {
    let tools = Toolchain::discover()?;
    let mut docs = Vec::with_capacity(after.len() + 1);
    for earlier in &after {
        docs.push(qlive_core::absolute(earlier)?);
    }
    docs.push(qlive_core::absolute(&doc)?);
    let chain = Chain { docs };

    let target_source = read_expanded(chain.target())?.text;
    let has_cells = cells::from_markdown(&target_source).iter().any(Cell::is_code);

    let (session, cold) = if has_cells {
        let mut session = start_session(&tools, chain.target())?;
        let pass = run_chain(&mut session, &chain)?;
        report_pass("first run", &pass);
        let cold = cold_cells(&tools, chain.target(), &pass)?;
        (Some(session), cold)
    } else {
        if !after.is_empty() {
            println!("qlive: --after ignored: the previewed document has no code cells");
        }
        (None, cold_plain(&tools, chain.target())?)
    };

    let page_file = cold.page_file.clone();
    let root = page_file
        .parent()
        .ok_or_else(|| anyhow!("render output has no parent directory"))?
        .to_path_buf();
    let entry = page_file
        .file_name()
        .ok_or_else(|| anyhow!("render output has no file name"))?
        .to_string_lossy()
        .to_string();
    let preview = Preview::new(root, entry);
    preview.set_page(cold.page.clone());

    let live = Live {
        tools,
        chain,
        session,
        cold,
    };
    {
        let preview = preview.clone();
        std::thread::spawn(move || {
            if let Err(e) = watch(live, &preview) {
                eprintln!("qlive: watcher stopped: {e}");
            }
        });
    }

    println!("qlive: http://127.0.0.1:{port}/  (ctrl-c to stop)");
    qlive_server::serve(preview, port).await
}

// ---------------------------------------------------------------------------------
// notebooks

/// `doc.qmd` -> `doc.qlive`: the stem of the notebook Quarto renders, and therefore
/// of its figure directory, `doc.qlive_files`.
fn notebook_stem(doc: &Path) -> String {
    let stem = doc
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());
    format!("{stem}.qlive")
}

// ---------------------------------------------------------------------------------
// cold renders

fn cold_plain(tools: &Toolchain, doc: &Path) -> Result<Cold> {
    let started = Instant::now();
    let capture = qlive_quarto::capture::capture(&tools.quarto, doc, Some("html"))?;
    let page_file = qlive_quarto::output_path(doc, Some("html"));
    let page = std::fs::read_to_string(&page_file)?;
    let shell = Shell::from_page(&page)?;

    let source = read_expanded(doc)?.text;
    let (front, body) = frontmatter::split(&source);

    let warm = if frontmatter::same_body(body, &capture.input) {
        Some(LuaWorker::start_quarto(
            &tools.pandoc,
            &capture.env,
            &capture.cwd,
            &capture.format,
        )?)
    } else {
        println!("qlive: warm path off: Quarto preprocessed the body in a way qlive does not reproduce");
        None
    };
    report_cold(started, warm.is_some());

    Ok(Cold {
        capture,
        shell,
        page,
        page_file,
        front_matter: front.map(str::to_string),
        warm,
    })
}

/// Hand Quarto our notebook with `--no-execute`, so it renders our outputs and never
/// runs a cell; then check that our engine markdown equals what Quarto made of it.
fn cold_cells(tools: &Toolchain, doc: &Path, pass: &Pass) -> Result<Cold> {
    let started = Instant::now();
    let dir = doc
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?;
    let stem = notebook_stem(doc);
    let notebook = dir.join(format!("{stem}.ipynb"));
    std::fs::write(
        &notebook,
        serde_json::to_string_pretty(&cells::to_notebook(&pass.cells, &pass.outputs, "python3"))?,
    )?;

    let capture =
        qlive_quarto::capture::capture_with(&tools.quarto, &notebook, Some("html"), &["--no-execute"])?;
    let page_file = qlive_quarto::output_path(&notebook, Some("html"));
    let page = std::fs::read_to_string(&page_file)?;
    let shell = Shell::from_page(&page)?;

    let all = vec![true; pass.cells.len()];
    let ours = engine::to_markdown(&pass.cells, &pass.outputs, &stem, dir, &all)?;
    let warm = match first_difference(
        &engine::without_cell_ids(&ours),
        &engine::without_cell_ids(&capture.input),
    ) {
        None => Some(LuaWorker::start_quarto(
            &tools.pandoc,
            &capture.env,
            &capture.cwd,
            &capture.format,
        )?),
        Some(diff) => {
            println!("qlive: warm path off: engine markdown differs from Quarto's — {diff}");
            None
        }
    };
    report_cold(started, warm.is_some());

    let source = read_expanded(doc)?.text;
    let (front, _) = frontmatter::split(&source);
    Ok(Cold {
        capture,
        shell,
        page,
        page_file,
        front_matter: front.map(str::to_string),
        warm,
    })
}

// ---------------------------------------------------------------------------------
// saves

fn on_save(live: &mut Live, preview: &Preview) -> Result<()> {
    let started = Instant::now();
    let source = read_expanded(live.chain.target())?.text;
    let (front, body) = frontmatter::split(&source);
    let front_changed = front.map(str::to_string) != live.cold.front_matter;

    match live.session.as_mut() {
        Some(session) => {
            let pass = run_chain(session, &live.chain)?;
            if !front_changed {
                if let Some(worker) = live.cold.warm.as_mut() {
                    let dir = live
                        .chain
                        .target()
                        .parent()
                        .ok_or_else(|| anyhow!("document has no parent directory"))?;
                    let markdown = engine::to_markdown(
                        &pass.cells,
                        &pass.outputs,
                        &notebook_stem(live.chain.target()),
                        dir,
                        &pass.fresh,
                    )?;
                    if push_warm(
                        worker,
                        &markdown,
                        &live.cold.capture.metadata,
                        &live.cold.shell,
                        dir,
                        preview,
                    )? {
                        println!(
                            "qlive: warm {:>5.0} ms | cells: {} run, {} reused, kernel {:.0} ms | pushed, no reload",
                            ms(started),
                            pass.executed,
                            pass.reused,
                            pass.kernel_ms
                        );
                        for line in &pass.ran {
                            println!("qlive:   ran {line}");
                        }
                        return Ok(());
                    }
                }
            }
            live.cold = cold_cells(&live.tools, live.chain.target(), &pass)?;
        }
        None => {
            if !front_changed {
                if let Some(worker) = live.cold.warm.as_mut() {
                    let dir = live
                        .chain
                        .target()
                        .parent()
                        .ok_or_else(|| anyhow!("document has no parent directory"))?;
                    if push_warm(
                        worker,
                        body,
                        &live.cold.capture.metadata,
                        &live.cold.shell,
                        dir,
                        preview,
                    )? {
                        println!("qlive: warm {:>5.0} ms | pushed, no reload", ms(started));
                        return Ok(());
                    }
                }
            }
            live.cold = cold_plain(&live.tools, live.chain.target())?;
        }
    }

    preview.set_page(live.cold.page.clone());
    preview.notify_reload();
    let why = if front_changed { "front matter changed" } else { "warm path unavailable" };
    println!("qlive: cold {:>5.0} ms ({why}), page reloaded", ms(started));
    Ok(())
}

/// Render through the warm pipeline and push. `false` means fall back to cold.
///
/// Takes the cold render's metadata and shell rather than the whole `Cold`, because
/// the caller holds a mutable borrow of its `warm` field.
fn push_warm(
    worker: &mut LuaWorker,
    text: &str,
    metadata: &str,
    shell: &Shell,
    root: &Path,
    preview: &Preview,
) -> Result<bool> {
    let rendered = worker.render_quarto(text, metadata)?;
    if rendered.has_fatal() {
        println!(
            "qlive: Quarto's filters reported FATAL, falling back to a cold render: {}",
            rendered.diagnostics.join(" | ")
        );
        return Ok(false);
    }
    // Anchors, as Quarto's postprocessor adds them; then image URLs versioned by
    // content, so a re-run figure is refetched instead of shown from cache.
    let html = shell::version_images(&shell::add_heading_anchors(&rendered.out), root);
    preview.set_page(shell.page(&html));
    preview.send_body(&html);
    Ok(true)
}

/// Watch every document in the chain and every file they include; a change to any
/// of them starts a pass. The set is refreshed after each pass, since an edit can
/// add or remove includes.
fn watch(mut live: Live, preview: &Preview) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;
    let mut watched_dirs: Vec<PathBuf> = Vec::new();
    let mut files = watched_files(&live.chain);
    // Directories, not files: editors replace the file on save, which silently
    // drops a file watch.
    watch_dirs(&mut watcher, &files, &mut watched_dirs)?;

    while let Ok(event) = rx.recv() {
        let Ok(event) = event else { continue };
        let ours = event
            .paths
            .iter()
            .any(|p| files.iter().any(|f| same_file(p, f)));
        if !ours {
            continue;
        }
        std::thread::sleep(DEBOUNCE);
        while rx.try_recv().is_ok() {}

        if let Err(e) = on_save(&mut live, preview) {
            eprintln!("qlive: {e}");
        }
        files = watched_files(&live.chain);
        watch_dirs(&mut watcher, &files, &mut watched_dirs)?;
        // Our own renders write beside the documents; drop those events too.
        while rx.try_recv().is_ok() {}
    }

    Err(anyhow!("file watcher stopped"))
}

/// Every file the preview depends on: the chain's documents and what they include.
/// A document whose includes cannot be read still counts itself, so fixing the
/// broken include is noticed.
fn watched_files(chain: &Chain) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for doc in &chain.docs {
        let found = read_expanded(doc)
            .map(|e| e.files)
            .unwrap_or_else(|_| vec![doc.clone()]);
        for file in found {
            if !files.iter().any(|f| same_file(f, &file)) {
                files.push(file);
            }
        }
    }
    files
}

fn watch_dirs(
    watcher: &mut notify::RecommendedWatcher,
    files: &[PathBuf],
    watched: &mut Vec<PathBuf>,
) -> Result<()> {
    for file in files {
        if let Some(dir) = file.parent() {
            if !watched.iter().any(|d| same_file(d, dir)) {
                watcher.watch(dir, RecursiveMode::NonRecursive)?;
                watched.push(dir.to_path_buf());
            }
        }
    }
    Ok(())
}


// ---------------------------------------------------------------------------------
// helpers

fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// `None` when equal, otherwise the first differing line of each.
fn first_difference(ours: &str, quartos: &str) -> Option<String> {
    if ours == quartos {
        return None;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (ours.lines().collect(), quartos.lines().collect());
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or("<end>"), b.get(i).copied().unwrap_or("<end>"));
        if x != y {
            return Some(format!("line {}: ours {x:?}, Quarto's {y:?}", i + 1));
        }
    }
    Some("trailing whitespace differs".to_string())
}

fn report_pass(label: &str, pass: &Pass) {
    println!(
        "qlive: {label}: {} cells run, {} reused, kernel {:.0} ms",
        pass.executed, pass.reused, pass.kernel_ms
    );
    for line in &pass.ran {
        println!("qlive:   ran {line}");
    }
}

fn report_cold(started: Instant, warm: bool) {
    println!(
        "qlive: cold render + capture in {:.0} ms{}",
        ms(started),
        if warm { ", warm pipeline loaded" } else { "" }
    );
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
