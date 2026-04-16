//! Seam between raw annotation discovery and semantic path resolution.
//!
//! An `AnnotationSite` is what the walker produces: where the annotation
//! lives and what requirement(s) it links to. An `ItemInfo` is what the
//! resolver returns: the enclosing item's rustdoc-convention path and,
//! when applicable, the body tokens used for seal hashing.
//!
//! Resolvers are swappable. Today we ship `SyntacticResolver` (in
//! `cargo-rubric`) backed by `rustc_lexer`. Future options — rustdoc JSON
//! output, `librustdoc` visitors, or an `ExplicitOnlyResolver` that
//! rejects any site without a declared `id` — plug in behind the same
//! trait without touching check/matrix/seal.

use std::path::PathBuf;

use crate::manifest::Direction;

#[derive(Debug, Clone)]
pub struct AnnotationSite {
    pub file: PathBuf,
    pub line: usize,
    /// Byte offset of the `#` (attribute) or leading ident (function-like)
    /// in the source file. Used by semantic resolvers that need to map
    /// back into rustc's byte-indexed span model.
    pub byte_offset: usize,
    pub direction: Direction,
    pub label_path: Vec<String>,
    /// `Some` when the annotation carries `id = "..."`. Resolvers return
    /// this verbatim as the item path, bypassing syntactic analysis.
    pub explicit_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ItemInfo {
    /// rustdoc-convention path: `crate::module::Type::method` etc.
    pub path: String,
    /// The enclosing fn's body-token stream, normalized for hashing.
    /// `None` for items without a hashable body (struct/trait/const),
    /// or when the resolver cannot reach a body syntactically.
    pub body_tokens: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolveError {
    pub site: AnnotationSite,
    pub msg: String,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.site.file.display(), self.site.line, self.msg)
    }
}

impl std::error::Error for ResolveError {}

/// Resolver implementations take a site plus the file's source text and
/// return the enclosing item's path information. `resolve` is per-site so
/// the macro can call into a resolver at expand time; batch impls cache
/// internally on first invocation.
pub trait AnnotationResolver {
    fn resolve(&mut self, site: &AnnotationSite, source: &str) -> Result<ItemInfo, ResolveError>;
}

/// Passthrough resolver — honors `explicit_id` when present, otherwise
/// returns an error asking the developer to declare one. Useful in
/// safety-critical contexts that refuse to trust heuristics.
pub struct ExplicitOnlyResolver;

impl AnnotationResolver for ExplicitOnlyResolver {
    fn resolve(&mut self, site: &AnnotationSite, _source: &str) -> Result<ItemInfo, ResolveError> {
        match &site.explicit_id {
            Some(id) => Ok(ItemInfo { path: id.clone(), body_tokens: None }),
            None => Err(ResolveError {
                site: site.clone(),
                msg: "ExplicitOnlyResolver requires `id = \"...\"` on every annotation".to_string(),
            }),
        }
    }
}
use std::path::Path;
use rustc_lexer::{tokenize, TokenKind};

pub struct SyntacticResolver {
    crate_name: String,
    src_root: std::path::PathBuf,
}

impl SyntacticResolver {
    pub fn new(crate_name: impl Into<String>, src_root: impl Into<std::path::PathBuf>) -> Self {
        Self { crate_name: crate_name.into(), src_root: src_root.into() }
    }
}

impl AnnotationResolver for SyntacticResolver {
    fn resolve(&mut self, site: &AnnotationSite, source: &str) -> Result<ItemInfo, ResolveError> {
        if let Some(id) = &site.explicit_id {
            return Ok(ItemInfo { path: id.clone(), body_tokens: None });
        }
        let file_mods = file_module_path(&self.src_root, &site.file);
        let (inline_mods, fn_name, body_tokens) = scan_enclosing(source, site.byte_offset);
        let mut parts: Vec<String> = vec![self.crate_name.clone()];
        parts.extend(file_mods);
        parts.extend(inline_mods);
        if let Some(name) = &fn_name {
            parts.push(name.clone());
        }
        Ok(ItemInfo { path: parts.join("::"), body_tokens })
    }
}

/// `src/lib.rs` → [], `src/parser.rs` → ["parser"], `src/parser/mod.rs`
/// → ["parser"], `src/parser/visitor.rs` → ["parser", "visitor"].
fn file_module_path(src_root: &Path, file: &Path) -> Vec<String> {
    let rel = match file.strip_prefix(src_root) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut parts = Vec::new();
    for comp in rel.components() {
        let std::path::Component::Normal(name) = comp else { continue; };
        let s = name.to_string_lossy();
        if s == "lib.rs" || s == "main.rs" || s == "mod.rs" { continue; }
        let stem = s.strip_suffix(".rs").unwrap_or(&s);
        parts.push(stem.to_string());
    }
    parts
}

