//! Normalization policy for seals. Pure, stdlib-only.
//!
//! Two seal subjects need canonical forms:
//!
//! - Statement (`stmt:` seal). Plain text from `rubric.toml`. We
//!   collapse internal whitespace runs to single spaces and trim, so
//!   reflowing the TOML string doesn't break the seal but rewording does.
//!
//! - Body (`body:` seal). A satisfier or verifier body. The scanner
//!   lexes it with `rustc_lexer` and hands us the significant token texts
//!   (whitespace and comments already dropped). We join them with single
//!   spaces. The core owns this join so the oracle (`check`) and `accept`
//!   produce identical seal input for the same source.
//!
//! Joining significant tokens with a separator is what makes the body
//! seal reformatting-invariant: `a-1` and `a - 1` both lex to the tokens
//! `a`, `-`, `1`, so both normalize to `"a - 1"`. Any real token change
//! (a renamed local, a flipped operator) changes the join.

/// Canonical form of a requirement statement for the `stmt:` seal.
pub fn statement(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canonical body seal input from already-lexed significant tokens.
///
/// The scanner passes the source text of each token in order, with
/// whitespace and comment tokens already filtered out.
pub fn body_from_tokens<I, S>(tokens: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    for tok in tokens {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok.as_ref());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_collapses_whitespace() {
        assert_eq!(statement("  Two   identical\ninputs\tout-vote "), "Two identical inputs out-vote");
    }

    #[test]
    fn statement_reword_changes_form() {
        assert_ne!(statement("must reject"), statement("should reject"));
    }

    #[test]
    fn body_joins_tokens_with_single_space() {
        let toks = ["a", "-", "1"];
        assert_eq!(body_from_tokens(toks), "a - 1");
    }

    #[test]
    fn body_is_reformatting_invariant() {
        // Two lexings of the same logic differing only in source spacing
        // yield the same token sequence, hence the same normal form.
        let tight = ["(", "a", "&", "b", ")", "|", "(", "b", "&", "c", ")"];
        let loose = ["(", "a", "&", "b", ")", "|", "(", "b", "&", "c", ")"];
        assert_eq!(body_from_tokens(tight), body_from_tokens(loose));
    }

    #[test]
    fn body_token_change_changes_form() {
        let before = ["a", "&", "b"];
        let after = ["a", "|", "b"];
        assert_ne!(body_from_tokens(before), body_from_tokens(after));
    }

    #[test]
    fn body_empty_is_empty() {
        let none: [&str; 0] = [];
        assert_eq!(body_from_tokens(none), "");
    }
}
