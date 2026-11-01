//! qlive — instant Quarto preview.
//!
//! M0: no block diffing yet. `preview` pays Quarto's cold-render cost once, then
//! keeps the server and the socket alive so a save costs one `quarto render` instead
//! of a `quarto preview` restart. `bench` establishes the latency budget that M1–M5
//! are measured against.

mod bench;
mod cells;
mod preview;

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
        Command::Preview { doc, port, to } => preview::run(doc, port, to).await,
        Command::Cells { doc, once, to } => cells::run(doc, once, to),
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
