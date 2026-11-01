//! qlive — instant Quarto preview.
//!
//! M0: no block diffing yet. `preview` pays Quarto's cold-render cost once, then
//! keeps the server and the socket alive so a save costs one `quarto render` instead
//! of a `quarto preview` restart. `bench` establishes the latency budget that M1–M5
//! are measured against.

mod bench;
mod cells;
mod editor;
mod live;
mod parity;
mod preview;
mod restart;
mod session;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "qlive",
    version,
    about = "Instant Quarto preview: warm engines, block-level patches",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Native editor operations: one JSON request on stdin, one JSON response on stdout.
    Editor,
    /// Cold-render once, then watch the file and re-render on change.
    Preview {
        /// The .qmd or .md to preview.
        doc: PathBuf,
        /// Port for the preview server.
        #[arg(long, default_value_t = 4444)]
        port: u16,
        /// Quarto format, e.g. html or typst. Defaults to the document's own.
        #[arg(long)]
        to: Option<String>,
    },
    /// Live preview: warm Quarto renders pushed into the page, no reload; with code
    /// cells, only the cells that changed run.
    Live {
        /// The .qmd or .md to preview.
        doc: PathBuf,
        /// Documents whose cells run first, in the same kernel, so their variables are
        /// available here. Repeat in order: --after ch1.qmd --after ch2.qmd
        #[arg(long = "after")]
        after: Vec<PathBuf>,
        /// Port for the preview server.
        #[arg(long, default_value_t = 4444)]
        port: u16,
        /// Ignore the restart cache: render cold with Quarto, as on a first start.
        #[arg(long)]
        fresh: bool,
    },
    /// Execute only the cells that changed, against a kernel that outlives the render.
    Cells {
        /// The .qmd with code cells.
        doc: PathBuf,
        /// One pass, then exit, instead of watching for saves.
        #[arg(long)]
        once: bool,
        /// Quarto format, e.g. html.
        #[arg(long)]
        to: Option<String>,
    },
    /// Compare the warm path with Quarto across a corpus of documents. Documents with
    /// code cells, local filters or extensions are skipped, never executed.
    Parity {
        /// Directory to search for .qmd files, e.g. quarto-cli's tests/docs/smoke-all.
        dir: PathBuf,
        /// Only run the first N eligible documents.
        #[arg(long)]
        limit: Option<usize>,
        /// Write the markdown report here as well.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Measure the latency budget this project is built on.
    Bench {
        /// Document to measure. A fixture is generated when omitted.
        #[arg(long)]
        doc: Option<PathBuf>,
        /// Iterations for the warm measurements.
        #[arg(long, default_value_t = 20)]
        iters: usize,
        /// Sections in the generated fixture (40 gives ~283 lines).
        #[arg(long, default_value_t = 40)]
        sections: usize,
        /// Port for the benchmarked `pandoc server`.
        #[arg(long, default_value_t = 4330)]
        pandoc_port: u16,
    },
    /// The Python kernels `qlive live` keeps running between starts: list, start, stop.
    Kernels {
        /// Only this document's kernel. Required with --start.
        doc: Option<PathBuf>,
        /// Start the document's kernel and run its cells, without a preview.
        #[arg(long, conflicts_with = "stop")]
        start: bool,
        /// Shut kernels down, freeing their memory (and GPU memory).
        #[arg(long)]
        stop: bool,
        /// Print a JSON array, for editors.
        #[arg(long)]
        json: bool,
    },
    /// Report which binaries qlive found, and their versions.
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Editor => editor::run(),
        Command::Preview { doc, port, to } => preview::run(doc, port, to).await,
        Command::Live { doc, after, port, fresh } => live::run(doc, after, port, fresh).await,
        Command::Cells { doc, once, to } => cells::run(doc, once, to),
        Command::Parity { dir, limit, report } => parity::run(dir, limit, report),
        Command::Bench {
            doc,
            iters,
            sections,
            pandoc_port,
        } => bench::run(doc, iters, sections, pandoc_port),
        Command::Kernels { doc, start, stop, json } => kernels(doc, start, stop, json),
        Command::Doctor => doctor(),
    }
}

fn kernels(doc: Option<PathBuf>, start: bool, stop: bool, json: bool) -> Result<()> {
    let doc = doc.map(|d| qlive_core::absolute(&d)).transpose()?;
    if start {
        let doc = doc.ok_or_else(|| anyhow::anyhow!("--start needs the document"))?;
        let tools = qlive_core::Toolchain::discover()?;
        let chain = session::Chain { docs: vec![doc] };
        if session::kept_kernel_running(&tools, &chain) {
            println!("kernel already running");
            return Ok(());
        }
        let mut session = session::start_session(&tools, &chain, session::KernelMode::Persistent { fresh: false })?;
        if !session.is_persistent() { anyhow::bail!("the kernel could not be kept running"); }
        let pass = session::run_chain(&mut session, &chain)?;
        println!("kernel ready: {} cells run, {} reused, {:.0} ms in the kernel", pass.executed, pass.reused, pass.kernel_ms);
        return Ok(());
    }
    let same = |label: &str| doc.as_ref().is_none_or(|d| {
        let (a, b) = (d.to_string_lossy(), label);
        if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
    });
    let mut listed = Vec::new();
    for entry in session::kernels_dir().and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
        let Ok(info) = std::fs::read(&path).map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<serde_json::Value>(&bytes)?)) else { continue };
        let label = info["label"].as_str().unwrap_or("?").to_owned();
        if !same(&label) { continue; }
        let port = info["port"].as_u64().unwrap_or(0) as u16;
        let alive = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), std::time::Duration::from_millis(300)).is_ok();
        let state = if !alive {
            let _ = std::fs::remove_file(&path);
            "gone"
        } else if stop {
            if qlive_kernel::Kernel::stop_daemon(&path).is_err() {
                // Busy serving a preview: end the daemon and its kernel directly.
                if let Some(pid) = info["pid"].as_u64() { kill_tree(pid); }
                let _ = std::fs::remove_file(&path);
            }
            "stopped"
        } else {
            "running"
        };
        listed.push(serde_json::json!({"doc": label, "pid": info["pid"], "started": info["started"], "state": state}));
    }
    if json {
        println!("{}", serde_json::Value::Array(listed));
        return Ok(());
    }
    if listed.is_empty() { println!("no kept kernels"); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    for k in &listed {
        let up = k["started"].as_f64().map(|t| format!(", up {:.0} min", (now - t) / 60.0)).unwrap_or_default();
        println!("{:<8} {} (pid {}{up})", k["state"].as_str().unwrap_or(""), k["doc"].as_str().unwrap_or(""), k["pid"]);
    }
    Ok(())
}

fn kill_tree(pid: u64) {
    let _ = if cfg!(windows) {
        std::process::Command::new("taskkill.exe").args(["/PID", &pid.to_string(), "/T", "/F"]).output()
    } else {
        std::process::Command::new("kill").args(["-9", &pid.to_string()]).output()
    };
}

fn doctor() -> Result<()> {
    let tools = qlive_core::Toolchain::discover()?;
    println!("quarto : {}", tools.quarto.display());
    println!("pandoc : {}", tools.pandoc.display());
    match &tools.typst {
        Some(t) => println!("typst  : {}", t.display()),
        None => println!("typst  : not found (the typst output path will be unavailable)"),
    }
    println!();
    for (name, version) in tools.versions() {
        println!("{name:<7}: {version}");
    }
    Ok(())
}