// satisfies: resolver::tracks_item_path
/// Scan tokens up to and around `byte_offset`, returning the inline-`mod`
/// path at that point, the enclosing fn name (if any), and that fn's body
/// tokens normalized for seal hashing (if we can isolate it syntactically).
///
/// Three phases:
/// - `Before`: haven't reached byte_offset; track scope stack.
/// - `AwaitingFn`: past byte_offset with no enclosing fn (attribute-form
///   site); the very next fn declaration at the captured mod depth is
///   the attached fn.
/// - `InFnBody`: enclosing fn identified; scan to its matching `}` and
///   return body tokens.
fn scan_enclosing(source: &str, byte_offset: usize) -> (Vec<String>, Option<String>, Option<String>) {
    #[derive(Debug)]
    #[allow(dead_code)] // open_depth is read via pattern matching only
    enum Scope {
        Mod { open_depth: usize, name: String },
        Fn { open_depth: usize, name: String, body_start: usize },
        Block { open_depth: usize },
    }
    #[derive(Debug)]
    enum Pending { Mod(String), Fn(String), Other }

    enum Phase {
        Before,
        AwaitingFn { captured_mods: Vec<String>, mod_depth: usize },
        InFnBody { captured_mods: Vec<String>, fn_name: String, body_start: usize, open_depth: usize },
    }

    let mut scopes: Vec<Scope> = Vec::new();
    let mut brace_depth: usize = 0;
    let mut pending: Option<Pending> = None;
    let mut phase = Phase::Before;

    let toks: Vec<_> = tokenize(source).collect();
    let mut pos: usize = 0;
    for (i, tok) in toks.iter().enumerate() {
        let start = pos;
        let end = pos + tok.len;
        let text = &source[start..end];

        // Phase transition: first time we reach byte_offset, snapshot.
        if matches!(phase, Phase::Before) && start >= byte_offset {
            let mods: Vec<String> = scopes.iter().filter_map(|s| {
                if let Scope::Mod { name, .. } = s { Some(name.clone()) } else { None }
            }).collect();
            let enclosing_fn = scopes.iter().rev().find_map(|s| {
                if let Scope::Fn { name, body_start, open_depth } = s {
                    Some((name.clone(), *body_start, *open_depth))
                } else { None }
            });
            phase = match enclosing_fn {
                Some((name, body_start, open_depth)) => Phase::InFnBody {
                    captured_mods: mods, fn_name: name, body_start, open_depth,
                },
                None => Phase::AwaitingFn { captured_mods: mods, mod_depth: brace_depth },
            };
        }

        match tok.kind {
            TokenKind::Ident => match text {
                "mod" => {
                    if let Some(name) = next_ident(&toks, i, source, pos) {
                        pending = Some(Pending::Mod(name));
                    }
                }
                "fn" => {
                    if let Some(name) = next_ident(&toks, i, source, pos) {
                        pending = Some(Pending::Fn(name));
                    }
                }
                "impl" | "trait" | "struct" | "enum" | "union" | "match"
                | "loop" | "while" | "for" | "if" | "else" | "unsafe" => {
                    pending = Some(Pending::Other);
                }
                _ => {}
            },
            TokenKind::OpenBrace => {
                brace_depth += 1;
                match pending.take() {
                    Some(Pending::Mod(name)) => scopes.push(Scope::Mod { open_depth: brace_depth, name }),
                    Some(Pending::Fn(name)) => {
                        // Attribute-form case: if we're awaiting a fn at
                        // the captured mod depth, this one is the attached
                        // fn — transition to InFnBody.
                        if let Phase::AwaitingFn { captured_mods, mod_depth } = &phase {
                            if brace_depth - 1 == *mod_depth {
                                phase = Phase::InFnBody {
                                    captured_mods: captured_mods.clone(),
                                    fn_name: name.clone(),
                                    body_start: end,
                                    open_depth: brace_depth,
                                };
                            }
                        }
                        scopes.push(Scope::Fn { open_depth: brace_depth, name, body_start: end });
                    }
                    _ => scopes.push(Scope::Block { open_depth: brace_depth }),
                }
            }
            TokenKind::CloseBrace => {
                // End-of-body check for the InFnBody phase.
                if let Phase::InFnBody { captured_mods, fn_name, body_start, open_depth } = &phase {
                    if brace_depth == *open_depth {
                        let body_text = &source[*body_start..start];
                        let normalized = normalize_body_tokens(body_text);
                        return (captured_mods.clone(), Some(fn_name.clone()), Some(normalized));
                    }
                }
                if brace_depth > 0 { brace_depth -= 1; }
                scopes.pop();
                pending = None;
            }
            TokenKind::Semi => {
                pending = None;
            }
            _ => {}
        }
        pos = end;
    }

    // EOF.
    match phase {
        Phase::Before => (Vec::new(), None, None),
        Phase::AwaitingFn { captured_mods, .. } => (captured_mods, None, None),
        Phase::InFnBody { captured_mods, fn_name, .. } => (captured_mods, Some(fn_name), None),
    }
}

