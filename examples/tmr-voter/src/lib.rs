//! Triple modular redundancy (TMR) voter, a worked rubric example.
//!
//! Three redundant channels feed a majority vote. The requirements live
//! in `rubric.toml`. Annotations below name the satisfier and verifiers.
//! Annotations here are plain comments, so this crate has no rubric
//! dependency in its build graph.

/// A three-channel redundant voter.
struct Tmr;

impl Tmr {
    /// Majority vote: the output any two or more channels agree on.
    // satisfies: TMR-1
    fn vote(a: bool, b: bool, c: bool) -> bool {
        (a & b) | (b & c) | (a & c)
    }
}

/// Public entry point so integration tests can exercise the voter without
/// reaching private items.
pub fn majority3(a: bool, b: bool, c: bool) -> bool {
    Tmr::vote(a, b, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    // verifies: TMR-1
    #[test]
    fn agreement_wins() {
        assert!(majority3(true, true, false));
        assert!(!majority3(false, false, true));
    }

    // verifies: TMR-2
    #[test]
    fn order_independent() {
        for (a, b, c) in [(true, true, false), (true, false, true), (false, true, true)] {
            assert_eq!(majority3(a, b, c), majority3(c, b, a));
        }
    }
}
