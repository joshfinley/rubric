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
/// An item's signature, tokens normalized (visibility through the body
/// brace, excluding the block).
pub const SCHEME_SIG: &str = "sig";
/// Signature and body together. A change to either breaks the seal.
pub const SCHEME_FULL: &str = "full";
/// Raw bytes of an `external:` evidence file.
pub const SCHEME_FILE: &str = "file";
/// A requirement's attestation root, hashed over its current leg seals.
pub const SCHEME_ATTEST: &str = "attest";

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

/// Seal an already-normalized signature (`sig:` scheme).
// satisfies: SEAL-SIG
pub fn signature_seal(normalized_sig: &str) -> String {
    seal(SCHEME_SIG, normalized_sig)
}

/// Seal signature and body together (`full:` scheme). A NUL between them
/// keeps a token from colliding across the boundary.
// satisfies: SEAL-SIG
pub fn full_seal(normalized_sig: &str, normalized_body: &str) -> String {
    seal(SCHEME_FULL, &format!("{normalized_sig}\u{0}{normalized_body}"))
}

/// Seal the raw bytes of an evidence file (`file:` scheme).
// satisfies: SEAL-FILE
pub fn file_seal(bytes: &[u8]) -> String {
    format!("{SCHEME_FILE}:{:016x}", fnv1a_64(bytes))
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

    // verifies: SEAL-SIG
    #[test]
    fn sig_and_full_carry_their_schemes() {
        assert!(signature_seal("pub fn f ( )").starts_with("sig:"));
        assert!(full_seal("pub fn f ( )", "body").starts_with("full:"));
    }

    // verifies: SEAL-SIG
    #[test]
    fn full_seal_trips_on_signature_or_body() {
        let base = full_seal("pub fn f ( )", "a");
        assert_ne!(base, full_seal("fn f ( )", "a")); // signature moved
        assert_ne!(base, full_seal("pub fn f ( )", "b")); // body moved
    }

    // verifies: SEAL-FILE
    #[test]
    fn file_seal_hashes_raw_bytes() {
        assert!(file_seal(b"snapshot").starts_with("file:"));
        assert_ne!(file_seal(b"snapshot"), file_seal(b"snapshob"));
    }
}
