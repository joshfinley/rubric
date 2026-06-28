//! `cargo rubric audit`: the temporal reconciliation check.
//!
//! Walks the git history of `rubric.lock` and reports any commit where a
//! `reconcile` requirement's leg seal moved without its `<attest>` root
//! moving in the same commit. That is a blind accept. The chain was
//! re-sealed and shipped without a review checkpoint.
//!
//! The reconcile policy is read from the current manifest. The guarantee
//! depends on git history integrity. A force-push rewrites the branch and
//! changes descendant hashes. Git does not prevent it. Branch protection
//! provides the immutability.

use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::ATTEST_MARKER;

use crate::git_history::{lock_history, SealMap};
use crate::members;
use crate::project;

pub fn run() -> ExitCode {
    members::drive(audit_one)
}

fn audit_one(root: &Path, label: Option<&str>) -> Result<bool, String> {
    if let Some(l) = label {
        println!("== {l} ==");
    }
    let manifest = project::read_manifest(root)?;
    let reconcile: Vec<&str> = manifest
        .requirements
        .iter()
        .filter(|r| r.reconcile)
        .map(|r| r.label.as_str())
        .collect();
    if reconcile.is_empty() {
        println!("no reconcile requirements; nothing to audit");
        return Ok(true);
    }

    let (commits, versions) = lock_history(root)?;
    if commits.is_empty() {
        println!("no committed history for rubric.lock");
        return Ok(true);
    }

    let empty = SealMap::new();
    let mut clean = true;
    for (i, commit) in commits.iter().enumerate() {
        let new = &versions[i];
        let old = versions.get(i + 1).unwrap_or(&empty);
        for label in &reconcile {
            let moved = legs_moved(label, old, new);
            if !moved.is_empty() && !attest_moved(label, old, new) {
                clean = false;
                let short = &commit.sha[..commit.sha.len().min(9)];
                println!("commit {short}  {}  {}", commit.date, commit.author);
                for item in moved {
                    println!("    {label} {item}   re-sealed without attestation");
                }
                println!();
            }
        }
    }

    if clean {
        println!("no unattested re-seals in rubric.lock history");
    }
    Ok(clean)
}

/// Leg seals (anything but `<attest>`) of `label` that were introduced or
/// re-sealed between `old` and `new`.
fn legs_moved(label: &str, old: &SealMap, new: &SealMap) -> Vec<String> {
    let mut moved = Vec::new();
    for ((req, item), seal) in new {
        if req != label || item == ATTEST_MARKER {
            continue;
        }
        if old.get(&(req.clone(), item.clone())) != Some(seal) {
            moved.push(item.clone());
        }
    }
    moved
}

/// Whether `label`'s `<attest>` root changed between `old` and `new`.
fn attest_moved(label: &str, old: &SealMap, new: &SealMap) -> bool {
    let key = (label.to_string(), ATTEST_MARKER.to_string());
    old.get(&key) != new.get(&key)
}
