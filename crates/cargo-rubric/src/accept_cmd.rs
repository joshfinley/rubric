//! `cargo rubric accept`: scan annotations and re-seal the chain.
//!
//! Builds the lock from the current scan and manifest (one statement
//! seal per requirement, plus a body seal per cited item), writes
//! `rubric.lock`, and prints what changed against the previous lock.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::{self, ItemFacts, ATTEST_MARKER, STATEMENT_MARKER};
use rubric_trace::hash;
use rubric_trace::lock::{self, Entry, Key, Lock, Origin, Seal};
use rubric_trace::manifest::{Manifest, Requirement, SealMode};

use crate::members;
use crate::project;

pub fn run() -> ExitCode {
    members::drive(accept_one)
}

fn accept_one(root: &Path, label: Option<&str>) -> Result<bool, String> {
    if let Some(l) = label {
        println!("== {l} ==");
    }
    let p = project::load(root)?;

    let new = build_lock(&p.manifest, &p.scan.items, &p.scan.citations, &p.lock);
    report_diff(&p.lock, &new);

    std::fs::write(root.join("rubric.lock"), lock::render(&new))
        .map_err(|e| format!("writing rubric.lock: {e}"))?;
    Ok(true)
}

/// Re-seal: a statement entry per requirement, plus a content entry per
/// cited item. The requirement's seal mode picks what each entry hashes
/// (body, signature, both, or nothing). `seal = off` gets `off` seals.
///
/// Existing `<attest>` entries are carried over from `prev` untouched.
/// `attest` writes them. A re-`accept` that moves a leg leaves the stale
/// root in place, and `check` reports the requirement as unreconciled.
fn build_lock(
    manifest: &Manifest,
    items: &[ItemFacts],
    citations: &[rubric_trace::check::Citation],
    prev: &Lock,
) -> Lock {
    let reqs: BTreeMap<&str, &Requirement> =
        manifest.requirements.iter().map(|r| (r.label.as_str(), r)).collect();
    let items_by: BTreeMap<&str, &ItemFacts> =
        items.iter().map(|i| (i.path.as_str(), i)).collect();

    // Dedup by key. A Hash seal wins over Off, Annotation origin over Declared.
    let mut entries: BTreeMap<Key, Entry> = BTreeMap::new();

    for r in &manifest.requirements {
        let seal = if r.seal == SealMode::Off {
            Seal::Off
        } else {
            parse_seal(&hash::statement_seal(&r.statement))
        };
        insert(&mut entries, Key {
            req_label: r.label.clone(),
            item_path: STATEMENT_MARKER.to_string(),
        }, Origin::Declared, seal);
    }

    for c in citations {
        let seal = match (reqs.get(c.req_label.as_str()), items_by.get(c.item_path.as_str())) {
            (Some(r), Some(item)) => match check::current_seal(r, item) {
                Some(s) => parse_seal(&s),
                None => Seal::Off, // existence-only mode or bodyless/external item
            },
            // Unknown requirement or unresolved item: existence-only.
            _ => Seal::Off,
        };
        insert(&mut entries, Key {
            req_label: c.req_label.clone(),
            item_path: c.item_path.clone(),
        }, c.origin, seal);
    }

    // Carry over `<attest>` roots verbatim (`accept` must not recompute them),
    // but only for requirements still present and reconciling. A dropped or
    // de-reconciled requirement's root is not kept.
    for e in &prev.entries {
        if e.key.item_path == ATTEST_MARKER
            && reqs.get(e.key.req_label.as_str()).is_some_and(|r| r.reconcile)
        {
            entries.entry(e.key.clone()).or_insert_with(|| e.clone());
        }
    }

    Lock { entries: entries.into_values().collect() }
}

fn insert(map: &mut BTreeMap<Key, Entry>, key: Key, origin: Origin, seal: Seal) {
    match map.get(&key) {
        // Keep a real hash over an Off, and Annotation origin over Declared.
        Some(existing) if matches!(existing.seal, Seal::Hash { .. }) && seal == Seal::Off => {}
        Some(existing) if existing.origin == Origin::Annotation && origin == Origin::Declared => {}
        _ => {
            map.insert(key.clone(), Entry { key, seal, origin });
        }
    }
}

fn parse_seal(s: &str) -> Seal {
    Seal::parse(s).expect("hash module emits valid seals")
}

fn report_diff(old: &Lock, new: &Lock) {
    let old_map: BTreeMap<&Key, &Seal> = old.entries.iter().map(|e| (&e.key, &e.seal)).collect();
    let new_map: BTreeMap<&Key, &Seal> = new.entries.iter().map(|e| (&e.key, &e.seal)).collect();

    let (mut added, mut changed, mut removed) = (0u32, 0u32, 0u32);
    for (k, seal) in &new_map {
        match old_map.get(k) {
            None => {
                added += 1;
                println!("+ {} {}", k.req_label, display_item(&k.item_path));
            }
            Some(prev) if prev != seal => {
                changed += 1;
                println!("~ {} {}", k.req_label, display_item(&k.item_path));
            }
            _ => {}
        }
    }
    for k in old_map.keys() {
        if !new_map.contains_key(*k) {
            removed += 1;
            println!("- {} {}", k.req_label, display_item(&k.item_path));
        }
    }

    if added + changed + removed == 0 {
        println!("rubric: chain already sealed, nothing changed ({} entries)", new.entries.len());
    } else {
        println!("\nrubric: {added} added, {changed} re-sealed, {removed} removed");
    }
}

fn display_item(item_path: &str) -> &str {
    if item_path == STATEMENT_MARKER {
        "(statement)"
    } else {
        item_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attest(label: &str, hex: &str) -> Entry {
        Entry {
            key: Key { req_label: label.into(), item_path: ATTEST_MARKER.into() },
            seal: Seal::Hash { scheme: "attest".into(), hex: hex.into() },
            origin: Origin::Declared,
        }
    }

    #[test]
    fn build_lock_keeps_only_live_reconcile_attest_roots() {
        let manifest = rubric_trace::manifest::parse(
            "[req.R]\nkind=\"functional\"\nstatement=\"s\"\nreconcile=true\nverified_by=[\"crate::t\"]\n",
        )
        .unwrap();
        // prev holds a root for the live `R` and a stale one for removed `GONE`.
        let prev = Lock { entries: vec![attest("R", "1111"), attest("GONE", "2222")] };
        let new = build_lock(&manifest, &[], &[], &prev);
        let has_attest = |label: &str| {
            new.entries.iter().any(|e| e.key.req_label == label && e.key.item_path == ATTEST_MARKER)
        };
        assert!(has_attest("R"));
        assert!(!has_attest("GONE"));
    }
}
