//! `qlive cells` — execute only the cells that need it, and render with Quarto.
//!
//! Uses the same kernel session and dependency planner as `qlive live`
//! ([`crate::session`]): one kernel held for the life of the command, so skipping a
//! cell is sound; a cell runs only when its source or something it reads changed.
//! Each pass writes a populated notebook and hands it to
//! `quarto render --no-execute`, so Quarto renders our outputs and runs nothing.

use crate::session::{run_chain, start_session, Chain, KernelMode};
use anyhow::{anyhow, Result};
use notify::{RecursiveMode, Watcher};
use qlive_core::cells;
use qlive_core::Toolchain;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Editors touch a file several times per save.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub fn run(doc: PathBuf, once: bool, to: Option<String>) -> Result<()> {
    let tools = Toolchain::discover()?;
    let doc = qlive_core::absolute(&doc)?;
    let chain = Chain {
        docs: vec![doc.clone()],
    };
    let mut session = start_session(&tools, &chain, KernelMode::Private)?;
    let notebook = doc.with_extension("qlive.ipynb");

    loop {
        if let Err(e) = pass(&tools, &mut session, &chain, &notebook, to.as_deref()) {
            eprintln!("qlive: {e}");
        }
        if once {
            return Ok(());
        }
        wait_for_change(&doc)?;
    }
}

fn pass(
    tools: &Toolchain,
    session: &mut crate::session::Session,
    chain: &Chain,
    notebook: &Path,
    to: Option<&str>,
) -> Result<()> {
    let started = Instant::now();
    let pass = run_chain(session, chain)?;

    let nb = cells::to_notebook(&pass.cells, &pass.outputs, "python3");
    std::fs::write(notebook, serde_json::to_string_pretty(&nb)?)?;
    let rendered = qlive_quarto::render_no_execute(&tools.quarto, notebook, to)?;

    println!(
        "qlive: {} run, {} reused | kernel {:.0} ms | render {:.0} ms | total {:.0} ms -> {}",
        pass.executed,
        pass.reused,
        pass.kernel_ms,
        rendered.duration.as_secs_f64() * 1000.0,
        started.elapsed().as_secs_f64() * 1000.0,
        rendered.output.display()
    );
    for line in &pass.ran {
        println!("qlive:   ran {line}");
    }
    Ok(())
}

/// Watch the document's directory and return when the document itself is touched.
fn wait_for_change(doc: &Path) -> Result<()> {
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
        if event
            .paths
            .iter()
            .any(|p| p.file_name() == Some(name.as_os_str()))
        {
            std::thread::sleep(DEBOUNCE);
            while rx.try_recv().is_ok() {}
            return Ok(());
        }
    }

    Err(anyhow!("file watcher stopped"))
}
