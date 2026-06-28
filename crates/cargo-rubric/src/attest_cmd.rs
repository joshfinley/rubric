//! `cargo rubric attest`: record the attestation root for each reconcile
//! requirement.
//!
//! `attest` is the deliberate review checkpoint, kept separate from
//! `accept`. It refuses to run while the chain has findings other than the
//! reconciliation gap it exists to close. It then records the current root
//! for reconcile requirements in the lock under `<attest>`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::{self, Finding, ItemFacts, ATTEST_MARKER};
use rubric_trace::lock::{self, Entry, Key, Lock, Origin, Seal};
use rubric_trace::manifest::Requirement;

use crate::members;
use crate::project;

pub fn run() -> ExitCode {
    members::drive(attest_one)
}

fn attest_one(root: &Path, label: Option<&str>) -> Result<bool, String> {
    if let Some(l) = label {
        println!("== {l} ==");
    }
    let p = project::load(root)?;

    // Refuse to attest a chain that has findings other than the
    // reconciliation gap. Attestation vouches for a chain that is otherwise
    // intact. Fix a broken seal or missing leg first, then accept.
    let report = check::check(&p.manifest, &p.lock, &p.scan);
    let blockers = report
        .findings
        .iter()
        .filter(|f| !matches!(f, Finding::Unreconciled { .. }))
        .count();
    if blockers > 0 {
        println!(
            "refusing to attest: {blockers} unresolved finding(s). \
             Run `cargo rubric check`, then `accept`."
        );
        return Ok(false);
    }

    let reconcile: Vec<&Requirement> =
        p.manifest.requirements.iter().filter(|r| r.reconcile).collect();
    if reconcile.is_empty() {
        println!("no reconcile requirements; nothing to attest");
        return Ok(true);
    }

    let items: BTreeMap<&str, &ItemFacts> =
        p.scan.items.iter().map(|i| (i.path.as_str(), i)).collect();
    let mut lock = p.lock.clone();
    for r in &reconcile {
        let root_seal = check::attestation_root(r, &p.scan.citations, &items);
        upsert_attest(&mut lock, &r.label, &root_seal);
        println!("attested {}: {root_seal}", r.label);
    }

    std::fs::write(root.join("rubric.lock"), lock::render(&lock))
        .map_err(|e| format!("writing rubric.lock: {e}"))?;
    Ok(true)
}

/// Set (or replace) a requirement's `<attest>` entry.
fn upsert_attest(lock: &mut Lock, req_label: &str, root_seal: &str) {
    let key = Key { req_label: req_label.to_string(), item_path: ATTEST_MARKER.to_string() };
    let seal = Seal::parse(root_seal).expect("attestation_root emits a valid seal");
    lock.entries.retain(|e| e.key != key);
    lock.entries.push(Entry { key, seal, origin: Origin::Declared });
}
