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
    /// Report which binaries qlive found, and their versions.
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Editor => editor::run(),
        Command::Preview { doc, port, to } => preview::run(doc, port, to).await,
        Command::Live { doc, after, port } => live::run(doc, after, port).await,
        Command::Cells { doc, once, to } => cells::run(doc, once, to),
        Command::Parity { dir, limit, report } => parity::run(dir, limit, report),
        Command::Bench {
            doc,
            iters,
            sections,
            pandoc_port,
        } => bench::run(doc, iters, sections, pandoc_port),
        Command::Doctor => doctor(),
    }
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
