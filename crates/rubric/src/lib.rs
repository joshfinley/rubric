//! Attribute macros for requirement traceability.
//!
//! ```ignore
//! rubric::setup!();
//!
//! #[satisfies(crate::reqs::parser::header_magic)]
//! pub fn check_magic(...) { ... }
//!
//! #[verifies(crate::reqs::parser::header_magic)]
//! fn test_check_magic() { ... }
//! ```
//!
//! Both attributes accept a comma-delimited path list. Paths resolve
//! against the marker module `rubric::setup!()` emits, so name resolution
//! catches typos at `cargo check` speed and rust-analyzer autocompletes
//! requirement names without any custom LSP.

use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

use rubric_core::manifest::{Manifest, Requirement};

#[proc_macro_attribute]
pub fn satisfies(args: TokenStream, item: TokenStream) -> TokenStream {
    expand(Direction::Satisfies, args, item)
}

#[proc_macro_attribute]
pub fn verifies(args: TokenStream, item: TokenStream) -> TokenStream {
    expand(Direction::Verifies, args, item)
}

/// `rubric::setup!();` — place at crate root. Reads `rubric.toml` and
/// emits `pub mod reqs { ... }`, re-exports the two attribute macros so
/// they can be used bare (`#[satisfies(...)]` / `#[verifies(...)]`), and
/// produces a `traceability` module whose rustdoc page carries the matrix.
#[proc_macro]
pub fn setup(_args: TokenStream) -> TokenStream {
    match expand_setup() {
        Ok(ts) => ts,
        Err(msg) => compile_error(&msg),
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction { Satisfies, Verifies }

impl Direction {
    fn heading(self) -> &'static str {
        match self {
            Direction::Satisfies => "Satisfies requirements",
            Direction::Verifies => "Verifies requirements",
        }
    }
}

fn expand(dir: Direction, args: TokenStream, item: TokenStream) -> TokenStream {
    match expand_inner(dir, args, item.clone()) {
        Ok(out) => out,
        Err(msg) => {
            let mut out = item;
            out.extend(compile_error(&msg));
            out
        }
    }
}

fn expand_inner(dir: Direction, args: TokenStream, item: TokenStream) -> Result<TokenStream, String> {
    let paths = parse_paths(args)?;

    let mut const_checks = TokenStream::new();
    for (path, _) in &paths {
        const_checks.extend(const_check(path.clone()));
    }

    let item_out = match load_manifest_src() {
        None => item,
        Some(src) => {
            let manifest = Manifest::parse(&src)
                .map_err(|e| format!("rubric.toml: {}", e))?;
            let descs = resolve_descriptions(&manifest, &paths)?;
            inject_doc_attrs(item, dir, &descs)
        }
    };

    let mut out = item_out;
    out.extend(const_checks);
    Ok(out)
}

fn resolve_descriptions(
    manifest: &Manifest,
    paths: &[(TokenStream, Vec<String>)],
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (_, label_path) in paths {
        let req: &Requirement = manifest.requirements.iter()
            .find(|r| &r.label_path == label_path)
            .ok_or_else(|| format!(
                "requirement `{}` not found in rubric.toml",
                label_path.join("::"),
            ))?;
        out.push((req.label(), req.description.clone()));
    }
    Ok(out)
}

fn load_manifest_src() -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let path = std::path::Path::new(&dir).join("rubric.toml");
    std::fs::read_to_string(path).ok()
}

fn parse_paths(args: TokenStream) -> Result<Vec<(TokenStream, Vec<String>)>, String> {
    let mut groups: Vec<Vec<TokenTree>> = vec![Vec::new()];
    for tt in args {
        if matches!(&tt, TokenTree::Punct(p) if p.as_char() == ',') {
            groups.push(Vec::new());
        } else {
            groups.last_mut().unwrap().push(tt);
        }
    }
    groups.retain(|g| !g.is_empty());
    if groups.is_empty() {
        return Err("expected at least one requirement path".to_string());
    }
    let out: Vec<(TokenStream, Vec<String>)> = groups.into_iter()
        .map(|toks| {
            let idents: Vec<String> = toks.iter().filter_map(|t| match t {
                TokenTree::Ident(id) => Some(id.to_string()),
                _ => None,
            }).collect();
            let label_path: Vec<String> = if let Some(i) = idents.iter().position(|s| s == "reqs") {
                idents[i + 1..].to_vec()
            } else {
                let skip = if idents.first().map_or(false, |s| matches!(s.as_str(), "crate" | "super" | "self")) { 1 } else { 0 };
                idents[skip..].to_vec()
            };
            (toks.into_iter().collect(), label_path)
        })
        .collect();
    for (_, label) in &out {
        if label.is_empty() {
            return Err("could not extract a label path (did you mean `crate::reqs::...`?)".to_string());
        }
    }
    Ok(out)
}

fn const_check(path: TokenStream) -> TokenStream {
    let mut out = TokenStream::new();
    out.extend([
        TokenTree::Ident(Ident::new("const", Span::call_site())),
        TokenTree::Ident(Ident::new("_", Span::call_site())),
        TokenTree::Punct(Punct::new(':', Spacing::Alone)),
    ]);
    out.extend(path.clone());
    out.extend([TokenTree::Punct(Punct::new('=', Spacing::Alone))]);
    out.extend(path);
    out.extend([TokenTree::Punct(Punct::new(';', Spacing::Alone))]);
    out
}

