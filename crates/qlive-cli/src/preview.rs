//! `qlive preview` — M0's loop: cold render once, re-render on change, tell the
//! browser. No diffing yet; the win here is purely that nothing restarts.

use anyhow::{anyhow, Result};
use notify::{RecursiveMode, Watcher};
use qlive_core::Toolchain;
use qlive_server::Preview;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Ignore file events that arrive within this window of the last render — editors
/// touch a file several times per save.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub async fn run(doc: PathBuf, port: u16, to: Option<String>) -> Result<()> {
    let tools = Toolchain::discover()?;
    let doc = qlive_core::absolute(&doc)?;

    println!("qlive: cold render with {}", tools.quarto.display());
    let first = qlive_quarto::render(&tools.quarto, &doc, to.as_deref())?;
    println!(
        "qlive: {} in {:.0} ms  (the Deno tax, paid once per session)",
        first.output.display(),
        first.duration.as_secs_f64() * 1000.0
    );

    let root = first
        .output
        .parent()
        .ok_or_else(|| anyhow!("render output has no parent directory"))?
        .to_path_buf();
    let entry = first
        .output
        .file_name()
        .ok_or_else(|| anyhow!("render output has no file name"))?
        .to_string_lossy()
        .to_string();

    let preview = Preview::new(root, entry);

    {
        let preview = preview.clone();
        let quarto = tools.quarto.clone();
        let doc = doc.clone();
        let to = to.clone();
        std::thread::spawn(move || {
            if let Err(e) = watch(&quarto, &doc, to.as_deref(), preview) {
                eprintln!("qlive: watcher stopped: {e}");
            }
        });
    }

    println!("qlive: http://127.0.0.1:{port}/  (ctrl-c to stop)");
    qlive_server::serve(preview, port).await
}

/// Watch the document's directory (not the file: editors replace inodes on save,
/// which silently drops a file watch) and filter by name.
fn watch(quarto: &Path, doc: &Path, to: Option<&str>, preview: Preview) -> Result<()> {
    let dir = doc
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?;
    let name = doc
        .file_name()
        .ok_or_else(|| anyhow!("document has no file name"))?
        .to_os_string();

    // Annotated: notify infers the handler type from this impl, and leaving it to
    // inference is how you get "type annotations needed" on a clean checkout.
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;

    let mut last = Instant::now()
        .checked_sub(DEBOUNCE)
        .unwrap_or_else(Instant::now);

    while let Ok(event) = rx.recv() {
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                eprintln!("qlive: watch error: {e}");
                continue;
            }
        };

        let touched_doc = event
            .paths
            .iter()
            .any(|p| p.file_name() == Some(name.as_os_str()));
        if !touched_doc || last.elapsed() < DEBOUNCE {
            continue;
        }

        // Collapse the rest of this save's events.
        while rx.try_recv().is_ok() {}
        last = Instant::now();

        match qlive_quarto::render(quarto, doc, to) {
            Ok(done) => {
                println!(
                    "qlive: re-rendered in {:.0} ms",
                    done.duration.as_secs_f64() * 1000.0
                );
                preview.notify_reload();
            }
            Err(e) => eprintln!("qlive: render failed: {e}"),
        }
    }

    Ok(())
}
