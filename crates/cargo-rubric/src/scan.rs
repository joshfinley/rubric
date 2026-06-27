//! Source scanner: lex Rust files and extract the attribution facts the
//! pure oracle needs.
//!
//! Lexing uses `rustc_lexer`, the compiler's own lexer, so token
//! boundaries match what rustc sees. On top of the flat token stream we
//! run a lightweight structural walk that tracks module/impl nesting,
//! recognizes the three annotation forms, and captures function bodies.
//!
//! What the walk understands:
//!
//! - Free functions, functions in inline `mod` blocks, and inherent and
//!   trait-impl methods. A method's path includes its type, e.g.
//!   `crate::voter::Tmr::vote`, not `crate::voter::vote`.
//! - The three annotation forms: `// satisfies:` / `// verifies:`
//!   comments, `#[cfg_attr(any(), satisfies(...))]`, and the bare
//!   `#[satisfies(...)]` / `#[verifies(...)]` attributes (including
//!   path-qualified spellings). Multiple comma-separated labels are read.
//! - `#[test]` and `#[ignore]` (and path-qualified `#[tokio::test]`),
//!   feeding the dead-verifier check.
//!
//! Punt list (cite via `rubric.toml` declaration instead): closures,
//! items generated inside macro bodies, and annotations inside function
//! bodies. Function-like macro invocations at item level are skipped
//! whole so their contents are never misread as items.

use std::collections::BTreeMap;

use rubric_trace::check::{Citation, Direction, ItemFacts, Scan};
use rubric_trace::lock::Origin;
use rubric_trace::manifest::Manifest;
use rubric_trace::normalize;
use rustc_lexer::{tokenize, TokenKind};

/// One source file with its module path prefix (e.g. `["crate", "voter"]`).
pub struct FileInput {
    pub module_path: Vec<String>,
    pub source: String,
}

/// Scan a set of files plus the manifest's declared paths into the pure
/// oracle's input. Citations are sorted for reproducibility; output is
/// independent of file order.
pub fn scan_files(files: &[FileInput], manifest: &Manifest) -> Scan {
    let mut items: BTreeMap<String, ItemFacts> = BTreeMap::new();
    let mut citations: Vec<Citation> = Vec::new();

    for f in files {
        let parsed = parse(&f.module_path, &f.source);
        for it in parsed.items {
            items.insert(it.path.clone(), it);
        }
        for (label, path, dir) in parsed.annotations {
            citations.push(Citation {
                req_label: label,
                item_path: path,
                direction: dir,
                origin: Origin::Annotation,
            });
        }
    }

    // Declared citations from the manifest.
    for r in &manifest.requirements {
        for p in &r.satisfied_by {
            citations.push(Citation {
                req_label: r.label.clone(),
                item_path: p.clone(),
                direction: Direction::Satisfies,
                origin: Origin::Declared,
            });
        }
        for p in &r.verified_by {
            citations.push(Citation {
                req_label: r.label.clone(),
                item_path: p.clone(),
                direction: Direction::Verifies,
                origin: Origin::Declared,
            });
        }
    }

    // Every cited path needs an ItemFacts. Discovered items already have
    // one. For the rest, an `external:` path resolves to file evidence
    // (existence checked by the caller), and any other unknown path is a
    // dangling declaration the oracle reports as unresolved.
    for c in &citations {
        if items.contains_key(&c.item_path) {
            continue;
        }
        let external = c.item_path.starts_with("external:");
        items.insert(
            c.item_path.clone(),
            ItemFacts {
                path: c.item_path.clone(),
                resolved: external,
                is_test: false,
                is_ignored: false,
                body: None,
            },
        );
    }

    citations.sort_by(|a, b| {
        (&a.req_label, &a.item_path, dir_ord(a.direction))
            .cmp(&(&b.req_label, &b.item_path, dir_ord(b.direction)))
    });

    Scan { citations, items: items.into_values().collect() }
}

fn dir_ord(d: Direction) -> u8 {
    match d {
        Direction::Satisfies => 0,
        Direction::Verifies => 1,
    }
}

