//! `cargo rubric log`: the seal-event timeline from the git history of
//! `rubric.lock`.
//!
//! Walks the commits that touched `rubric.lock`, parses each version, and
//! reports every `(requirement, item)` seal that was introduced, re-sealed,
//! or removed, newest first. The source diff is one `git show` away.

use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::STATEMENT_MARKER;
use rubric_trace::lock::Seal;

use crate::git_history::{lock_history, SealMap};
use crate::members;

pub fn run() -> ExitCode {
    members::drive(log_one)
}

fn log_one(root: &Path, label: Option<&str>) -> Result<bool, String> {
    if let Some(l) = label {
        println!("== {l} ==");
    }
    try_run(root).map(|()| true)
}

fn try_run(root: &Path) -> Result<(), String> {
    let (commits, versions, _) = lock_history(root, None)?;

    if commits.is_empty() {
        println!("no committed history for rubric.lock");
        return Ok(());
    }

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
