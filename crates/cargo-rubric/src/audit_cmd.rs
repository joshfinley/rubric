//! `cargo rubric audit`: the temporal reconciliation check.
//!
//! Walks the git history of `rubric.lock` and reports any commit where a
//! `reconcile` requirement's leg seal moved without its `<attest>` root
//! moving in the same commit. That is a blind accept. The chain was
//! re-sealed and shipped without a review checkpoint. A `<since>` ref
//! scopes the walk to `<since>..HEAD`, judging a branch on its own commits;
//! the no-arg form is the full-history forensic report.
//!
//! The reconcile policy is read from the current manifest. The guarantee
//! depends on git history integrity. A force-push rewrites the branch and
//! changes descendant hashes. Git does not prevent it. Branch protection
//! provides the immutability.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::ATTEST_MARKER;

use crate::git_history::{lock_history, SealMap};
use crate::members;
use crate::project;

/// `cargo rubric audit [<since>]`: with a `<since>` ref, only commits in
/// `<since>..HEAD` are audited (a branch judged on its own commits);
/// without one, the whole `rubric.lock` history is walked.
pub fn run(args: &[String]) -> ExitCode {
    let base = args.first().cloned();
    members::drive(move |root, label| audit_one(root, label, base.as_deref()))
}

fn audit_one(root: &Path, label: Option<&str>, base: Option<&str>) -> Result<bool, String> {
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

    let (commits, versions, boundary) = lock_history(root, base)?;
    if commits.is_empty() {
        match base {
            Some(b) => println!("no rubric.lock changes since {b}"),
            None => println!("no committed history for rubric.lock"),
        }
        return Ok(true);
    }

    let mut clean = true;
    for (i, commit) in commits.iter().enumerate() {
        let new = &versions[i];
        let old = versions.get(i + 1).unwrap_or(&boundary);
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

/// Leg seals (anything but `<attest>`) of `label` that differ between `old`
/// and `new` — introduced, re-sealed, or removed.
fn legs_moved(label: &str, old: &SealMap, new: &SealMap) -> Vec<String> {
    let mut items: BTreeSet<&str> = BTreeSet::new();
    for (req, item) in new.keys().chain(old.keys()) {
        if req == label && item != ATTEST_MARKER {
            items.insert(item.as_str());
        }
    }
    items
        .into_iter()
        .filter(|item| {
            let key = (label.to_string(), item.to_string());
            old.get(&key) != new.get(&key)
        })
        .map(str::to_string)
        .collect()
}

/// Whether `label`'s `<attest>` root is present in `new` and differs from
/// `old`. A removed or unchanged root is not a re-attestation.
fn attest_moved(label: &str, old: &SealMap, new: &SealMap) -> bool {
    let key = (label.to_string(), ATTEST_MARKER.to_string());
    new.get(&key).is_some() && old.get(&key) != new.get(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubric_trace::lock::Seal;

    fn seal(hex: &str) -> Seal {
        Seal::Hash { scheme: "body".into(), hex: hex.into() }
    }

    fn map(entries: &[(&str, &str, Seal)]) -> SealMap {
        entries.iter().map(|(r, i, s)| ((r.to_string(), i.to_string()), s.clone())).collect()
    }

    #[test]
    fn legs_moved_detects_introduced_resealed_and_removed() {
        let old = map(&[("R", "crate::f", seal("aaaa")), ("R", "crate::g", seal("bbbb"))]);
        // f re-sealed, g removed, h introduced.
        let new = map(&[("R", "crate::f", seal("cccc")), ("R", "crate::h", seal("dddd"))]);
        assert_eq!(legs_moved("R", &old, &new), vec!["crate::f", "crate::g", "crate::h"]);
    }

    #[test]
    fn legs_moved_ignores_attest_and_other_labels() {
        let old = map(&[("R", ATTEST_MARKER, seal("aaaa")), ("OTHER", "crate::x", seal("bbbb"))]);
        let new = map(&[("R", ATTEST_MARKER, seal("cccc")), ("OTHER", "crate::x", seal("eeee"))]);
        assert!(legs_moved("R", &old, &new).is_empty());
    }

    #[test]
    fn attest_moved_requires_present_and_changed() {
        let with = |hex| map(&[("R", ATTEST_MARKER, seal(hex))]);
        let empty = SealMap::new();
        assert!(attest_moved("R", &empty, &with("aaaa"))); // introduced
        assert!(attest_moved("R", &with("aaaa"), &with("bbbb"))); // changed
        assert!(!attest_moved("R", &with("aaaa"), &with("aaaa"))); // unchanged
        assert!(!attest_moved("R", &with("aaaa"), &empty)); // removed
    }
}
