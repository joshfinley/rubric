//! Workspace discovery shared across subcommands.
//!
//! At a workspace root (a `Cargo.toml` with `[workspace]` and no
//! `[package]`) we iterate every member that owns a `rubric.toml`.
//! Anywhere else we operate on the single crate.

use std::path::{Path, PathBuf};

/// Return the set of crate roots a subcommand should operate on.
/// - Single crate (has `rubric.toml`) → that crate.
/// - Workspace root → every member with a `rubric.toml`.
/// - Otherwise → empty (caller should error).
pub fn resolve_targets(cwd: &Path) -> Vec<PathBuf> {
    // Explicit single-crate: any dir with rubric.toml wins, even if its
    // Cargo.toml also declares [workspace] (virtual + real hybrids).
    if cwd.join("rubric.toml").is_file() {
        return vec![cwd.to_path_buf()];
    }
    // Workspace root: scan members.
    let cargo = match std::fs::read_to_string(cwd.join("Cargo.toml")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let is_workspace = cargo.lines().any(|l| l.trim() == "[workspace]");
    if !is_workspace {
        return Vec::new();
    }
    discover_members(cwd, &cargo)
        .into_iter()
        .filter(|m| m.join("rubric.toml").is_file())
        .collect()
}

/// Resolve the list of `rubric.toml` paths a subcommand should operate on.
/// - `--manifest-path <p>` → `[p]` (explicit, always single).
/// - Otherwise → `resolve_targets(cwd)` manifests, or the single path
///   discovered by walking up for a `rubric.toml` as a legacy fallback.
pub fn manifest_targets(explicit: Option<&str>, cwd: &Path) -> Vec<PathBuf> {
    if let Some(p) = explicit {
        return vec![PathBuf::from(p)];
    }
    let roots = resolve_targets(cwd);
    if !roots.is_empty() {
        return roots.into_iter().map(|r| r.join("rubric.toml")).collect();
    }
    // Legacy: walk up from cwd.
    let mut cur: Option<&Path> = Some(cwd);
    while let Some(dir) = cur {
        let p = dir.join("rubric.toml");
        if p.is_file() { return vec![p]; }
        cur = dir.parent();
    }
    Vec::new()
}

/// Parse `[workspace.members]` with minimal glob expansion (trailing `*`).
pub fn discover_members(workspace_root: &Path, cargo_toml: &str) -> Vec<PathBuf> {
    let mut raw: Vec<String> = Vec::new();
    let mut in_workspace = false;
    let mut in_members = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_workspace = t == "[workspace]";
            in_members = false;
            continue;
        }
        if !in_workspace { continue; }
        if let Some(rest) = t.strip_prefix("members") {
            let after_eq = rest.trim_start_matches([' ', '\t', '=']);
            if after_eq.starts_with('[') { in_members = true; }
            for s in after_eq.split(&['[', ']', ',']) {
                let s = s.trim().trim_matches('"');
                if !s.is_empty() { raw.push(s.to_string()); }
            }
            if t.ends_with(']') { in_members = false; }
        } else if in_members {
            for s in t.split(&['[', ']', ',']) {
                let s = s.trim().trim_matches('"');
                if !s.is_empty() { raw.push(s.to_string()); }
            }
            if t.ends_with(']') { in_members = false; }
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in raw {
        if let Some(prefix) = entry.strip_suffix("/*") {
            let dir = workspace_root.join(prefix);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() && p.join("Cargo.toml").is_file() { out.push(p); }
                }
            }
        } else {
            let p = workspace_root.join(&entry);
            if p.join("Cargo.toml").is_file() { out.push(p); }
        }
    }
    out.sort();
    out.dedup();
    out
}
