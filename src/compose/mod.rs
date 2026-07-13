//! Static composition of WASM packages into a single self-contained binary.
//!
//! Produces one merged `.wasm` where each package's cross-package imports are
//! resolved to direct internal calls — no runtime cross-module dispatch, and
//! (unlike a runtime [`crate::runtime::PicCompositionBuilder`]) no host harness:
//! the output has zero imports and runs on any stock runtime.
//!
//! See [`compose`] for the pipeline (binaryen `wasm-merge` + a `walrus`
//! internalize pass). [`ParsedModule`] is a lightweight reusable WASM parser
//! (also used by the `inspect` CLI command).
//!
//! # Example
//!
//! ```ignore
//! use packr::compose::{compose, ComposeSpec, PackageSpec};
//!
//! let composed = compose(&ComposeSpec::new(vec![
//!     PackageSpec::new("pack:alloc", std::fs::read("pack_alloc.wasm")?),
//!     PackageSpec::new("math", std::fs::read("doubler.wasm")?), // adder imports "math"
//!     PackageSpec::new("adder", std::fs::read("adder.wasm")?),
//! ]))?;
//! std::fs::write("composed.wasm", composed)?;
//! ```

mod error;
mod parser;
mod static_compose;

pub use error::ComposeError;
pub use parser::ParsedModule;
pub use static_compose::{compose, ComposeSpec, Layout, PackageSpec};
