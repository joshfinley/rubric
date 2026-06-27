//! Workspace member enumeration and the multi-crate driver.
//!
//! When run from a workspace root, each verb operates on every member
//! that has a `rubric.toml`. The `[workspace]` table is read with a small
//! tolerant scanner (a full Cargo.toml has inline tables and arrays of
//! tables that the stdlib-only `toml_lite` doesn't model; here we only
//! need `members` and `exclude`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Crate roots to process: workspace members that carry a `rubric.toml`
/// (plus the workspace root itself, if it carries one), or just `cwd` when
/// this is not a workspace. The root is honored whether the root manifest
/// is a package or a virtual `[workspace]`; its satisfiers are typically
/// `external:` citations to integration tests, since the root is the
/// natural home for system-level requirements.
pub fn targets(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    let text = match std::fs::read_to_string(cwd.join("Cargo.toml")) {
        Ok(t) => t,
        // No Cargo.toml: treat cwd as the crate. The verb reports a
        // missing rubric.toml if there isn't one.
        Err(_) => return Ok(vec![cwd.to_path_buf()]),
    };

    let cargo = parse_cargo(&text);
    if !cargo.is_workspace {
        return Ok(vec![cwd.to_path_buf()]);
    }

    let mut dirs = Vec::new();
    if cwd.join("rubric.toml").exists() {
        dirs.push(cwd.to_path_buf());
    }
    let excluded = expand_all(cwd, &cargo.exclude);
    for member in expand_all(cwd, &cargo.members) {
        if excluded.contains(&member) {
            continue;
        }
        if member.join("rubric.toml").exists() {
            dirs.push(member);
        }
    }
    dirs.sort();
    dirs.dedup();

    if dirs.is_empty() {
        return Err("workspace has no member with a rubric.toml (run `cargo rubric init` in a member)".into());
    }
    Ok(dirs)
}

/// Run `per_crate` over every target, aggregating exit status. When more
/// than one crate is processed, the relative path is passed as a label so
/// the verb can print a header in its own format. Returns failure if any
/// crate fails.
pub fn drive<F>(per_crate: F) -> ExitCode
where
    F: Fn(&Path, Option<&str>) -> Result<bool, String>,
{
    let cwd = Path::new(".");
    let targets = match targets(cwd) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cargo rubric: {e}");
            return ExitCode::FAILURE;
        }
    };

    let multi = targets.len() > 1;
    let mut ok = true;
    for (i, target) in targets.iter().enumerate() {
        let label = if multi { Some(rel_label(cwd, target)) } else { None };
        if multi && i > 0 {
            println!();
        }
        match per_crate(target, label.as_deref()) {
            Ok(success) => ok &= success,
            Err(e) => {
                eprintln!("cargo rubric: {}: {e}", label.unwrap_or_else(|| ".".into()));
                ok = false;
            }
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn rel_label(cwd: &Path, target: &Path) -> String {
    target
        .strip_prefix(cwd)
        .unwrap_or(target)
        .display()
        .to_string()
}

#[derive(Default)]
struct Cargo {
    is_workspace: bool,
    members: Vec<String>,
    exclude: Vec<String>,
}

enum Arr {
    Members,
    Exclude,
}

fn parse_cargo(text: &str) -> Cargo {
    let mut c = Cargo::default();
    let mut section = String::new();
    let mut collecting: Option<Arr> = None;
    let mut depth = 0i32;

    for raw in text.lines() {
        let t = strip_line_comment(raw).trim();

        if let Some(arr) = &collecting {
            push_quoted(t, arr, &mut c);
            depth += brackets(t);
            if depth <= 0 {
                collecting = None;
            }
            continue;
        }

        if t.starts_with('[') {
            section = t.trim_start_matches('[').trim_end_matches(']').trim().to_string();
            if section == "workspace" {
                c.is_workspace = true;
            }
            continue;
        }

        if section == "workspace" {
            let key = t.split('=').next().map(str::trim).unwrap_or("");
            let arr = match key {
                "members" => Some(Arr::Members),
                "exclude" => Some(Arr::Exclude),
                _ => None,
            };
            if let Some(arr) = arr {
                let value = t.splitn(2, '=').nth(1).unwrap_or("");
                push_quoted(value, &arr, &mut c);
                depth = brackets(value);
                if depth > 0 {
                    collecting = Some(arr);
                }
            }
        }
    }
    c
}

fn push_quoted(s: &str, arr: &Arr, c: &mut Cargo) {
    for q in quoted_strings(s) {
        match arr {
            Arr::Members => c.members.push(q),
            Arr::Exclude => c.exclude.push(q),
        }
    }
}

fn brackets(s: &str) -> i32 {
    s.chars().map(|ch| match ch {
        '[' => 1,
        ']' => -1,
        _ => 0,
    }).sum()
}

/// Comment stripper that respects double-quoted strings.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quote = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b'#' if !in_quote => return &line[..i],
            _ => {}
        }
    }
    line
}

