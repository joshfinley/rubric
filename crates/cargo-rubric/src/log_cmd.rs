//! `cargo rubric log`: the seal-event timeline from the git history of
//! `rubric.lock`.
//!
//! Rubric keeps no history file of its own. This walks the commits that
//! touched `rubric.lock`, parses each version, and reports every
//! `(requirement, item)` seal that was introduced, re-sealed, or removed,
//! newest first. The actual source diff is one `git show` away.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, ExitCode};

use rubric_trace::check::STATEMENT_MARKER;
use rubric_trace::lock::{self, Seal};

use crate::members;

pub fn run() -> ExitCode {
    members::drive(log_one)
}

struct Commit {
    sha: String,
    author: String,
    date: String,
}

fn log_one(root: &Path, label: Option<&str>) -> Result<bool, String> {
    if let Some(l) = label {
        println!("== {l} ==");
    }
    try_run(root).map(|()| true)
}

fn try_run(root: &Path) -> Result<(), String> {
    // Repo-root-relative path to the lockfile (git show needs it).
    let prefix = git(root, &["rev-parse", "--show-prefix"])
        .map_err(|_| "not a git repository (or no commits yet)".to_string())?;
    let rel = format!("{}rubric.lock", prefix.trim());

    let raw = git(
        root,
        &["log", "--format=%H%x1f%an%x1f%ad", "--date=short", "--", "rubric.lock"],
    )?;
    let commits: Vec<Commit> = raw
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\u{1f}');
            Some(Commit {
                sha: f.next()?.to_string(),
                author: f.next()?.to_string(),
                date: f.next()?.to_string(),
            })
        })
        .collect();

    if commits.is_empty() {
        println!("no committed history for rubric.lock");
        return Ok(());
    }

    // Seal map per commit (empty if that version doesn't parse).
    let versions: Vec<SealMap> = commits
        .iter()
        .map(|c| {
            let spec = format!("{}:{}", c.sha, rel);
            match git(root, &["show", &spec]) {
                Ok(src) => seal_map(&src),
                Err(_) => SealMap::new(),
            }
        })
        .collect();

    let mut printed_any = false;
    for (i, commit) in commits.iter().enumerate() {
        let new = &versions[i];
        let empty = SealMap::new();
        let old = versions.get(i + 1).unwrap_or(&empty);
        let events = diff(old, new);
        if events.is_empty() {
            continue;
        }
        printed_any = true;
        let short = &commit.sha[..commit.sha.len().min(9)];
        println!("commit {short}  {}  {}", commit.date, commit.author);
        for e in events {
            println!("    {}", e.render());
        }
        println!();
    }

    if !printed_any {
        println!("no seal changes recorded in rubric.lock history");
    }
    Ok(())
}

type SealMap = BTreeMap<(String, String), Seal>;

fn seal_map(src: &str) -> SealMap {
    match lock::parse(src) {
        Ok(l) => l
            .entries
            .into_iter()
            .map(|e| ((e.key.req_label, e.key.item_path), e.seal))
            .collect(),
        Err(_) => SealMap::new(),
    }
}

enum Event {
    Introduced { req: String, item: String, seal: Seal },
    Resealed { req: String, item: String, from: Seal, to: Seal },
    Removed { req: String, item: String, seal: Seal },
}

impl Event {
    fn render(&self) -> String {
        match self {
            Event::Introduced { req, item, seal } => {
                format!("+ {} {}   {}", req, item_label(item), seal.render())
            }
            Event::Resealed { req, item, from, to } => {
                format!("~ {} {}   {} → {}", req, item_label(item), from.render(), to.render())
            }
            Event::Removed { req, item, seal } => {
                format!("- {} {}   {}", req, item_label(item), seal.render())
            }
        }
    }
}

fn diff(old: &SealMap, new: &SealMap) -> Vec<Event> {
    let mut events = Vec::new();
    for (k, seal) in new {
        match old.get(k) {
            None => events.push(Event::Introduced {
                req: k.0.clone(),
                item: k.1.clone(),
                seal: seal.clone(),
            }),
            Some(prev) if prev != seal => events.push(Event::Resealed {
                req: k.0.clone(),
                item: k.1.clone(),
                from: prev.clone(),
                to: seal.clone(),
            }),
            _ => {}
        }
    }
    for (k, seal) in old {
        if !new.contains_key(k) {
            events.push(Event::Removed {
                req: k.0.clone(),
                item: k.1.clone(),
                seal: seal.clone(),
            });
        }
    }
    events
}

fn item_label(item: &str) -> &str {
    if item == STATEMENT_MARKER {
        "(statement)"
    } else {
        item
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
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