struct ParsedFile {
    items: Vec<ItemFacts>,
    annotations: Vec<(String, String, Direction)>,
}

fn parse(module_path: &[String], src: &str) -> ParsedFile {
    let toks = lex(src);
    let mut p = FileParser {
        src,
        toks,
        i: 0,
        path: module_path.to_vec(),
        frames: Vec::new(),
        pending: Pending::default(),
        items: Vec::new(),
        annotations: Vec::new(),
    };
    p.run();
    ParsedFile { items: p.items, annotations: p.annotations }
}

#[derive(Clone, Copy)]
struct Tok {
    kind: TokenKind,
    start: usize,
    end: usize,
}

fn lex(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut pos = 0;
    for t in tokenize(src) {
        out.push(Tok { kind: t.kind, start: pos, end: pos + t.len });
        pos += t.len;
    }
    out
}

fn is_trivia(k: TokenKind) -> bool {
    matches!(k, TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment { .. })
}

#[derive(Default)]
struct Pending {
    satisfies: Vec<String>,
    verifies: Vec<String>,
    is_test: bool,
    is_ignored: bool,
}

/// A scope frame. `true` means it pushed a path segment (`mod`/`impl`/
/// `trait`) that must pop when the frame closes.
struct FileParser<'a> {
    src: &'a str,
    toks: Vec<Tok>,
    i: usize,
    path: Vec<String>,
    frames: Vec<bool>,
    pending: Pending,
    items: Vec<ItemFacts>,
    annotations: Vec<(String, String, Direction)>,
}

