//! Shared types for the rubric traceability toolchain.
//!
//! The `rubric` proc-macro crate and the `cargo-rubric` CLI both depend
//! on this crate so they agree on manifest shape and the FNV-1a id
//! derivation used as a stable identifier in matrix output.

// Under test compilation, pull in the proc-macros (dev-dep) and emit the
// marker module so test fixtures can write `#[rubric::verifies(...)]`.
#[cfg(test)]
rubric::setup!();

pub mod build;
pub mod codegen;
pub mod fnv;
pub mod hash;
pub mod lockfile;
pub mod manifest;
pub mod resolver;
pub mod toml_lite;
pub mod walker;

pub use fnv::fnv1a_64;
pub use manifest::{Direction, Manifest, Requirement, ReqId};
