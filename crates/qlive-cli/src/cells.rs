//! `qlive cells` — execute only the cells that changed.
//!
//! The kernel is held for the life of this command, which is what makes skipping a
//! cell sound: the variables an earlier cell defined are still live. Each pass
//! compares the document's cell keys against the keys the kernel has actually
//! executed, runs from the first divergence onward, reuses cached outputs before it,
//! writes a populated notebook, and hands that to `quarto render --no-execute`.

use anyhow::{anyhow, Result};
use notify::{RecursiveMode, Watcher};
use qlive_core::cells::{self, Action, CellOutputs};
use qlive_core::Toolchain;
use qlive_kernel::Kernel;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Editors touch a file several times per save.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub fn run(doc: PathBuf, once: bool, to: Option<String>) -> Result<()> {
    let tools = Toolchain::discover()?;
    let python = tools.python.clone().ok_or_else(|| {
        anyhow!("no usable Python 3 found (the Microsoft Store stub does not count); set QLIVE_PYTHON")
    })?;
    let doc = qlive_core::absolute(&doc)?;
    let dir = doc
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?
        .to_path_buf();

    println!("qlive: kernel via {}", python.display());
    let booting = Instant::now();
    let mut kernel = Kernel::start(&python, &dir)?;
    println!(
        "qlive: kernel ready in {:.0} ms (paid once, not per render)",
        booting.elapsed().as_secs_f64() * 1000.0
    );

    let mut cache: HashMap<String, CellOutputs> = HashMap::new();
    let notebook = doc.with_extension("qlive.ipynb");

    loop {
        if let Err(e) = pass(&tools, &mut kernel, &mut cache, &doc, &notebook, to.as_deref()) {
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
    kernel: &mut Kernel,
    cache: &mut HashMap<String, CellOutputs>,
    doc: &Path,
    notebook: &Path,
    to: Option<&str>,
) -> Result<()> {
    let started = Instant::now();

    let source = std::fs::read_to_string(doc)?;
    let cells = cells::from_markdown(&source);
    let keys = cells::code_keys(&cells);
    let actions = cells::plan(&keys, kernel.executed());

    let mut outputs: Vec<Option<CellOutputs>> = vec![None; cells.len()];
    let mut executed = 0usize;
    let mut reused = 0usize;
    let mut kernel_ms = 0.0f64;
    let mut code_index = 0usize;

    for (index, cell) in cells.iter().enumerate() {
        if !cell.is_code() {
            continue;
        }
        let key = &keys[code_index];
        match actions[code_index] {
            Action::Reuse => {
                outputs[index] = cache.get(key).cloned();
                reused += 1;
            }
            Action::Execute => {
                let result = kernel.execute(code_index, key, &cell.source)?;
                kernel_ms += result.ms;
                let produced = CellOutputs {
                    outputs: result.outputs,
                    execution_count: result.execution_count,
                };
                cache.insert(key.clone(), produced.clone());
                outputs[index] = Some(produced);
                executed += 1;
            }
        }
        code_index += 1;
    }

    let nb = cells::to_notebook(&cells, &outputs, "python3");
    std::fs::write(notebook, serde_json::to_string_pretty(&nb)?)?;

    let rendered = qlive_quarto::render_no_execute(&tools.quarto, notebook, to)?;

    println!(
        "qlive: {executed} executed, {reused} reused | kernel {kernel_ms:.0} ms | render {:.0} ms | total {:.0} ms -> {}",
        rendered.duration.as_secs_f64() * 1000.0,
        started.elapsed().as_secs_f64() * 1000.0,
        rendered.output.display()
    );
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
