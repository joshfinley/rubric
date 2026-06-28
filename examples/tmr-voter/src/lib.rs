//! Triple modular redundancy (TMR): three diverse channels feed a majority
//! vote. The requirements live in `rubric.toml`. Annotations here are plain
//! comments, so this crate has no rubric dependency in its build graph.

pub mod channels {
    //! The redundant channels. There are no rubric annotations in this
    //! module. The `TMR-CHANNELS` pointcut binds every `pub fn` here. A
    //! fourth channel added without review fails `check` until accepted.

    /// Channel A.
    pub fn channel_a(sample: u8) -> bool {
        sample >= 3
    }

    /// Channel B, developed independently of A.
    pub fn channel_b(sample: u8) -> bool {
        sample > 2
    }

    /// Channel C, developed independently of A and B.
    pub fn channel_c(sample: u8) -> bool {
        sample / 3 >= 1
    }
}

/// Majority vote: the output two or more channels agree on.
// satisfies: TMR-VOTE
pub fn vote(a: bool, b: bool, c: bool) -> bool {
    (a & b) | (b & c) | (a & c)
}

#[cfg(test)]
mod tests {
    use super::*;

    // verifies: TMR-VOTE
    #[test]
    fn majority_decides() {
        assert!(vote(true, true, false));
        assert!(!vote(false, false, true));
    }

    // verifies: TMR-CHANNELS
    #[test]
    fn channels_agree_on_clear_input() {
        let s = 9;
        assert!(channels::channel_a(s) && channels::channel_b(s) && channels::channel_c(s));
    }
}