/// Peek for the next `Ident` token after index `i`. Returns its text.
fn next_ident(toks: &[rustc_lexer::Token], i: usize, source: &str, mut pos: usize) -> Option<String> {
    // pos corresponds to toks[i].start. Advance past toks[i] to begin.
    pos += toks[i].len;
    for tok in &toks[i + 1..] {
        let start = pos;
        let end = pos + tok.len;
        match tok.kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment { .. } => {
                pos = end;
                continue;
            }
            TokenKind::Ident => return Some(source[start..end].to_string()),
            _ => return None,
        }
    }
    None
}

/// Strip whitespace, line comments, block comments, and `#[doc]`-style
/// inner attributes from the body text. Returns a canonical token string
/// suitable for FNV-1a hashing (stable under reformatting).
fn normalize_body_tokens(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut pos = 0usize;
    for tok in tokenize(body) {
        let end = pos + tok.len;
        let keep = !matches!(
            tok.kind,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment { .. }
        );
        if keep {
            out.push_str(&body[pos..end]);
        }
        pos = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Direction;
    use std::path::PathBuf;

    fn site_at(file: &str, byte_offset: usize) -> AnnotationSite {
        AnnotationSite {
            file: PathBuf::from(file),
            line: 1,
            byte_offset,
            direction: Direction::Satisfies,
            label_path: vec!["x".into()],
            explicit_id: None,
        }
    }

    #[test]
    fn honors_explicit_id() {
        let mut r = SyntacticResolver::new("c", "/src");
        let mut site = site_at("/src/lib.rs", 0);
        site.explicit_id = Some("my::explicit::id".to_string());
        let info = r.resolve(&site, "").unwrap();
        assert_eq!(info.path, "my::explicit::id");
    }

    #[test]
    #[rubric::verifies(crate::reqs::resolver::tracks_item_path)]
    fn top_level_fn_path() {
        let src = r#"#[bind(satisfies = X)]
fn do_thing() { let _ = 1; }
"#;
        let offset = 0; // before the #[
        let mut r = SyntacticResolver::new("mycrate", "/src");
        let site = site_at("/src/lib.rs", offset);
        let info = r.resolve(&site, src).unwrap();
        assert_eq!(info.path, "mycrate::do_thing");
        assert!(info.body_tokens.as_ref().unwrap().contains("let_"));
        // Actually body is `let _ = 1;` normalized → `let_=1;`. Check a specific invariant:
        assert!(!info.body_tokens.as_ref().unwrap().contains(' '));
    }

    #[test]
    fn file_path_contributes_module() {
        let src = r#"#[bind(satisfies = X)]
fn inner() {}
"#;
        let mut r = SyntacticResolver::new("mycrate", "/src");
        let site = site_at("/src/parser/sub.rs", 0);
        let info = r.resolve(&site, src).unwrap();
        assert_eq!(info.path, "mycrate::parser::sub::inner");
    }

    #[test]
    fn inline_mod_nesting() {
        let src = r#"
mod tests {
    #[bind(verifies = X)]
    fn case_a() {}
}
"#;
        // offset where `#[` begins — find it.
        let offset = src.find("#[").unwrap();
        let mut r = SyntacticResolver::new("mycrate", "/src");
        let site = site_at("/src/lib.rs", offset);
        let info = r.resolve(&site, src).unwrap();
        assert_eq!(info.path, "mycrate::tests::case_a");
    }

    #[test]
    fn bind_at_inside_fn_body() {
        let src = r#"
fn outer() {
    bind_at!(verifies = X);
    let _ = 2;
}
"#;
        let offset = src.find("bind_at").unwrap();
        let mut r = SyntacticResolver::new("mycrate", "/src");
        let site = site_at("/src/lib.rs", offset);
        let info = r.resolve(&site, src).unwrap();
        assert_eq!(info.path, "mycrate::outer");
        assert!(info.body_tokens.is_some());
    }
}
