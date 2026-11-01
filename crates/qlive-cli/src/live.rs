//! `qlive live` — M2: warm renders spliced into the cold page, pushed without reload.
//!
//! One cold `quarto render` (with capture) yields three things: the full page, its
//! shell, and Quarto's pandoc pipeline to host warm. After that, a save re-renders
//! the body through Quarto's own reader and filters in ~80 ms, adds the heading
//! anchors Quarto's postprocessor would, splices it into the shell, and pushes it to
//! the browser, which swaps `<main>` in place.
//!
//! The cold path is taken whenever the warm one could be wrong:
//!
//! * the **front matter changed** — template, title block and metadata depend on it;
//! * Quarto's filters reported **FATAL** — pandoc exits 0 regardless;
//! * the **self-check** failed at cold-render time: the body Quarto fed pandoc must
//!   equal the document's body, and engine preprocessing (executed cells, includes)
//!   breaks that. Such documents get a cold render per save instead of a wrong
//!   preview.

use anyhow::{anyhow, Result};
use notify::{RecursiveMode, Watcher};
use qlive_core::{cells, frontmatter, Toolchain};
use qlive_pandoc::LuaWorker;
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
    /// The front matter this was rendered with; any change needs a new cold render.
    front_matter: Option<String>,
    /// `None` when the self-check says the warm path would not match Quarto.
    warm: Option<LuaWorker>,
}

pub async fn run(doc: PathBuf, port: u16) -> Result<()> {
    let tools = Toolchain::discover()?;
    let doc = qlive_core::absolute(&doc)?;

    let state = cold(&tools, &doc)?;

    let output = qlive_quarto::output_path(&doc, Some("html"));
    let root = output
        .parent()
        .ok_or_else(|| anyhow!("render output has no parent directory"))?
        .to_path_buf();
    let entry = output
        .file_name()
        .ok_or_else(|| anyhow!("render output has no file name"))?
        .to_string_lossy()
        .to_string();

    let preview = Preview::new(root, entry);
    preview.set_page(state.page.clone());

    {
        let preview = preview.clone();
        let tools = tools.clone();
        let doc = doc.clone();
        std::thread::spawn(move || {
            if let Err(e) = watch(&tools, &doc, state, &preview) {
                eprintln!("qlive: watcher stopped: {e}");
            }
        });
    }

    println!("qlive: http://127.0.0.1:{port}/  (ctrl-c to stop)");
    qlive_server::serve(preview, port).await
}

fn cold(tools: &Toolchain, doc: &Path) -> Result<Cold> {
    let started = Instant::now();
    let capture = qlive_quarto::capture::capture(&tools.quarto, doc, Some("html"))?;
    let page = std::fs::read_to_string(qlive_quarto::output_path(doc, Some("html")))?;
    let shell = Shell::from_page(&page)?;

    let source = std::fs::read_to_string(doc)?;
    let (front, body) = frontmatter::split(&source);

    let refusal = if cells::from_markdown(&source).iter().any(|c| c.is_code()) {
        Some("the document has executable cells, and the warm path does not reproduce engine output yet")
    } else if !frontmatter::same_body(body, &capture.input) {
        Some("Quarto preprocessed the body before pandoc (an include, or similar)")
    } else {
        None
    };

    let warm = match refusal {
        None => Some(LuaWorker::start_quarto(
            &tools.pandoc,
            &capture.env,
            &capture.cwd,
            &capture.format,
        )?),
        Some(why) => {
            println!("qlive: warm path off: {why}; each save will cold-render instead");
            None
        }
    };

    println!(
        "qlive: cold render + capture in {:.0} ms{}",
        ms(started),
        if warm.is_some() { ", warm pipeline loaded" } else { "" }
    );

    Ok(Cold {
        capture,
        shell,
        page,
        front_matter: front.map(str::to_string),
        warm,
    })
}

fn on_save(tools: &Toolchain, doc: &Path, state: &mut Cold, preview: &Preview) -> Result<()> {
    let started = Instant::now();
    let source = std::fs::read_to_string(doc)?;
    let (front, body) = frontmatter::split(&source);
    let front = front.map(str::to_string);
    let front_changed = front != state.front_matter;

    if !front_changed {
        if let Some(worker) = state.warm.as_mut() {
            let rendered = worker.render_quarto(body, &state.capture.metadata)?;
            if rendered.has_fatal() {
                println!(
                    "qlive: Quarto's filters reported FATAL, falling back to a cold render: {}",
                    rendered.diagnostics.join(" | ")
                );
            } else {
                let html = shell::add_heading_anchors(&rendered.out);
                preview.set_page(state.shell.page(&html));
                preview.send_body(&html);
                println!(
                    "qlive: warm {:>5.0} ms (Quarto pipeline {:.0} ms), pushed, no reload",
                    ms(started),
                    rendered.ms
                );
                return Ok(());
            }
        }
    }

    let why = if front_changed { "front matter changed" } else { "warm path unavailable" };
    *state = cold(tools, doc)?;
    preview.set_page(state.page.clone());
    preview.notify_reload();
    println!("qlive: cold {:>5.0} ms ({why}), page reloaded", ms(started));
    Ok(())
}

/// Watch the document's directory (not the file: editors replace it on save, which
/// silently drops a file watch) and filter by name.
fn watch(tools: &Toolchain, doc: &Path, mut state: Cold, preview: &Preview) -> Result<()> {
    let dir = doc
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?;
    let name = doc
        .file_name()
        .ok_or_else(|| anyhow!("document has no file name"))?
        .to_os_string();

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;

    while let Ok(event) = rx.recv() {
        let Ok(event) = event else { continue };
        if !event
            .paths
            .iter()
            .any(|p| p.file_name() == Some(name.as_os_str()))
        {
            continue;
        }
        // Let the editor finish writing, then collapse the rest of this save.
        std::thread::sleep(DEBOUNCE);
        while rx.try_recv().is_ok() {}

        if let Err(e) = on_save(tools, doc, &mut state, preview) {
            eprintln!("qlive: {e}");
        }
        // Our own cold renders write beside the document; drop those events too.
        while rx.try_recv().is_ok() {}
    }

    Err(anyhow!("file watcher stopped"))
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
