//! Shared pieces of qlive: source splitting and toolchain discovery.
//!
//! M0 scope: the splitter exists so `qlive bench` can report how little of a
//! document a single edit actually touches. The diff and patch layers that consume
//! it are M1/M2 — see the design note.

pub mod blocks;
pub mod cells;
pub mod frontmatter;
pub mod paths;
pub mod toolchain;

pub use blocks::{dirty_count, hashes, split, Block, BlockKind};
pub use cells::{Action, Cell, CellKind, CellOutputs};
pub use paths::absolute;
pub use toolchain::Toolchain;
