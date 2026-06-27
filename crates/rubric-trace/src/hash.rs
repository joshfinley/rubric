//! Scheme-tagged seal computation.
//!
//! A seal is `<scheme>:<16-hex>`. The scheme tag keeps the lockfile
//! format extensible to new hash subjects without a format break.

use crate::fnv::fnv1a_64;
use crate::normalize;

/// Body of a satisfier or verifier, tokens normalized.
pub const SCHEME_BODY: &str = "body";
/// Statement text of a requirement.
pub const SCHEME_STMT: &str = "stmt";

/// Compute a seal over already-normalized content.
// satisfies: SEAL-FORMAT
pub fn seal(scheme: &str, normalized: &str) -> String {
    format!("{scheme}:{:016x}", fnv1a_64(normalized.as_bytes()))
}

/// Seal a requirement statement (`stmt:` scheme), normalizing first.
pub fn statement_seal(text: &str) -> String {
    seal(SCHEME_STMT, &normalize::statement(text))
}

/// Seal an already-normalized body (`body:` scheme). The scanner produces
/// the normalized form via [`normalize::body_from_tokens`].
pub fn body_seal(normalized_body: &str) -> String {
    seal(SCHEME_BODY, normalized_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // verifies: SEAL-FORMAT
    #[test]
    fn format_is_scheme_colon_hex16() {
        let s = seal(SCHEME_BODY, "fnbody");
        let (scheme, hex) = s.split_once(':').unwrap();
        assert_eq!(scheme, "body");
        assert_eq!(hex.len(), 16);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
