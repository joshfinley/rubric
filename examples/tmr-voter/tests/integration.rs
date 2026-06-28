//! Integration verifier: exercises the voter through its public surface.

use tmr_voter::vote;

// verifies: TMR-VOTE
#[test]
fn unanimous_inputs() {
    assert!(vote(true, true, true));
    assert!(!vote(false, false, false));
}
