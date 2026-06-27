//! Integration verifier: exercises the voter through its public surface.

use tmr_voter::majority3;

// verifies: TMR-1
#[test]
fn unanimous_channels() {
    assert!(majority3(true, true, true));
    assert!(!majority3(false, false, false));
}
