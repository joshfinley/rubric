//! Source walker — finds `#[bind(...)]` and `bind_at!(...)` annotations.
//!
//! Uses `rustc_lexer` to tokenize, then pattern-matches the token stream.
//! No full Rust parser: we only need to recognise two well-defined token
//! shapes, and rustc_lexer is the same lexer rustc uses, so we agree with
//! rustc on what counts as an identifier, comment, etc.

use std::path::{Path, PathBuf};

use crate::manifest::Direction;
use crate::resolver::AnnotationSite;
use rustc_lexer::{tokenize, TokenKind};

pub use crate::resolver::AnnotationSite as Annotation;

#[derive(Debug)]
pub struct WalkError {
    pub file: PathBuf,
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for WalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.file.display(), self.line, self.msg)
    }
}

/// Walk every `.rs` file under `root`, returning all annotations.
pub fn walk_dir(root: &Path) -> Result<Vec<AnnotationSite>, WalkError> {
    let mut out = Vec::new();
    walk_inner(root, &mut out)?;
    Ok(out)
}

fn walk_inner(dir: &Path, out: &mut Vec<AnnotationSite>) -> Result<(), WalkError> {
    let entries = std::fs::read_dir(dir).map_err(|e| WalkError {
        file: dir.to_path_buf(),
        line: 0,
        msg: format!("read_dir: {}", e),
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        // Skip target/, hidden dirs, generated marker, vendor caches.
        if name.starts_with('.') || name == "target" { continue; }
        if path.is_dir() {
            walk_inner(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            // Skip generated marker file — it has no annotations and we
            // don't want to double-count anything inside it.
            if name == "__bind.rs" { continue; }
            let text = std::fs::read_to_string(&path).map_err(|e| WalkError {
                file: path.clone(), line: 0, msg: format!("read: {}", e),
            })?;
            scan_file(&path, &text, out)?;
        }
    }
    Ok(())
}

// satisfies: walker::find_attribute_form, walker::ignore_strings_and_comments, walker::multi_path_annotations, walker::comment_form_annotations
pub fn scan_file(path: &Path, text: &str, out: &mut Vec<AnnotationSite>) -> Result<(), WalkError> {
    // Comment-form annotations: `// satisfies: a, b` / `// verifies: c`.
    // Used by crates that can't consume the proc-macros (cycles, FFI, etc).
    scan_comment_form(path, text, out)?;

    let toks = collect_tokens(text);
    let mut i = 0;
    while i < toks.len() {
        // Attribute form: `#` `[` <path-ending-in-`satisfies`|`verifies`> `(` ... `)` `]`
        if matches!(toks[i].kind, TokenKind::Pound)
            && i + 1 < toks.len()
            && matches!(toks[i + 1].kind, TokenKind::OpenBracket)
        {
            if let Some((after_path, last)) = read_path(&toks, i + 2) {
                let dir = match last.as_str() {
                    "satisfies" => Some(Direction::Satisfies),
                    "verifies" => Some(Direction::Verifies),
                    _ => None,
                };
                if let Some(dir) = dir {
                    if after_path < toks.len()
                        && matches!(toks[after_path].kind, TokenKind::OpenParen)
                    {
                        let (rparen, anns) = parse_args(path, &toks, after_path, i, dir)?;
                        out.extend(anns);
                        i = rparen + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    Ok(())
}

/// Scan for `// bind: <direction> = <path>, <path>, …` annotations by
/// iterating rustc_lexer tokens so only real comments (not substrings
/// that happen to look like them inside string literals) are considered.
fn scan_comment_form(file: &Path, text: &str, out: &mut Vec<AnnotationSite>) -> Result<(), WalkError> {
    let mut pos = 0usize;
    let mut line = 1usize;
    for tok in tokenize(text) {
        let start = pos;
        let end = pos + tok.len;
        if matches!(tok.kind, TokenKind::LineComment) {
            let slice = &text[start..end];
            // strip leading slashes, handle `//` and `///` / `//!`.
            let body = slice.trim_start_matches('/').trim_start_matches('!');
            // `// bind:` form requires whitespace after the colon to
            // avoid `// bind::bind is a macro` collisions.
            // Recognise `// satisfies: a, b` / `// verifies: c, d` — the
            // direction is the keyword, followed by `:` and a comma list.
            let parsed = parse_direction_pragma(body);
            if let Some((dir, paths_str)) = parsed {
                match parse_comment_paths(paths_str) {
                    Ok(label_paths) => {
                        for label in label_paths {
                            out.push(AnnotationSite {
                                file: file.to_path_buf(),
                                line,
                                byte_offset: start,
                                direction: dir,
                                label_path: label,
                                explicit_id: None,
                            });
                        }
                    }
                    Err(msg) => return Err(WalkError { file: file.to_path_buf(), line, msg }),
                }
            }
        }
        line += text[start..end].bytes().filter(|&b| b == b'\n').count();
        pos = end;
    }
    Ok(())
}

/// Parse `<dir>: <paths…>` where dir is `satisfies` or `verifies`. The
/// body here is already the post-`///`-strip content of a line comment.
/// Whitespace before `:` is required so `satisfies::foo` (a Rust path
/// accidentally at line-start in a block comment) does not match.
fn parse_direction_pragma(body: &str) -> Option<(Direction, &str)> {
    let body = body.trim_start();
    for (keyword, dir) in &[("satisfies", Direction::Satisfies), ("verifies", Direction::Verifies)] {
        if let Some(rest) = body.strip_prefix(keyword) {
            if let Some(after_ws) = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('\t')) {
                if let Some(after_colon) = after_ws.trim_start().strip_prefix(':') {
                    return Some((*dir, after_colon.trim()));
                }
            }
            if let Some(after_colon) = rest.strip_prefix(':') {
                return Some((*dir, after_colon.trim()));
            }
        }
    }
    None
}

fn parse_comment_paths(paths_str: &str) -> Result<Vec<Vec<String>>, String> {
    let mut out = Vec::new();
    for part in paths_str.split(',') {
        let part = part.trim().trim_end_matches(';');
        if part.is_empty() { continue; }
        let segments: Vec<String> = part.split("::").map(|s| s.trim().to_string()).collect();
        let label = strip_marker_prefix_segments(&segments);
        if label.is_empty() {
            return Err(format!("empty label in comment annotation: `{}`", part));
        }
        out.push(label);
    }
    if out.is_empty() {
        return Err("no paths after `:` in comment-form annotation".to_string());
    }
    Ok(out)
}

/// Mirror of the attribute walker's label extraction: drop everything up
/// to and including a `reqs` segment; strip leading `crate`/`super`/`self`.
fn strip_marker_prefix_segments(segs: &[String]) -> Vec<String> {
    let skip = if segs.first().map(|s| matches!(s.as_str(), "crate" | "super" | "self")).unwrap_or(false) { 1 } else { 0 };
    let rest = &segs[skip..];
    if let Some(i) = rest.iter().position(|s| s == "reqs") {
        rest[i + 1..].to_vec()
    } else {
        rest.to_vec()
    }
}

/// Read a `Ident (:: Ident)*` sequence starting at `start`. Returns the
/// index just past the path and the text of the last segment, or None if
/// the position doesn't start with an identifier.
fn read_path(toks: &[Tok], start: usize) -> Option<(usize, String)> {
    if start >= toks.len() || !matches!(toks[start].kind, TokenKind::Ident) {
        return None;
    }
    let mut i = start + 1;
    let mut last = toks[start].text.clone();
    loop {
        if i + 2 < toks.len()
            && matches!(toks[i].kind, TokenKind::Colon)
            && matches!(toks[i + 1].kind, TokenKind::Colon)
            && matches!(toks[i + 2].kind, TokenKind::Ident)
        {
            last = toks[i + 2].text.clone();
            i += 3;
        } else {
            break;
        }
    }
    Some((i, last))
}

#[derive(Debug)]
struct Tok {
    kind: TokenKind,
    text: String,
    line: usize,
    byte_start: usize,
}

fn collect_tokens(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut line = 1usize;
    for tok in tokenize(text) {
        let end = pos + tok.len;
        let slice = &text[pos..end];
        let advance_lines = slice.bytes().filter(|&b| b == b'\n').count();
        let keep = !matches!(
            tok.kind,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment { .. }
        );
        if keep {
            out.push(Tok { kind: tok.kind, text: slice.to_string(), line, byte_start: pos });
        }
        pos = end;
        line += advance_lines;
    }
    out
}

fn parse_args(path: &Path, toks: &[Tok], lparen: usize, annotation_start: usize, dir: Direction) -> Result<(usize, Vec<AnnotationSite>), WalkError> {
    // Find matching `)`
    let mut depth = 1;
    let mut rparen = lparen;
    for j in (lparen + 1)..toks.len() {
        match toks[j].kind {
            TokenKind::OpenParen => depth += 1,
            TokenKind::CloseParen => {
                depth -= 1;
                if depth == 0 { rparen = j; break; }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(WalkError {
            file: path.to_path_buf(),
            line: toks[lparen].line,
            msg: "unmatched '(' in attribute".to_string(),
        });
    }
    let inner = &toks[lparen + 1..rparen];
    if inner.is_empty() {
        return Err(WalkError {
            file: path.to_path_buf(),
            line: toks[lparen].line,
            msg: "empty attribute args".to_string(),
        });
    }
    // The direction is now the attribute name; inner is a comma-separated
    // list of paths.
    let tail = inner;
    let mut groups: Vec<Vec<&Tok>> = vec![Vec::new()];
    for t in tail {
        if matches!(t.kind, TokenKind::Comma) {
            groups.push(Vec::new());
        } else {
            groups.last_mut().unwrap().push(t);
        }
    }
    let mut out = Vec::new();
    for group in &groups {
        if group.is_empty() { continue; }
        let mut segments: Vec<String> = Vec::new();
        let mut k = 0;
        while k < group.len() {
            match group[k].kind {
                TokenKind::Ident => {
                    segments.push(group[k].text.clone());
                    k += 1;
                    if k + 1 < group.len()
                        && matches!(group[k].kind, TokenKind::Colon)
                        && matches!(group[k + 1].kind, TokenKind::Colon)
                    {
                        k += 2;
                    } else {
                        break;
                    }
                }
                _ => return Err(WalkError {
                    file: path.to_path_buf(),
                    line: group[k].line,
                    msg: format!("unexpected token `{}` in path", group[k].text),
                }),
            }
        }
        let label_path = strip_marker_prefix(&segments);
        if label_path.is_empty() {
            return Err(WalkError {
                file: path.to_path_buf(),
                line: toks[lparen].line,
                msg: "could not extract a label path".to_string(),
            });
        }
        out.push(AnnotationSite {
            direction: dir,
            label_path,
            file: path.to_path_buf(),
            line: toks[lparen].line,
            byte_offset: toks[annotation_start].byte_start,
            explicit_id: None,
        });
    }
    if out.is_empty() {
        return Err(WalkError {
            file: path.to_path_buf(),
            line: toks[lparen].line,
            msg: "no paths in attribute".to_string(),
        });
    }
    Ok((rparen, out))
}

/// Strip everything up to and including a `reqs` segment. If `reqs` doesn't
/// appear (the user used `use` to import the marker), assume the whole path
/// is the label path. Always strips a leading `crate`/`super`/`self`.
fn strip_marker_prefix(segments: &[String]) -> Vec<String> {
    let mut start = 0;
    if let Some(first) = segments.first() {
        if matches!(first.as_str(), "crate" | "super" | "self") { start = 1; }
    }
    let rest = &segments[start..];
    if let Some(idx) = rest.iter().position(|s| s == "reqs") {
        rest[idx + 1..].to_vec()
    } else {
        rest.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[rubric::verifies(crate::reqs::walker::find_attribute_form)]
    fn finds_attribute_form() {
        let src = r#"
#[satisfies(crate::reqs::parser::header_magic)]
fn x() {}
"#;
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].direction, Direction::Satisfies));
        assert_eq!(out[0].label_path, vec!["parser", "header_magic"]);
    }

    #[test]
    #[rubric::verifies(crate::reqs::walker::ignore_strings_and_comments)]
    fn ignores_lookalike_inside_strings_and_comments() {
        let src = "\n\
// attribute-form in a comment does not count\n\
const S: &str = \"#[satisfies(fake::path)]\";\n";
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert!(out.is_empty(), "got: {:?}", out);
    }

    #[test]
    #[rubric::verifies(crate::reqs::walker::multi_path_annotations)]
    fn multi_path_in_one_attribute() {
        let src = r#"
#[satisfies(crate::reqs::a, crate::reqs::b, crate::reqs::c)]
fn x() {}
"#;
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].label_path, vec!["a"]);
        assert_eq!(out[1].label_path, vec!["b"]);
        assert_eq!(out[2].label_path, vec!["c"]);
    }

    #[test]
    fn handles_multiple_in_one_file() {
        let src = r#"
#[satisfies(crate::reqs::a)]
fn one() {}

#[verifies(crate::reqs::b)]
fn two() {}
"#;
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    #[rubric::verifies(crate::reqs::walker::comment_form_annotations)]
    fn finds_comment_form() {
        let src = "\
// satisfies: crate::reqs::a::b
fn impl_fn() {}

// verifies: crate::reqs::a::b, crate::reqs::c::d
fn test_fn() {}
";
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].label_path, vec!["a", "b"]);
        assert!(matches!(out[0].direction, Direction::Satisfies));
        assert_eq!(out[1].label_path, vec!["a", "b"]);
        assert!(matches!(out[1].direction, Direction::Verifies));
        assert_eq!(out[2].label_path, vec!["c", "d"]);
    }

    #[test]
    fn comment_form_ignores_non_pragma_comments() {
        let src = "\
// this is a normal comment
// TODO: something
// satisfies_is_not_a_prefix_match
fn x() {}
";
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert!(out.is_empty(), "got: {:?}", out);
    }

    #[test]
    fn reports_line_numbers() {
        let src = "\n\n#[satisfies(crate::reqs::a)]\nfn x() {}\n";
        let mut out = Vec::new();
        scan_file(Path::new("test.rs"), src, &mut out).unwrap();
        assert_eq!(out[0].line, 3);
    }
}