fn compile_error(msg: &str) -> TokenStream {
    let mut out = TokenStream::new();
    out.extend([
        TokenTree::Ident(Ident::new("compile_error", Span::call_site())),
        TokenTree::Punct(Punct::new('!', Spacing::Alone)),
    ]);
    let inner: TokenStream = std::iter::once(TokenTree::Literal(Literal::string(msg))).collect();
    out.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, inner))]);
    out.extend([TokenTree::Punct(Punct::new(';', Spacing::Alone))]);
    out
}

fn inject_doc_attrs(
    item: TokenStream,
    dir: Direction,
    entries: &[(String, String)],
) -> TokenStream {
    if entries.is_empty() { return item; }
    let toks: Vec<TokenTree> = item.into_iter().collect();
    let insert_at = find_doc_attr_insert_point(&toks);
    let doc_body = render_doc_body(dir, entries);
    let doc_attr = make_doc_attr(&doc_body);
    let mut out: Vec<TokenTree> = toks[..insert_at].to_vec();
    out.extend(doc_attr);
    out.extend(toks[insert_at..].iter().cloned());
    out.into_iter().collect()
}

fn find_doc_attr_insert_point(toks: &[TokenTree]) -> usize {
    let mut last_end = 0usize;
    let mut i = 0usize;
    while i + 1 < toks.len() {
        let is_pound = matches!(&toks[i], TokenTree::Punct(p) if p.as_char() == '#');
        let is_bracket = matches!(&toks[i + 1], TokenTree::Group(g) if g.delimiter() == Delimiter::Bracket);
        if !is_pound || !is_bracket { break; }
        if let TokenTree::Group(g) = &toks[i + 1] {
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            let is_doc = matches!(inner.first(), Some(TokenTree::Ident(id)) if id.to_string() == "doc");
            if is_doc {
                last_end = i + 2;
            }
        }
        i += 2;
    }
    last_end
}

fn render_doc_body(dir: Direction, entries: &[(String, String)]) -> String {
    let mut s = String::new();
    s.push_str("\n\n# ");
    s.push_str(dir.heading());
    s.push_str("\n\n");
    for (label, desc) in entries {
        let target = format!("crate::reqs::{}", label);
        s.push_str(&format!("- [`{}`][{}] — {}\n", label, target, desc));
    }
    s
}

fn make_doc_attr(body: &str) -> Vec<TokenTree> {
    let mut inner = TokenStream::new();
    inner.extend([
        TokenTree::Ident(Ident::new("doc", Span::call_site())),
        TokenTree::Punct(Punct::new('=', Spacing::Alone)),
        TokenTree::Literal(Literal::string(body)),
    ]);
    vec![
        TokenTree::Punct(Punct::new('#', Spacing::Alone)),
        TokenTree::Group(Group::new(Delimiter::Bracket, inner)),
    ]
}

fn expand_setup() -> Result<TokenStream, String> {
    let manifest_src = load_manifest_src()
        .ok_or_else(|| "rubric.toml not found in CARGO_MANIFEST_DIR — run `cargo rubric init` to scaffold one".to_string())?;
    let manifest = Manifest::parse(&manifest_src)
        .map_err(|e| format!("rubric.toml: {}", e))?;
    let marker = rubric_core::codegen::render(&manifest);

    let mut out = String::new();
    out.push_str(&marker);
    // Re-export the two attribute macros so consumers write
    // `#[satisfies(...)]` / `#[verifies(...)]` without prefixing.
    out.push_str("\n#[doc(inline)]\npub use rubric::satisfies;\n");
    out.push_str("\n#[doc(inline)]\npub use rubric::verifies;\n");

    if std::env::var("OUT_DIR").is_ok() {
        out.push_str("\npub mod traceability {\n    #![doc = include_str!(concat!(env!(\"OUT_DIR\"), \"/rubric_matrix.md\"))]\n}\n");
    } else {
        let inline = render_inline_matrix(&manifest);
        out.push_str("\n#[doc = r###\"");
        out.push_str(&inline);
        out.push_str("\"###]\npub mod traceability {}\n");
    }

    out.push_str("\n#[allow(dead_code)]\nconst _RUBRIC_TOML_TRACK: &[u8] = include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/rubric.toml\"));\n");

    out.parse::<TokenStream>()
        .map_err(|e| format!("setup!() expansion failed to parse: {}", e))
}

fn render_inline_matrix(manifest: &Manifest) -> String {
    let mut s = String::from("# Traceability matrix\n\n");
    s.push_str("| ID | Requirement | Description | Satisfied by (declared) | Verified by (declared) |\n");
    s.push_str("|----|-------------|-------------|-------------------------|------------------------|\n");
    for req in &manifest.requirements {
        let id = req.id();
        let label = req.label();
        let desc = req.description.replace('|', "\\|").replace('\n', " ");
        let sat = if req.satisfied_by.is_empty() { "—".to_string() } else {
            req.satisfied_by.iter().map(|p| format!("`{}`", p)).collect::<Vec<_>>().join("<br>")
        };
        let ver = if req.verified_by.is_empty() { "—".to_string() } else {
            req.verified_by.iter().map(|p| format!("`{}`", p)).collect::<Vec<_>>().join("<br>")
        };
        s.push_str(&format!(
            "| `{}` | [`{}`][crate::reqs::{}] | {} | {} | {} |\n",
            id, label, label, desc, sat, ver,
        ));
    }
    s
}
