//! Scheme-tagged body hashing for seal values.
//!
//! A seal's lockfile value is `<scheme>:<hex>`. The scheme identifier
//! names which tokens were hashed and how; this module dispatches on it.
//! Today only `body` exists — FNV-1a over the function body's
//! normalized token stream (whitespace / line comments / block comments
//! stripped by the resolver already).
//!
//! New schemes (`sig`, custom) can be added here without changing
//! lockfile format or macro/seal subcommand wiring.

use crate::fnv::fnv1a_64;
use crate::resolver::ItemInfo;

pub const DEFAULT_SCHEME: &str = "body";

// satisfies: hash::body_scheme
pub fn compute(scheme: &str, info: &ItemInfo) -> Result<String, String> {
    match scheme {
        "body" => {
            let tokens = info.body_tokens.as_deref()
                .ok_or_else(|| "`body` scheme requires a resolvable function body (struct/trait/const items have none)".to_string())?;
            Ok(format!("body:{:016x}", fnv1a_64(tokens.as_bytes())))
        }
        other => Err(format!("unknown hash scheme: `{}`", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(body: &str) -> ItemInfo {
        ItemInfo { path: "x::y".into(), body_tokens: Some(body.into()) }
    }

    #[test]
    #[rubric::verifies(crate::reqs::hash::body_scheme)]
    fn body_scheme_is_deterministic() {
        let a = compute("body", &info("let_=1;")).unwrap();
        let b = compute("body", &info("let_=1;")).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("body:"));
        assert_eq!(a.len(), "body:".len() + 16);
    }

    #[test]
    fn body_scheme_detects_change() {
        let a = compute("body", &info("let_=1;")).unwrap();
        let b = compute("body", &info("let_=2;")).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn body_scheme_requires_body() {
        let info = ItemInfo { path: "x".into(), body_tokens: None };
        assert!(compute("body", &info).is_err());
    }

    #[test]
    fn unknown_scheme_errors() {
        assert!(compute("sha256", &info("x")).is_err());
    }
}
