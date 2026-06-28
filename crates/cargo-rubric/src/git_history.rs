//! Shared git plumbing for reading the seal history of `rubric.lock`.
//!
//! Rubric keeps no history file of its own. `log` and `audit` walk the
//! commits that touched `rubric.lock` and parse each version's seals.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use rubric_trace::lock::{self, Seal};

pub struct Commit {
    pub sha: String,
    pub author: String,
    pub date: String,
}

/// Seal state of one `rubric.lock` version, keyed by `(req_label, item_path)`.
pub type SealMap = BTreeMap<(String, String), Seal>;

/// Commits that touched `rubric.lock` (newest first) and the parsed seal
/// state at each. A version that does not parse reads as empty.
pub fn lock_history(root: &Path) -> Result<(Vec<Commit>, Vec<SealMap>), String> {
    // Repo-root-relative path to the lockfile (`git show` needs it).
    let prefix = git(root, &["rev-parse", "--show-prefix"])
        .map_err(|_| "not a git repository (or no commits yet)".to_string())?;
    let rel = format!("{}rubric.lock", prefix.trim());

    let raw = git(
        root,
        &["log", "--format=%H%x1f%an%x1f%ad", "--date=short", "--", "rubric.lock"],
    )?;
    let commits: Vec<Commit> = raw.lines().filter_map(parse_line).collect();

    let versions = commits
        .iter()
        .map(|c| match git(root, &["show", &format!("{}:{}", c.sha, rel)]) {
            Ok(src) => seal_map(&src),
            Err(_) => SealMap::new(),
        })
        .collect();
    Ok((commits, versions))
}

fn parse_line(line: &str) -> Option<Commit> {
    let mut f = line.split('\u{1f}');
    Some(Commit {
        sha: f.next()?.to_string(),
        author: f.next()?.to_string(),
        date: f.next()?.to_string(),
    })
}

pub fn seal_map(src: &str) -> SealMap {
    match lock::parse(src) {
        Ok(l) => l
            .entries
            .into_iter()
            .map(|e| ((e.key.req_label, e.key.item_path), e.seal))
            .collect(),
        Err(_) => SealMap::new(),
    }
}

pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
