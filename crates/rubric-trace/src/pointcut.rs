//! Quantified selectors over scanned items (join points).
//!
//! A requirement's `cover = "<designator>"` names a set of items by
//! visibility, kind, and module scope. The census check then requires
//! every matched item to be cited.
//!
//! Grammar:
//!
//! ```text
//! pointcut := vis [ kind ] "within" scope
//! vis      := "pub" | "pub(crate)" | "any"
//! kind     := "fn"|"struct"|"enum"|"union"|"const"|"static"|"type"|"trait"|"mod"|"item"
//! scope    := segment ("::" segment)* [ "::*" ]
//! ```
//!
//! Examples: `pub within crate::audited`, `pub fn within crate::api::*`.

use crate::check::{ItemFacts, ItemKind, Visibility};

/// Holds a visibility predicate, an optional kind filter, and a module
/// scope prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointcut {
    pub vis: VisPred,
    pub kind: KindPred,
    /// Module prefix, with any trailing `::*` already stripped.
    pub scope: String,
}

/// Visibility predicate. `Pub` matches fully public items. `PubCrate`
/// matches `pub(crate)` and wider. `Any` matches every visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisPred {
    Pub,
    PubCrate,
    Any,
}

/// Kind filter. `Any` (the `item` keyword, or no kind) matches every kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindPred {
    Any,
    Only(ItemKind),
}

/// Parse a `cover` designator. Errors include a reason string.
// satisfies: POINTCUT-PARSE
pub fn parse(s: &str) -> Result<Pointcut, String> {
    let toks: Vec<&str> = s.split_whitespace().collect();
    let within = toks
        .iter()
        .position(|t| *t == "within")
        .ok_or_else(|| format!("pointcut `{s}` must contain `within`"))?;
    let head = &toks[..within];
    let tail = &toks[within + 1..];

    if head.is_empty() {
        return Err(format!("pointcut `{s}` needs a visibility before `within`"));
    }
    if head.len() > 2 {
        return Err(format!("pointcut `{s}` has extra tokens before `within`"));
    }
    if tail.len() != 1 {
        return Err(format!("pointcut `{s}` needs exactly one scope path after `within`"));
    }

    let vis = parse_vis(head[0])?;
    let kind = match head.get(1) {
        None => KindPred::Any,
        Some(k) => parse_kind(k)?,
    };
    let scope = tail[0].strip_suffix("::*").unwrap_or(tail[0]).to_string();
    if scope.is_empty() {
        return Err(format!("pointcut `{s}` has an empty scope"));
    }
    if scope.contains('*') {
        return Err(format!(
            "pointcut `{s}` has a stray `*` in its scope (only a trailing `::*` is allowed)"
        ));
    }
    Ok(Pointcut { vis, kind, scope })
}

fn parse_vis(s: &str) -> Result<VisPred, String> {
    match s {
        "pub" => Ok(VisPred::Pub),
        "pub(crate)" => Ok(VisPred::PubCrate),
        "any" => Ok(VisPred::Any),
        other => Err(format!(
            "unknown visibility `{other}` in pointcut (expected `pub`, `pub(crate)`, or `any`)"
        )),
    }
}

fn parse_kind(s: &str) -> Result<KindPred, String> {
    let k = match s {
        "item" => return Ok(KindPred::Any),
        "fn" => ItemKind::Fn,
        "struct" => ItemKind::Struct,
        "enum" => ItemKind::Enum,
        "union" => ItemKind::Union,
        "const" => ItemKind::Const,
        "static" => ItemKind::Static,
        "type" => ItemKind::TypeAlias,
        "trait" => ItemKind::Trait,
        "mod" => ItemKind::Mod,
        other => return Err(format!("unknown kind `{other}` in pointcut")),
    };
    Ok(KindPred::Only(k))
}

impl Pointcut {
    /// Whether a scanned item is one of this pointcut's join points.
    // satisfies: POINTCUT-MATCH
    pub fn matches(&self, item: &ItemFacts) -> bool {
        self.vis_ok(item.vis) && self.kind_ok(item.kind) && self.scope_ok(&item.path)
    }

