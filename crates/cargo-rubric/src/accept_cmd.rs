//! `cargo rubric accept`: scan annotations and re-seal the chain.
//!
//! Builds the lock from the current scan and manifest (one statement
//! seal per requirement, plus a body seal per cited item), writes
//! `rubric.lock`, and prints what changed against the previous lock.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::{ItemFacts, STATEMENT_MARKER};
use rubric_trace::hash;
use rubric_trace::lock::{self, Entry, Key, Lock, Origin, Seal};
use rubric_trace::manifest::Manifest;

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

    let new = build_lock(&p.manifest, &p.scan.items, &p.scan.citations);
    report_diff(&p.lock, &new);

    std::fs::write(root.join("rubric.lock"), lock::render(&new))
        .map_err(|e| format!("writing rubric.lock: {e}"))?;
    Ok(true)
}

/// Re-seal: a statement entry per requirement, plus a body entry per
/// cited item. `sig_only` requirements get `off` seals (existence only).
fn build_lock(
    manifest: &Manifest,
    items: &[ItemFacts],
    citations: &[rubric_trace::check::Citation],
) -> Lock {
    let sig_only: BTreeMap<&str, bool> =
        manifest.requirements.iter().map(|r| (r.label.as_str(), r.sig_only)).collect();
    let body_of: BTreeMap<&str, Option<&str>> =
        items.iter().map(|i| (i.path.as_str(), i.body.as_deref())).collect();

    // Dedup by key. A Hash seal wins over Off, Annotation origin over Declared.
    let mut entries: BTreeMap<Key, Entry> = BTreeMap::new();

    for r in &manifest.requirements {
        let seal = if r.sig_only {
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
        let off = sig_only.get(c.req_label.as_str()).copied().unwrap_or(false);
        let seal = if off {
            Seal::Off
        } else {
            match body_of.get(c.item_path.as_str()).copied().flatten() {
                Some(body) => parse_seal(&hash::body_seal(body)),
                None => Seal::Off, // external evidence or bodyless item
            }
        };
        insert(&mut entries, Key {
            req_label: c.req_label.clone(),
            item_path: c.item_path.clone(),
        }, c.origin, seal);
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