fn quoted_strings(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            if j <= bytes.len() {
                out.push(s[start..j].to_string());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn expand_all(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in patterns {
        if p.contains('*') {
            glob(root, p, &mut out);
        } else {
            out.push(root.join(p));
        }
    }
    out
}

fn glob(root: &Path, pattern: &str, out: &mut Vec<PathBuf>) {
    let comps: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    walk_glob(root, &comps, out);
}

fn walk_glob(cur: &Path, comps: &[&str], out: &mut Vec<PathBuf>) {
    match comps.split_first() {
        None => {
            if cur.is_dir() {
                out.push(cur.to_path_buf());
            }
        }
        Some((&"**", rest)) => {
            walk_glob(cur, rest, out); // ** matches zero directories
            for sub in dirs_in(cur) {
                walk_glob(&sub, comps, out); // …or one or more
            }
        }
        Some((head, rest)) if head.contains('*') => {
            for sub in dirs_in(cur) {
                if let Some(name) = sub.file_name().and_then(|n| n.to_str()) {
                    if wildcard(head, name) {
                        walk_glob(&sub, rest, out);
                    }
                }
            }
        }
        Some((head, rest)) => {
            let next = cur.join(head);
            if next.exists() {
                walk_glob(&next, rest, out);
            }
        }
    }
}

fn dirs_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// Classic `*`/`?` glob match over a single path component.
fn wildcard(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = name.chars().collect();
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_explicit_members() {
        let c = parse_cargo("[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n");
        assert!(c.is_workspace);
        assert_eq!(c.members, vec!["crates/a", "crates/b"]);
    }

    #[test]
    fn multiline_members_with_comments() {
        let c = parse_cargo(
            "[workspace]\nmembers = [\n  \"crates/a\",  # the core\n  \"crates/b\",\n]\nexclude = [\"crates/x\"]\n",
        );
        assert_eq!(c.members, vec!["crates/a", "crates/b"]);
        assert_eq!(c.exclude, vec!["crates/x"]);
    }

    #[test]
    fn package_without_workspace_is_not_a_workspace() {
        let c = parse_cargo("[package]\nname = \"foo\"\n");
        assert!(!c.is_workspace);
    }

    #[test]
    fn workspace_keys_outside_section_are_ignored() {
        // A `members =` under [package.metadata] must not be read.
        let c = parse_cargo("[package.metadata]\nmembers = [\"nope\"]\n");
        assert!(!c.is_workspace);
        assert!(c.members.is_empty());
    }

    #[test]
    fn wildcard_matches() {
        assert!(wildcard("*", "anything"));
        assert!(wildcard("crate-*", "crate-core"));
        assert!(!wildcard("crate-*", "other"));
        assert!(wildcard("a?c", "abc"));
        assert!(!wildcard("a?c", "ac"));
    }

    /// A unique, empty scratch directory for a `targets()` test.
    fn tmp_workspace(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "cargo-rubric-test-{}-{tag}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn virtual_workspace_root_with_rubric_is_a_target() {
        // A pure `[workspace]` root (no `[package]`) that carries a
        // rubric.toml is now honored alongside its members.
        let dir = tmp_workspace("virtual-root");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"a\"]\n").unwrap();
        std::fs::write(dir.join("rubric.toml"), "").unwrap();
        let member = dir.join("a");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(member.join("rubric.toml"), "").unwrap();

        let got = targets(&dir).unwrap();
        assert!(got.contains(&dir), "virtual root should be a target: {got:?}");
        assert!(got.contains(&member), "member should be a target: {got:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workspace_root_without_rubric_is_not_a_target() {
        // No root rubric.toml: only members carry the contract.
        let dir = tmp_workspace("no-root-rubric");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"a\"]\n").unwrap();
        let member = dir.join("a");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(member.join("rubric.toml"), "").unwrap();

        let got = targets(&dir).unwrap();
        assert!(!got.contains(&dir), "root without rubric.toml must not be a target: {got:?}");
        assert_eq!(got, vec![member]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