    fn vis_ok(&self, v: Visibility) -> bool {
        match self.vis {
            VisPred::Any => true,
            VisPred::Pub => v == Visibility::Pub,
            VisPred::PubCrate => matches!(v, Visibility::Pub | Visibility::PubCrate),
        }
    }

    fn kind_ok(&self, k: ItemKind) -> bool {
        match self.kind {
            KindPred::Any => true,
            KindPred::Only(want) => k == want,
        }
    }

    fn scope_ok(&self, path: &str) -> bool {
        path == self.scope || path.starts_with(&format!("{}::", self.scope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(path: &str, vis: Visibility, kind: ItemKind) -> ItemFacts {
        ItemFacts {
            path: path.into(),
            resolved: true,
            is_test: false,
            is_ignored: false,
            vis,
            kind,
            body: None,
            signature: None,
            evidence_seal: None,
        }
    }

    // verifies: POINTCUT-PARSE
    #[test]
    fn parses_vis_kind_scope() {
        let p = parse("pub fn within crate::api").unwrap();
        assert_eq!(p.vis, VisPred::Pub);
        assert_eq!(p.kind, KindPred::Only(ItemKind::Fn));
        assert_eq!(p.scope, "crate::api");
    }

    // verifies: POINTCUT-PARSE
    #[test]
    fn parses_without_kind_and_strips_wildcard() {
        let p = parse("pub within crate::api::*").unwrap();
        assert_eq!(p.kind, KindPred::Any);
        assert_eq!(p.scope, "crate::api");
    }

    // verifies: POINTCUT-PARSE
    #[test]
    fn item_keyword_is_any_kind() {
        assert_eq!(parse("any item within crate").unwrap().kind, KindPred::Any);
    }

    // verifies: POINTCUT-PARSE
    #[test]
    fn rejects_garbage() {
        assert!(parse("pub fn crate::api").is_err()); // no `within`
        assert!(parse("within crate").is_err()); // no visibility
        assert!(parse("pub fn within").is_err()); // no scope
        assert!(parse("pub blah within crate").is_err()); // bad kind
        assert!(parse("pub fn extra within crate").is_err()); // extra head token
        assert!(parse("pub fn within crate::api::**").is_err()); // stray wildcard
        assert!(parse("pub fn within crate::*::api").is_err()); // stray wildcard
    }

    // verifies: POINTCUT-MATCH
    #[test]
    fn matches_pub_fn_in_scope_only() {
        let p = parse("pub fn within crate::api").unwrap();
        assert!(p.matches(&it("crate::api::connect", Visibility::Pub, ItemKind::Fn)));
        assert!(p.matches(&it("crate::api::sub::open", Visibility::Pub, ItemKind::Fn)));
        assert!(!p.matches(&it("crate::other::f", Visibility::Pub, ItemKind::Fn))); // out of scope
        assert!(!p.matches(&it("crate::api::S", Visibility::Pub, ItemKind::Struct))); // wrong kind
        assert!(!p.matches(&it("crate::api::g", Visibility::Private, ItemKind::Fn))); // wrong vis
    }

    // verifies: POINTCUT-MATCH
    #[test]
    fn pub_crate_predicate_includes_pub() {
        let p = parse("pub(crate) within crate").unwrap();
        assert!(p.matches(&it("crate::a", Visibility::PubCrate, ItemKind::Fn)));
        assert!(p.matches(&it("crate::b", Visibility::Pub, ItemKind::Fn)));
        assert!(!p.matches(&it("crate::c", Visibility::Private, ItemKind::Fn)));
    }

    // verifies: POINTCUT-MATCH
    #[test]
    fn scope_matches_exact_path() {
        let p = parse("any item within crate::api").unwrap();
        assert!(p.matches(&it("crate::api", Visibility::Private, ItemKind::Mod)));
        assert!(!p.matches(&it("crate::apiary", Visibility::Pub, ItemKind::Fn))); // prefix is not a path boundary
    }
}