impl<'a> FileParser<'a> {
    fn slice(&self, idx: usize) -> &'a str {
        &self.src[self.toks[idx].start..self.toks[idx].end]
    }

    fn kind(&self, idx: usize) -> TokenKind {
        self.toks[idx].kind
    }

    /// Index of the next non-trivia token at or after `from`.
    fn next_sig(&self, from: usize) -> Option<usize> {
        (from..self.toks.len()).find(|&j| !is_trivia(self.toks[j].kind))
    }

    /// Given an opening delimiter at `open_idx`, the index of its match.
    fn skip_balanced(&self, open_idx: usize, open: TokenKind, close: TokenKind) -> usize {
        let mut depth = 0usize;
        let mut j = open_idx;
        while j < self.toks.len() {
            let k = self.toks[j].kind;
            if k == open {
                depth += 1;
            } else if k == close {
                depth -= 1;
                if depth == 0 {
                    return j;
                }
            }
            j += 1;
        }
        self.toks.len().saturating_sub(1)
    }

    /// Skip a `<...>` generic group, treating `->` as an arrow (not a
    /// closing angle) and `>>` as two closers.
    fn skip_angles(&self, open_idx: usize) -> usize {
        let mut depth = 0usize;
        let mut prev: Option<TokenKind> = None;
        let mut j = open_idx;
        while j < self.toks.len() {
            let k = self.toks[j].kind;
            match k {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    if prev != Some(TokenKind::Minus) && depth > 0 {
                        depth -= 1;
                        if depth == 0 {
                            return j;
                        }
                    }
                }
                _ => {}
            }
            if !is_trivia(k) {
                prev = Some(k);
            }
            j += 1;
        }
        self.toks.len().saturating_sub(1)
    }

    fn run(&mut self) {
        while self.i < self.toks.len() {
            let k = self.kind(self.i);
            match k {
                TokenKind::Whitespace => self.i += 1,
                TokenKind::LineComment | TokenKind::BlockComment { .. } => {
                    self.scan_comment(self.i);
                    self.i += 1;
                }
                TokenKind::Pound => self.handle_pound(),
                TokenKind::OpenBrace => {
                    self.frames.push(false);
                    self.i += 1;
                }
                TokenKind::CloseBrace => {
                    if let Some(seg) = self.frames.pop() {
                        if seg {
                            self.path.pop();
                        }
                    }
                    self.i += 1;
                }
                TokenKind::Semi => {
                    self.pending = Pending::default();
                    self.i += 1;
                }
                TokenKind::Ident => self.handle_ident(),
                _ => self.i += 1,
            }
        }
    }

    fn handle_ident(&mut self) {
        match self.slice(self.i) {
            "fn" => self.handle_fn(),
            "mod" => self.handle_mod(),
            "impl" => self.handle_impl(),
            "trait" => self.handle_trait(),
            // Transparent modifiers: keep pending markers for the fn ahead.
            "pub" | "async" | "unsafe" | "const" | "extern" | "default" | "move"
            | "static" | "auto" | "dyn" => self.i += 1,
            other => {
                if self.try_skip_macro() {
                    return;
                }
                // Other item starters end the attribute run.
                if matches!(
                    other,
                    "struct" | "enum" | "union" | "use" | "type" | "let"
                        | "return" | "match" | "if" | "while" | "for" | "loop"
                ) {
                    self.pending = Pending::default();
                }
                self.i += 1;
            }
        }
    }

    /// `name!( ... )` / `name! { ... }` / `name![ ... ]`, skip whole.
    fn try_skip_macro(&mut self) -> bool {
        let bang = match self.next_sig(self.i + 1) {
            Some(j) if self.kind(j) == TokenKind::Not => j,
            _ => return false,
        };
        let op = match self.next_sig(bang + 1) {
            Some(j) => j,
            None => return false,
        };
        let (o, c) = match self.kind(op) {
            TokenKind::OpenParen => (TokenKind::OpenParen, TokenKind::CloseParen),
            TokenKind::OpenBracket => (TokenKind::OpenBracket, TokenKind::CloseBracket),
            TokenKind::OpenBrace => (TokenKind::OpenBrace, TokenKind::CloseBrace),
            _ => return false,
        };
        let close = self.skip_balanced(op, o, c);
        self.i = close + 1;
        true
    }

    fn handle_pound(&mut self) {
        // `#[ ... ]` or `#![ ... ]`.
        let mut j = self.i + 1;
        if let Some(n) = self.next_sig(j) {
            if self.kind(n) == TokenKind::Not {
                j = n + 1;
            }
        }
        if let Some(n) = self.next_sig(j) {
            if self.kind(n) == TokenKind::OpenBracket {
                let close = self.skip_balanced(n, TokenKind::OpenBracket, TokenKind::CloseBracket);
                self.parse_attr(n + 1, close);
                self.i = close + 1;
                return;
            }
        }
        self.i += 1;
    }

    /// Inspect an attribute's inner token range `[start, end)` for
    /// `test`/`ignore` and for `satisfies(...)`/`verifies(...)`.
    fn parse_attr(&mut self, start: usize, end: usize) {
        // Leading attribute name path: `a::b::name`.
        if let Some(name) = self.leading_attr_name(start, end) {
            match name {
                "test" => self.pending.is_test = true,
                "ignore" => self.pending.is_ignored = true,
                _ => {}
            }
        }
        // Scan for satisfies/verifies anywhere inside (covers cfg_attr).
        let mut j = start;
        while j < end {
            if self.kind(j) == TokenKind::Ident {
                let dir = match self.slice(j) {
                    "satisfies" => Some(Direction::Satisfies),
                    "verifies" => Some(Direction::Verifies),
                    _ => None,
                };
                if let Some(dir) = dir {
                    if let Some(op) = self.next_sig(j + 1) {
                        if op < end && self.kind(op) == TokenKind::OpenParen {
                            let cp = self.skip_balanced(op, TokenKind::OpenParen, TokenKind::CloseParen);
                            let inner = &self.src[self.toks[op].end..self.toks[cp].start];
                            for label in split_labels(inner) {
                                match dir {
                                    Direction::Satisfies => self.pending.satisfies.push(label),
                                    Direction::Verifies => self.pending.verifies.push(label),
                                }
                            }
                            j = cp + 1;
                            continue;
                        }
                    }
                }
            }
            j += 1;
        }
    }

    /// Last identifier of a leading `a::b::name` path in `[start, end)`.
    fn leading_attr_name(&self, start: usize, end: usize) -> Option<&'a str> {
        let mut j = self.next_sig(start)?;
        let mut last = None;
        while j < end {
            if self.kind(j) == TokenKind::Ident {
                last = Some(self.slice(j));
                // Expect `::` to continue the path, else stop.
                match self.next_sig(j + 1) {
                    Some(c1) if c1 < end && self.kind(c1) == TokenKind::Colon => {
                        match self.next_sig(c1 + 1) {
                            Some(c2) if c2 < end && self.kind(c2) == TokenKind::Colon => {
                                j = match self.next_sig(c2 + 1) {
                                    Some(n) => n,
                                    None => break,
                                };
                                continue;
                            }
                            _ => break,
                        }
                    }
                    _ => break,
                }
            } else {
                break;
            }
        }
        last
    }

    fn scan_comment(&mut self, idx: usize) {
        let text = self.slice(idx);
        let (dir, after) = match (text.find("satisfies:"), text.find("verifies:")) {
            (Some(s), Some(v)) if s < v => (Direction::Satisfies, &text[s + "satisfies:".len()..]),
            (Some(_), Some(v)) => (Direction::Verifies, &text[v + "verifies:".len()..]),
            (Some(s), None) => (Direction::Satisfies, &text[s + "satisfies:".len()..]),
            (None, Some(v)) => (Direction::Verifies, &text[v + "verifies:".len()..]),
            (None, None) => return,
        };
        // For block comments, stop at the terminator or a newline.
        let after = after.split("*/").next().unwrap_or(after);
        let after = after.lines().next().unwrap_or(after);
        for label in split_labels(after) {
            match dir {
                Direction::Satisfies => self.pending.satisfies.push(label),
                Direction::Verifies => self.pending.verifies.push(label),
            }
        }
    }

    fn handle_mod(&mut self) {
        let name_idx = match self.next_sig(self.i + 1) {
            Some(j) if matches!(self.kind(j), TokenKind::Ident | TokenKind::RawIdent) => j,
            _ => {
                self.i += 1;
                return;
            }
        };
        let name = self.slice(name_idx).to_string();
        self.pending = Pending::default();
        match self.next_sig(name_idx + 1) {
            Some(b) if self.kind(b) == TokenKind::OpenBrace => {
                self.path.push(name);
                self.frames.push(true);
                self.i = b + 1;
            }
            _ => self.i = name_idx + 1, // `mod foo;`
        }
    }

    fn handle_impl(&mut self) {
        self.pending = Pending::default();
        let mut j = self.i + 1;
        // Optional `impl<...>` generics.
        if let Some(n) = self.next_sig(j) {
            if self.kind(n) == TokenKind::Lt {
                j = self.skip_angles(n) + 1;
            }
        }
        let (segment, body_open) = self.parse_impl_type(j);
        if let Some(seg) = segment {
            self.path.push(seg);
            self.frames.push(true);
        } else {
            self.frames.push(false);
        }
        self.i = body_open + 1;
    }

    /// From `start`, find the self-type's last path identifier and the
    /// body `{`. Handles `impl Trait for Type` and generic arguments.
    fn parse_impl_type(&self, start: usize) -> (Option<String>, usize) {
        let mut last: Option<String> = None;
        let mut j = start;
        while j < self.toks.len() {
            let k = self.kind(j);
            match k {
                TokenKind::OpenBrace => return (last, j),
                TokenKind::Lt => {
                    j = self.skip_angles(j) + 1;
                    continue;
                }
                TokenKind::Ident => {
                    let w = self.slice(j);
                    if w == "where" {
                        // Scan on to the body brace.
                        let mut m = j;
                        while m < self.toks.len() && self.kind(m) != TokenKind::OpenBrace {
                            m += 1;
                        }
                        return (last, m);
                    } else if w == "for" {
                        last = None; // real self type comes after `for`
                    } else if w == "dyn" {
                        // skip
                    } else {
                        last = Some(w.to_string());
                    }
                }
                _ => {}
            }
            j += 1;
        }
        (last, self.toks.len().saturating_sub(1))
    }

    fn handle_trait(&mut self) {
        self.pending = Pending::default();
        let name_idx = match self.next_sig(self.i + 1) {
            Some(j) if matches!(self.kind(j), TokenKind::Ident | TokenKind::RawIdent) => j,
            _ => {
                self.i += 1;
                return;
            }
        };
        let name = self.slice(name_idx).to_string();
        // Scan to the body brace (skipping generics / supertrait bounds).
        let mut j = name_idx + 1;
        let mut body = None;
        while j < self.toks.len() {
            match self.kind(j) {
                TokenKind::Lt => {
                    j = self.skip_angles(j) + 1;
                    continue;
                }
                TokenKind::OpenBrace => {
                    body = Some(j);
                    break;
                }
                TokenKind::Semi => break, // `trait Foo;` (marker trait decl, rare)
                _ => {}
            }
            j += 1;
        }
        match body {
            Some(b) => {
                self.path.push(name);
                self.frames.push(true);
                self.i = b + 1;
            }
            None => self.i = j + 1,
        }
    }

    fn handle_fn(&mut self) {
        let name_idx = match self.next_sig(self.i + 1) {
            Some(j) if matches!(self.kind(j), TokenKind::Ident | TokenKind::RawIdent) => j,
            _ => {
                self.i += 1;
                return;
            }
        };
        let name = self.slice(name_idx);
        let mut full = self.path.clone();
        full.push(name.to_string());
        let path = full.join("::");

        let pending = std::mem::take(&mut self.pending);

        let (body, after) = match self.find_fn_body(name_idx + 1) {
            Some(open) => {
                let close = self.skip_balanced(open, TokenKind::OpenBrace, TokenKind::CloseBrace);
                (Some(self.normalize_body(open, close)), close + 1)
            }
            None => (None, self.advance_past_decl(name_idx + 1)),
        };

        for l in &pending.satisfies {
            self.annotations.push((l.clone(), path.clone(), Direction::Satisfies));
        }
        for l in &pending.verifies {
            self.annotations.push((l.clone(), path.clone(), Direction::Verifies));
        }
        self.items.push(ItemFacts {
            path,
            resolved: true,
            is_test: pending.is_test,
            is_ignored: pending.is_ignored,
            body,
        });
        self.i = after;
    }

    /// First `{` at paren/bracket/angle depth 0, or `None` for a bodyless
    /// declaration (`;`).
    fn find_fn_body(&self, start: usize) -> Option<usize> {
        let (mut paren, mut bracket, mut angle) = (0usize, 0usize, 0usize);
        let mut prev: Option<TokenKind> = None;
        let mut j = start;
        while j < self.toks.len() {
            let k = self.kind(j);
            match k {
                TokenKind::OpenParen => paren += 1,
                TokenKind::CloseParen => paren = paren.saturating_sub(1),
                TokenKind::OpenBracket => bracket += 1,
                TokenKind::CloseBracket => bracket = bracket.saturating_sub(1),
                TokenKind::Lt => angle += 1,
                TokenKind::Gt => {
                    if prev != Some(TokenKind::Minus) {
                        angle = angle.saturating_sub(1);
                    }
                }
                TokenKind::OpenBrace if paren == 0 && bracket == 0 && angle == 0 => return Some(j),
                TokenKind::Semi if paren == 0 && bracket == 0 && angle == 0 => return None,
                _ => {}
            }
            if !is_trivia(k) {
                prev = Some(k);
            }
            j += 1;
        }
        None
    }

    fn advance_past_decl(&self, start: usize) -> usize {
        let mut j = start;
        while j < self.toks.len() {
            if self.kind(j) == TokenKind::Semi {
                return j + 1;
            }
            j += 1;
        }
        self.toks.len()
    }

    /// Significant tokens strictly inside `(open, close)`, joined by the
    /// core's body normalization policy.
    fn normalize_body(&self, open: usize, close: usize) -> String {
        let toks = (open + 1..close)
            .filter(|&j| !is_trivia(self.kind(j)))
            .map(|j| self.slice(j));
        normalize::body_from_tokens(toks)
    }
}

