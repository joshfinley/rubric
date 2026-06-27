//! Per-crate source discovery: walk a crate's `src/` and `tests/` trees
//! and derive each file's module path, so the scanner can produce stable
//! item paths.
//!
//! `src/` is rooted at `crate`, the standard Rust module layout:
//!
//! - `src/lib.rs`, `src/main.rs` → `["crate"]`
//! - `src/foo.rs`                → `["crate", "foo"]`
//! - `src/foo/mod.rs`            → `["crate", "foo"]`
//! - `src/foo/bar.rs`            → `["crate", "foo", "bar"]`
//!
//! `tests/` is walked recursively and rooted at `tests`, so every test
//! file (including nested ones and shared `mod` helpers) is reachable
//! and distinct from inline unit tests (`crate::…::tests::t`):
//!
//! - `tests/api.rs` fn `t`       → `tests::api::t`
//! - `tests/api/cases.rs` fn `t` → `tests::api::cases::t`
//! - `tests/common/mod.rs` fn `h`→ `tests::common::h`

use std::path::{Path, PathBuf};

use crate::scan::FileInput;

/// File stems that name their enclosing module rather than adding a segment.
const ROOT_STEMS: [&str; 3] = ["lib", "main", "mod"];

/// Read the crate's `src/` and `tests/` trees into `FileInput`s, sorted by
/// path so the result is independent of directory-walk order.
pub fn discover(root: &Path) -> std::io::Result<Vec<FileInput>> {
    let mut out = Vec::new();
    walk_tree(&root.join("src"), "crate", &mut out)?;
    walk_tree(&root.join("tests"), "tests", &mut out)?;
    Ok(out)
}

fn walk_tree(dir: &Path, root_seg: &str, out: &mut Vec<FileInput>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut paths = Vec::new();
    collect_rs(dir, &mut paths)?;
    paths.sort();
    for path in paths {
        let rel = path.strip_prefix(dir).unwrap_or(&path);
        let module_path = module_path_for(rel, root_seg);
        let source = std::fs::read_to_string(&path)?;
        out.push(FileInput { module_path, source });
    }
    Ok(())
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out)?;
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

/// Module path for a file relative to its tree root (`crate` or `tests`).
fn module_path_for(rel: &Path, root_seg: &str) -> Vec<String> {
    let mut segments = vec![root_seg.to_string()];
    let parts: Vec<String> = rel.iter().map(|c| c.to_string_lossy().to_string()).collect();

    for (idx, part) in parts.iter().enumerate() {
        let last = idx == parts.len() - 1;
        if last {
            let stem = part.strip_suffix(".rs").unwrap_or(part);
            if ROOT_STEMS.contains(&stem) {
                continue;
            }
            segments.push(stem.to_string());
        } else {
            segments.push(part.clone());
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(p: &str, root: &str) -> Vec<String> {
        module_path_for(Path::new(p), root)
    }

    #[test]
    fn crate_roots_have_no_extra_segment() {
        assert_eq!(mp("lib.rs", "crate"), vec!["crate"]);
        assert_eq!(mp("main.rs", "crate"), vec!["crate"]);
    }

    #[test]
    fn src_module_files() {
        assert_eq!(mp("voter.rs", "crate"), vec!["crate", "voter"]);
        assert_eq!(mp("voter/mod.rs", "crate"), vec!["crate", "voter"]);
        assert_eq!(mp("voter/tmr.rs", "crate"), vec!["crate", "voter", "tmr"]);
    }

    #[test]
    fn tests_rooting_including_nested() {
        assert_eq!(mp("api.rs", "tests"), vec!["tests", "api"]);
        assert_eq!(mp("api/cases.rs", "tests"), vec!["tests", "api", "cases"]);
        assert_eq!(mp("common/mod.rs", "tests"), vec!["tests", "common"]);
    }
}
