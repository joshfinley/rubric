//! Triple Modular Redundancy (TMR) voter.
//!
//! Canonical safety-critical pattern: take three independently-computed
//! inputs and emit the value that the majority agrees on. Single-fault
//! tolerant — one divergent channel is outvoted by the other two; three
//! mutually-disagreeing channels signal a fault that must be handled by
//! a higher layer.
//!
//! This crate is `#![no_std]`; it exists to serve both as a worked
//! example of the bind traceability chain and as a regression guard
//! against the `bind` macros accidentally emitting code that requires
//! the standard library.

#![no_std]
#![allow(dead_code)]

rubric::setup!();

/// The voter's possible outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote<T> {
    /// At least two channels agreed on this value.
    Majority(T),
    /// All three channels disagreed — the higher layer must decide.
    Disagreement,
}

/// Vote the majority of three channel outputs. When all three values are
/// equal, the shared value is returned. When two of three agree, the
/// agreed value wins. When all three differ, `Vote::Disagreement` is
/// returned so the caller can enter a recovery mode appropriate to its
/// domain (reset channel, fall back to last-known-good, latch a fault).
#[satisfies(
    crate::reqs::voter::majority_wins,
    crate::reqs::voter::all_agree_passthrough,
    crate::reqs::voter::single_fault_tolerance,
    crate::reqs::voter::unanimous_disagreement_flagged,
    crate::reqs::voter::deterministic,
    crate::reqs::voter::no_std_compatible,
)]
pub fn vote<T: Eq + Copy>(a: T, b: T, c: T) -> Vote<T> {
    if a == b || a == c {
        Vote::Majority(a)
    } else if b == c {
        Vote::Majority(b)
    } else {
        Vote::Disagreement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[verifies(crate::reqs::voter::all_agree_passthrough)]
    fn all_agree_returns_value() {
        assert_eq!(vote(7, 7, 7), Vote::Majority(7));
    }

    #[test]
    #[verifies(crate::reqs::voter::majority_wins)]
    fn two_of_three_win_regardless_of_position() {
        assert_eq!(vote(1, 1, 2), Vote::Majority(1));
        assert_eq!(vote(1, 2, 1), Vote::Majority(1));
        assert_eq!(vote(2, 1, 1), Vote::Majority(1));
    }

    #[test]
    #[verifies(crate::reqs::voter::single_fault_tolerance)]
    fn single_divergent_channel_does_not_affect_output() {
        let good = 42u32;
        let bad = 0xdead_beef;
        assert_eq!(vote(good, good, bad), Vote::Majority(good));
        assert_eq!(vote(good, bad, good), Vote::Majority(good));
        assert_eq!(vote(bad, good, good), Vote::Majority(good));
    }

    #[test]
    #[verifies(crate::reqs::voter::unanimous_disagreement_flagged)]
    fn all_three_differ_surfaces_disagreement() {
        assert_eq!(vote(1, 2, 3), Vote::Disagreement);
    }

    #[test]
    #[verifies(crate::reqs::voter::deterministic)]
    fn determinism_across_repeated_calls() {
        for _ in 0..50 {
            assert_eq!(vote(10, 20, 10), Vote::Majority(10));
            assert_eq!(vote(10, 20, 30), Vote::Disagreement);
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
