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

/// Commits that touched `rubric.lock` (newest first), the parsed seal state
/// at each, and the boundary state just before the listed range. With
/// `base` set, only commits in `base..HEAD` are walked and the boundary is
/// the lock at `base`; otherwise the whole history is walked and the
/// boundary is empty. A version that does not parse reads as empty.
pub fn lock_history(
    root: &Path,
    base: Option<&str>,
) -> Result<(Vec<Commit>, Vec<SealMap>, SealMap), String> {
    // Repo-root-relative path to the lockfile (`git show` needs it).
    let prefix = git(root, &["rev-parse", "--show-prefix"])
        .map_err(|_| "not a git repository (or no commits yet)".to_string())?;
    let rel = format!("{}rubric.lock", prefix.trim());

    let range = base.map(|b| format!("{b}..HEAD"));
    let mut args = vec!["log", "--format=%H%x1f%an%x1f%ad", "--date=short"];
    if let Some(r) = &range {
        args.push(r.as_str());
    }
    args.extend(["--", "rubric.lock"]);
    let raw = git(root, &args)?;
    let commits: Vec<Commit> = raw.lines().filter_map(parse_line).collect();

    let versions = commits
        .iter()
        .map(|c| match git(root, &["show", &format!("{}:{}", c.sha, rel)]) {
            Ok(src) => seal_map(&src),
            Err(_) => SealMap::new(),
        })
        .collect();

    // State just before the range: the lock at `base`. Empty for full
    // history, or when the lockfile predates `base`.
    let boundary = match base {
        Some(b) => git(root, &["show", &format!("{b}:{rel}")]).map(|s| seal_map(&s)).unwrap_or_default(),
        None => SealMap::new(),
    };
    Ok((commits, versions, boundary))
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