/// Split a comma-separated label list, keeping the leading label-char run
/// of each piece (`alnum`, `-`, `_`). Empty pieces are dropped.
fn split_labels(s: &str) -> Vec<String> {
    s.split(',')
        .filter_map(|piece| {
            let label: String = piece
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if label.is_empty() {
                None
            } else {
                Some(label)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_one(src: &str) -> ParsedFile {
        parse(&["crate".to_string()], src)
    }

    fn ann(p: &ParsedFile, label: &str) -> Vec<(String, Direction)> {
        p.annotations
            .iter()
            .filter(|(l, _, _)| l == label)
            .map(|(_, path, d)| (path.clone(), *d))
            .collect()
    }

    fn item<'a>(p: &'a ParsedFile, path: &str) -> &'a ItemFacts {
        p.items.iter().find(|i| i.path == path).expect("item not found")
    }

    #[test]
    fn comment_annotation_on_free_fn() {
        let src = "\
// satisfies: VOTER-1
pub fn vote(a: bool) -> bool { a }
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1"), vec![("crate::vote".to_string(), Direction::Satisfies)]);
        assert!(item(&p, "crate::vote").body.is_some());
    }

    #[test]
    fn cfg_attr_form() {
        let src = "\
#[cfg_attr(any(), satisfies(VOTER-1))]
fn vote() {}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1"), vec![("crate::vote".to_string(), Direction::Satisfies)]);
    }

    #[test]
    fn bare_attribute_form() {
        let src = "\
#[satisfies(VOTER-1)]
fn vote() {}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1")[0].1, Direction::Satisfies);
    }

    #[test]
    fn path_qualified_attribute() {
        let src = "\
#[rubric_trace_macros::verifies(VOTER-1)]
#[test]
fn t() { assert!(true); }
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1")[0].1, Direction::Verifies);
        assert!(item(&p, "crate::t").is_test);
    }

    #[test]
    fn multiple_labels() {
        let src = "\
// satisfies: A-1, B-2 , C-3
fn f() {}
";
        let p = scan_one(src);
        let labels: Vec<_> = p.annotations.iter().map(|(l, _, _)| l.clone()).collect();
        assert_eq!(labels, vec!["A-1", "B-2", "C-3"]);
    }

    #[test]
    fn test_and_ignore_flags() {
        let src = "\
// verifies: R-1
#[test]
#[ignore = \"flaky\"]
fn t() {}
";
        let p = scan_one(src);
        let it = item(&p, "crate::t");
        assert!(it.is_test);
        assert!(it.is_ignored);
    }

    #[test]
    fn tokio_test_is_a_test() {
        let src = "#[tokio::test]\nfn t() {}\n";
        let p = scan_one(src);
        assert!(item(&p, "crate::t").is_test);
    }

    #[test]
    fn fn_in_inline_mod() {
        let src = "\
mod voter {
    // satisfies: VOTER-1
    pub fn vote() {}
}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1")[0].0, "crate::voter::vote");
    }

    #[test]
    fn inherent_method_path_includes_type() {
        let src = "\
struct Tmr;
impl Tmr {
    // satisfies: VOTER-1
    pub fn vote(&self) -> bool { true }
}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1")[0].0, "crate::Tmr::vote");
    }

    #[test]
    fn trait_impl_method_uses_self_type() {
        let src = "\
impl Voter for Tmr {
    // satisfies: VOTER-1
    fn vote(&self) -> bool { true }
}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "VOTER-1")[0].0, "crate::Tmr::vote");
    }

    #[test]
    fn generic_impl_method_uses_base_type() {
        let src = "\
impl<T: Clone> Wrap<T> {
    // satisfies: W-1
    fn get(&self) -> T { unimplemented!() }
}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "W-1")[0].0, "crate::Wrap::get");
    }

    #[test]
    fn nested_mod_scoping_pops() {
        let src = "\
mod a {
    fn inner() {}
}
// satisfies: TOP-1
fn outer() {}
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "TOP-1")[0].0, "crate::outer");
        assert!(p.items.iter().any(|i| i.path == "crate::a::inner"));
        assert!(p.items.iter().any(|i| i.path == "crate::outer"));
    }

    #[test]
    fn body_is_reformatting_invariant() {
        let tight = scan_one("fn f() {(a&b)|(b&c)}\n");
        let loose = scan_one("fn f() {\n    ( a & b ) | ( b & c )\n}\n");
        assert_eq!(item(&tight, "crate::f").body, item(&loose, "crate::f").body);
    }

    #[test]
    fn body_changes_with_token_change() {
        let before = scan_one("fn f() { a & b }\n");
        let after = scan_one("fn f() { a | b }\n");
        assert_ne!(item(&before, "crate::f").body, item(&after, "crate::f").body);
    }

    #[test]
    fn comment_in_body_does_not_break_seal() {
        let with = scan_one("fn f() {\n    // a note\n    a + b\n}\n");
        let without = scan_one("fn f() { a + b }\n");
        assert_eq!(item(&with, "crate::f").body, item(&without, "crate::f").body);
    }

    #[test]
    fn slashes_in_string_are_not_a_comment_marker() {
        // `//` inside a string literal must not register as a comment.
        let src = "fn f() { let s = \"a // satisfies: GHOST b\"; }\n";
        let p = scan_one(src);
        assert!(p.annotations.is_empty());
    }

    #[test]
    fn macro_body_is_not_misparsed() {
        let src = "\
my_macro! {
    fn ghost() {}
}
// satisfies: REAL-1
fn real() {}
";
        let p = scan_one(src);
        assert!(!p.items.iter().any(|i| i.path == "crate::ghost"));
        assert_eq!(ann(&p, "REAL-1")[0].0, "crate::real");
    }

    #[test]
    fn bodyless_decl_has_no_body() {
        // A trait method declaration: recorded, but no body to seal.
        let src = "\
trait T {
    fn required(&self);
}
";
        let p = scan_one(src);
        let it = item(&p, "crate::T::required");
        assert!(it.body.is_none());
    }

    #[test]
    fn const_fn_keeps_annotation() {
        let src = "\
// satisfies: C-1
pub const fn k() -> u32 { 7 }
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "C-1")[0].0, "crate::k");
    }

    #[test]
    fn annotation_does_not_leak_past_semicolon() {
        let src = "\
// satisfies: STALE-1
const X: u32 = 5;
fn f() {}
";
        let p = scan_one(src);
        // The annotation attaches to nothing valid. It must not land on `f`.
        assert!(ann(&p, "STALE-1").iter().all(|(path, _)| path != "crate::f"));
    }

    #[test]
    fn private_fn_carries_annotation() {
        // Crates that avoid `pub` still get scanned. Keying is on `fn`.
        let src = "\
// satisfies: P-1
fn helper() -> u32 { 1 }
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "P-1")[0].0, "crate::helper");
    }

    #[test]
    fn pub_crate_fn_keeps_annotation() {
        let src = "\
// satisfies: P-2
pub(crate) fn helper() -> u32 { 1 }
";
        let p = scan_one(src);
        assert_eq!(ann(&p, "P-2")[0].0, "crate::helper");
    }

    #[test]
    fn integration_test_rooting() {
        // A file scanned under the `tests::api` root, as discovery assigns.
        let p = parse(&["tests".to_string(), "api".to_string()], "\
// verifies: R-1
#[test]
fn round_trips() { assert!(true); }
");
        let it = item(&p, "tests::api::round_trips");
        assert!(it.is_test);
        assert_eq!(ann(&p, "R-1")[0], ("tests::api::round_trips".to_string(), Direction::Verifies));
    }

    #[test]
    fn arrow_return_does_not_confuse_body_finder() {
        let src = "fn f<T: Fn() -> u32>(x: T) -> u32 { x() }\n";
        let p = scan_one(src);
        assert_eq!(item(&p, "crate::f").body, Some("x ( )".to_string()));
    }
}
