//! Pure verification core for rubric.
//!
//! Everything in this crate is a pure function over `(source content,
//! manifest, attribution data)`. No I/O, no globals, no side effects.
//! Callers (the `cargo-rubric` CLI, or a consumer's `build.rs`) read
//! files and hand the contents in. Stdlib-only by policy.

pub mod check;
pub mod fnv;
pub mod hash;
pub mod lock;
pub mod manifest;
pub mod normalize;
pub mod pointcut;
pub mod reach;
pub mod toml_lite;
