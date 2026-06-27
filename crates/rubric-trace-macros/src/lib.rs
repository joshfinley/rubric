//! Identity attribute macros for the bare-attribute annotation form.
//!
//! `#[satisfies(...)]` and `#[verifies(...)]` pass their input token stream
//! through unchanged, so they do nothing at compile time beyond marking the
//! item for the scanner. See the Rust reference on attribute macros [1].
//!
//! The `#[cfg_attr(any(), satisfies(...))]` form is compiler-inert and
//! doesn't need a macro crate [2], so the scanner supports both spellings.
//! Keep `cfg_attr` as the default: the proc-macro forms reduce to the same
//! pass-through, and some crates can't take on a proc-macro dependency
//! (TODO: add bound requirement reference here). Crates that can use the
//! macros get a shorter syntax for the same result.
//!
//! References:
//! [1] https://doc.rust-lang.org/reference/procedural-macros.html#the-proc_macro_attribute-attribute
//! [2] https://chrismorgan.info/blog/rust-cfg_attr/

use proc_macro::TokenStream;

/// Satisfy `proc_macro` for annotations. Sidesteps hygiene system by
/// passing `item` through unmodified. This macro must never do anything
/// else.
/// 
/// ```no_run
/// use rubric_trace_macros::satisfies;
///
/// #[satisfies(VOTER-1)]
/// fn vote() {}
/// ```
#[proc_macro_attribute]
pub fn satisfies(_args: TokenStream, item: TokenStream) -> TokenStream {
    item // pass-through
}

/// Verify `proc_macro` for annotations. Sidesteps hygiene system by
/// passing `item` through unmodified. This macro must never do anything
/// else.
/// ```no_run
/// use rubric_trace_macros::verifies;
///
/// #[verifies(VOTER-1)]
/// fn two_against_one() {}
/// ```
#[proc_macro_attribute]
pub fn verifies(_args: TokenStream, item: TokenStream) -> TokenStream {
    item // pass-through
}

