//! The oracle: pure verification of the attribution chain.
//!
//! `check` is a pure function. Callers do all I/O and lexing and pass the
//! scanned facts in. The same call backs both `cargo rubric check` and an
//! in-tree `build.rs`. Both run the same code, so the verdicts match.
//!
//! Inputs:
//!
//! - `Manifest`: the requirements (`rubric.toml`).
//! - `Lock`: the accepted seals (`rubric.lock`).
//! - `Scan`: what the scanner found in the current tree: every citation
//!   (annotations plus the manifest's declared paths, merged) and the
//!   facts about each cited item (resolution, test-ness, normalized body).
//!
//! A requirement's statement seal lives in the lock under the reserved
//! item path [`STATEMENT_MARKER`], which no Rust path or filename can
//! collide with.

use std::collections::BTreeMap;

use crate::hash;
use crate::lock::{Lock, Origin, Seal};
use crate::manifest::{Kind, Manifest, Requirement, SealMode};

/// Reserved lock item-path for a requirement's statement seal.
pub const STATEMENT_MARKER: &str = "<statement>";

/// Reserved lock item-path for a requirement's attestation root. `attest`
/// writes this entry; `accept` does not. A re-seal without re-attestation
/// is therefore visible.
pub const ATTEST_MARKER: &str = "<attest>";

/// Stand-in rendered when no accepted seal exists for a cited item yet
/// (a citation added but not `accept`ed).
const ABSENT: &str = "(absent)";

/// A citation leg's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Satisfies,
    Verifies,
}

/// One citation edge in the current tree: a source annotation or a
/// `satisfied_by`/`verified_by` declaration from the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    pub req_label: String,
    pub item_path: String,
    pub direction: Direction,
    pub origin: Origin,
}

/// An item's source visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    /// `pub(crate)`, `pub(super)`, or `pub(in path)`.
    PubCrate,
    Pub,
}

/// The kind of item a citation points at, for pointcut `kind` matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Fn,
    Struct,
    Enum,
    Union,
    Const,
    Static,
    TypeAlias,
    Trait,
    Mod,
}

/// What the scanner resolved about one cited item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFacts {
    pub path: String,
    /// Resolves to a real item, or the file exists for `external:` evidence.
    pub resolved: bool,
    /// The item is a test function (`#[test]`).
    pub is_test: bool,
    /// The test is `#[ignore]`d. Only meaningful when `is_test`.
    pub is_ignored: bool,
    /// The item's visibility, for pointcut matching.
    pub vis: Visibility,
    /// The item's kind, for pointcut matching.
    pub kind: ItemKind,
    /// Normalized body seal input. `None` for `external:` evidence and
    /// other items without a hashable body.
    pub body: Option<String>,
    /// Normalized signature seal input (visibility through the body brace,
    /// excluding the block). `None` for items without a hashable signature.
    pub signature: Option<String>,
    /// Precomputed `file:` seal of an `external:` evidence file's bytes,
    /// filled by the loader (which does the I/O). `None` for source items
    /// and for evidence that could not be read.
    pub evidence_seal: Option<String>,
}

/// Everything the scanner discovered, handed to the pure oracle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub citations: Vec<Citation>,
    pub items: Vec<ItemFacts>,
}

/// One verified defect in the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// Functional requirement with zero cited satisfiers.
    MissingSatisfier { req_label: String },
    /// Requirement (either kind) with zero cited verifiers.
    MissingVerifier { req_label: String },
    /// Cited item path doesn't resolve to a real item in the tree.
    UnresolvedPath { req_label: String, item_path: String },
    /// Recorded seal doesn't match current content (statement,
    /// satisfier body, or verifier body).
    SealBroken { req_label: String, item_path: String, recorded: String, current: String },
    /// Cited verifier exists but is `#[ignore]`d (or otherwise not live).
    DeadVerifier { req_label: String, item_path: String },
    /// Annotation cites a label not defined in the manifest.
    OrphanAnnotation { label: String, item_path: String },
    /// `satisfies` on an `invariant` requirement.
    KindViolation { req_label: String, item_path: String },
    /// A `satisfies` annotation on a non-function item. Non-fn items are
    /// bound by `cover` or `satisfied_by`, not by a comment/attribute.
    MisplacedAnnotation { label: String, item_path: String },
    /// A body-sealed citation resolves to an item with no body to hash. The
    /// author should pick an explicit seal mode (`signature` or `full`).
    SealModeMismatch { req_label: String, item_path: String },
    /// A `signature`/`full` seal on a requirement whose every cited item is
    /// external evidence, which is file-sealed and cannot honor the mode.
    SealModeOnExternal { req_label: String },
    /// A pointcut-covered item has no seal yet (a new `pub` that has not been `accept`ed).
    Uncovered { req_label: String, item_path: String },
    /// A `reconcile` requirement's current leg seals do not match the
    /// recorded `<attest>` root. A leg was re-sealed without a subsequent `attest`.
    Unreconciled { req_label: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Run the full check set as a pure function over the scanned facts.
// satisfies: CHECK-COVERAGE, CHECK-RESOLVE, CHECK-SEAL, CHECK-LIVE, CHECK-ORPHAN, CHECK-KIND, CHECK-COVER, CHECK-RECONCILE, CHECK-MISPLACED, CHECK-SEALMODE, CHECK-EXTSEAL
pub fn check(manifest: &Manifest, lock: &Lock, scan: &Scan) -> Report {
    let reqs: BTreeMap<&str, &Requirement> =
        manifest.requirements.iter().map(|r| (r.label.as_str(), r)).collect();
    let items: BTreeMap<&str, &ItemFacts> =
        scan.items.iter().map(|i| (i.path.as_str(), i)).collect();
    let seals: BTreeMap<(&str, &str), &Seal> = lock
        .entries
        .iter()
        .map(|e| ((e.key.req_label.as_str(), e.key.item_path.as_str()), &e.seal))
        .collect();

    let mut findings = Vec::new();

    // Checks 1, 2: coverage. `reqs` is sorted by label (BTreeMap).
    for (label, req) in &reqs {
        if req.kind == Kind::Functional && !has_citation(scan, label, Direction::Satisfies) {
            findings.push(Finding::MissingSatisfier { req_label: label.to_string() });
        }
        if !has_citation(scan, label, Direction::Verifies) {
            findings.push(Finding::MissingVerifier { req_label: label.to_string() });
        }
    }

    // Check 4a: statement seals. Requirements with `seal = off` are skipped.
    for (label, req) in &reqs {
        if req.seal == SealMode::Off {
            continue;
        }
        let current = hash::statement_seal(&req.statement);
        let recorded = rendered_seal(&seals, label, STATEMENT_MARKER);
        if recorded != current {
            findings.push(Finding::SealBroken {
                req_label: label.to_string(),
                item_path: STATEMENT_MARKER.to_string(),
                recorded,
                current,
            });
        }
    }

    // Per-citation checks (3, 5, 6, 7, 4b), in a deterministic order.
    let mut cites: Vec<&Citation> = scan.citations.iter().collect();
    cites.sort_by(|a, b| {
        (&a.req_label, &a.item_path, dir_ord(a.direction))
            .cmp(&(&b.req_label, &b.item_path, dir_ord(b.direction)))
    });

    for c in cites {
        // Check 6: orphan annotation (label not in manifest). Declared
        // citations come from the manifest, so they can't be orphans.
        let req = reqs.get(c.req_label.as_str()).copied();
        if req.is_none() {
            if c.origin == Origin::Annotation {
                findings.push(Finding::OrphanAnnotation {
                    label: c.req_label.clone(),
                    item_path: c.item_path.clone(),
                });
            }
            // No requirement to verify the rest against.
            continue;
        }
        let req = req.unwrap();

        // Check 3: path resolution.
        let item = items.get(c.item_path.as_str()).copied();
        let resolved = item.map(|i| i.resolved).unwrap_or(false);
        if !resolved {
            findings.push(Finding::UnresolvedPath {
                req_label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
            continue;
        }
        let item = item.unwrap();

        // Whether this citation comes from the requirement's cover pointcut.
        let covered = req.cover.as_ref().is_some_and(|pc| pc.matches(item));

        // Check 7: kind violation (satisfies on an invariant). Cover-pointcut
        // matches are exempt: a pointcut binds items as satisfiers regardless of kind.
        if c.direction == Direction::Satisfies && req.kind == Kind::Invariant && !covered {
            findings.push(Finding::KindViolation {
                req_label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
        }

        // A `satisfies` annotation must land on a function. Non-fn items are
        // bound by `cover` or `satisfied_by`, not a comment/attribute.
        if c.direction == Direction::Satisfies
            && c.origin == Origin::Annotation
            && item.kind != ItemKind::Fn
        {
            findings.push(Finding::MisplacedAnnotation {
                label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
            continue;
        }

        // Check 5: dead verifier. External evidence is exempt from the
        // live-test rule. Its resolution is file existence (check 3).
        if c.direction == Direction::Verifies
            && !is_external(&c.item_path)
            && (!item.is_test || item.is_ignored)
        {
            findings.push(Finding::DeadVerifier {
                req_label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
        }

        // Checks 4b + census. A covered item with no seal entry yet is an
        // unacknowledged join point (a new `pub`). Otherwise the seal mode
        // picks what to hash and a mismatch is drift.
        let recorded = rendered_seal(&seals, &c.req_label, &c.item_path);
        if covered && recorded == ABSENT {
            findings.push(Finding::Uncovered {
                req_label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
        } else if let Some(current) = current_seal(req, item) {
            if recorded != current {
                findings.push(Finding::SealBroken {
                    req_label: c.req_label.clone(),
                    item_path: c.item_path.clone(),
                    recorded,
                    current,
                });
            }
        } else if req.seal == SealMode::Body
            && !is_external(&c.item_path)
            && item.body.is_none()
        {
            // Body seal but the item has no body to hash. Report it rather
            // than silently sealing existence-only.
            findings.push(Finding::SealModeMismatch {
                req_label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
        }
    }

    // A signature/full seal needs source content. A requirement whose every
    // cited item is external evidence (file-sealed) can never honor it.
    for (label, req) in &reqs {
        if !matches!(req.seal, SealMode::Signature | SealMode::Full) {
            continue;
        }
        let mut legs = scan.citations.iter().filter(|c| c.req_label.as_str() == *label).peekable();
        if legs.peek().is_some() && legs.all(|c| is_external(&c.item_path)) {
            findings.push(Finding::SealModeOnExternal { req_label: label.to_string() });
        }
    }

    // Reconciliation: for each `reconcile` requirement, the current root must
    // match the recorded `<attest>`. `accept` moves leg seals but leaves
    // `<attest>` alone. An unattested re-seal stays red until `attest` runs.
    for (label, req) in &reqs {
        if !req.reconcile {
            continue;
        }
        let recorded = rendered_seal(&seals, label, ATTEST_MARKER);
        if recorded != attestation_root(req, scan) {
            findings.push(Finding::Unreconciled { req_label: label.to_string() });
        }
    }

    Report { findings }
}

/// A requirement's attestation root. Hashes its current leg seals (the
/// statement and each cited item's content seal) in deterministic order.
/// `attest` records this value. `check` recomputes it and compares.
// satisfies: CHECK-RECONCILE
pub fn attestation_root(req: &Requirement, scan: &Scan) -> String {
    let items: BTreeMap<&str, &ItemFacts> =
        scan.items.iter().map(|i| (i.path.as_str(), i)).collect();

    let mut legs: Vec<(String, String)> = Vec::new();
    if req.seal != SealMode::Off {
        legs.push((STATEMENT_MARKER.to_string(), hash::statement_seal(&req.statement)));
    }
    for c in &scan.citations {
        if c.req_label != req.label || c.item_path == ATTEST_MARKER {
            continue;
        }
        let seal = items
            .get(c.item_path.as_str())
            .and_then(|i| current_seal(req, i))
            .unwrap_or_else(|| "off".to_string());
        legs.push((c.item_path.clone(), seal));
    }
    // One entry per (req, item), matching the lock's keying.
    legs.sort();
    legs.dedup();

    let mut input = String::new();
    for (path, seal) in &legs {
        input.push_str(path);
        input.push('=');
        input.push_str(seal);
        input.push('\n');
    }
    hash::seal(hash::SCHEME_ATTEST, &input)
}

/// The seal a cited item should currently render to, given its
/// requirement's seal mode. `None` means existence-only (no content hash):
/// `seal = off`, or an item with no hashable content. `accept` writes this
/// value into the lock. `check` compares the recorded value against it.
/// Both sides agree by construction.
pub fn current_seal(req: &Requirement, item: &ItemFacts) -> Option<String> {
    if req.seal == SealMode::Off {
        return None;
    }
    // External evidence has no body or signature to hash. It is sealed by
    // its file bytes alone.
    if is_external(&item.path) {
        return item.evidence_seal.clone();
    }
    match req.seal {
        SealMode::Off => None,
        SealMode::Body => item.body.as_deref().map(hash::body_seal),
        SealMode::Signature => item.signature.as_deref().map(hash::signature_seal),
        SealMode::Full => match (item.signature.as_deref(), item.body.as_deref()) {
            (Some(sig), Some(body)) => Some(hash::full_seal(sig, body)),
            (Some(sig), None) => Some(hash::signature_seal(sig)),
            (None, Some(body)) => Some(hash::body_seal(body)),
            (None, None) => None,
        },
    }
}

fn has_citation(scan: &Scan, label: &str, dir: Direction) -> bool {
    scan.citations.iter().any(|c| c.req_label == label && c.direction == dir)
}

fn rendered_seal(seals: &BTreeMap<(&str, &str), &Seal>, label: &str, item: &str) -> String {
    seals.get(&(label, item)).map(|s| s.render()).unwrap_or_else(|| ABSENT.to_string())
}

fn is_external(item_path: &str) -> bool {
    item_path.starts_with("external:")
}

fn dir_ord(d: Direction) -> u8 {
    match d {
        Direction::Satisfies => 0,
        Direction::Verifies => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Entry, Key};
    use crate::manifest;

    fn cite(label: &str, item: &str, dir: Direction, origin: Origin) -> Citation {
        Citation { req_label: label.into(), item_path: item.into(), direction: dir, origin }
    }

    fn item(path: &str, resolved: bool, is_test: bool, is_ignored: bool, body: Option<&str>) -> ItemFacts {
        ItemFacts {
            path: path.into(),
            resolved,
            is_test,
            is_ignored,
            vis: Visibility::Private,
            kind: ItemKind::Fn,
            body: body.map(|s| s.into()),
            signature: None,
            evidence_seal: None,
        }
    }

    fn hash_entry(label: &str, item: &str, origin: Origin, seal: Seal) -> Entry {
        Entry { key: Key { req_label: label.into(), item_path: item.into() }, seal, origin }
    }

    fn parse_seal(s: &str) -> Seal {
        Seal::parse(s).unwrap()
    }

    /// A clean functional chain: one statement seal, one satisfier body
    /// seal, one verifier body seal, all matching.
    fn clean_scenario() -> (Manifest, Lock, Scan) {
        let m = manifest::parse(
            "[req.VOTER-1]\nkind = \"functional\"\nstatement = \"Two out of three\"\n",
        )
        .unwrap();

        let sat_body = "( a & b ) | ( b & c ) | ( a & c )";
        let ver_body = "assert ! ( vote ( true , true , false ) )";

        let lock = Lock {
            entries: vec![
                hash_entry("VOTER-1", STATEMENT_MARKER, Origin::Declared,
                    parse_seal(&hash::statement_seal("Two out of three"))),
                hash_entry("VOTER-1", "crate::voter::vote", Origin::Annotation,
                    parse_seal(&hash::body_seal(sat_body))),
                hash_entry("VOTER-1", "crate::voter::tests::two_against_one", Origin::Annotation,
                    parse_seal(&hash::body_seal(ver_body))),
            ],
        };

        let scan = Scan {
            citations: vec![
                cite("VOTER-1", "crate::voter::vote", Direction::Satisfies, Origin::Annotation),
                cite("VOTER-1", "crate::voter::tests::two_against_one", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                item("crate::voter::vote", true, false, false, Some(sat_body)),
                item("crate::voter::tests::two_against_one", true, true, false, Some(ver_body)),
            ],
        };
        (m, lock, scan)
    }

    #[test]
    fn clean_chain_has_no_findings() {
        let (m, lock, scan) = clean_scenario();
        assert!(check(&m, &lock, &scan).is_clean());
    }

    // verifies: CHECK-COVERAGE
    #[test]
    fn missing_satisfier() {
        let (m, lock, mut scan) = clean_scenario();
        scan.citations.retain(|c| c.direction != Direction::Satisfies);
        let r = check(&m, &lock, &scan);
        assert!(r.findings.contains(&Finding::MissingSatisfier { req_label: "VOTER-1".into() }));
    }

    // verifies: CHECK-COVERAGE
    #[test]
    fn missing_verifier() {
        let (m, lock, mut scan) = clean_scenario();
        scan.citations.retain(|c| c.direction != Direction::Verifies);
        let r = check(&m, &lock, &scan);
        assert!(r.findings.contains(&Finding::MissingVerifier { req_label: "VOTER-1".into() }));
    }

    // verifies: CHECK-RESOLVE
    #[test]
    fn unresolved_path() {
        let (m, lock, mut scan) = clean_scenario();
        for i in &mut scan.items {
            if i.path == "crate::voter::vote" {
                i.resolved = false;
            }
        }
        let r = check(&m, &lock, &scan);
        assert!(r.findings.contains(&Finding::UnresolvedPath {
            req_label: "VOTER-1".into(),
            item_path: "crate::voter::vote".into(),
        }));
    }

    // verifies: CHECK-SEAL
    #[test]
    fn broken_body_seal() {
        let (m, lock, mut scan) = clean_scenario();
        for i in &mut scan.items {
            if i.path == "crate::voter::vote" {
                i.body = Some("( a & b )".into()); // changed body
            }
        }
        let r = check(&m, &lock, &scan);
        assert!(r.findings.iter().any(|f| matches!(f,
            Finding::SealBroken { item_path, .. } if item_path == "crate::voter::vote")));
    }

    // verifies: CHECK-SEAL
    #[test]
    fn broken_statement_seal() {
        let (mut m, lock, scan) = clean_scenario();
        m.requirements[0].statement = "Two out of four".into(); // reworded
        let r = check(&m, &lock, &scan);
        assert!(r.findings.iter().any(|f| matches!(f,
            Finding::SealBroken { item_path, .. } if item_path == STATEMENT_MARKER)));
    }

    // verifies: CHECK-LIVE
    #[test]
    fn dead_verifier_when_ignored() {
        let (m, lock, mut scan) = clean_scenario();
        for i in &mut scan.items {
            if i.is_test {
                i.is_ignored = true;
            }
        }
        let r = check(&m, &lock, &scan);
        assert!(r.findings.iter().any(|f| matches!(f, Finding::DeadVerifier { .. })));
    }

    // verifies: CHECK-LIVE
    #[test]
    fn dead_verifier_when_not_a_test() {
        let (m, lock, mut scan) = clean_scenario();
        for i in &mut scan.items {
            if i.is_test {
                i.is_test = false; // the cited verifier is no longer a test fn
            }
        }
        let r = check(&m, &lock, &scan);
        assert!(r.findings.iter().any(|f| matches!(f, Finding::DeadVerifier { .. })));
    }

    // verifies: CHECK-ORPHAN
    #[test]
    fn orphan_annotation() {
        let (m, lock, mut scan) = clean_scenario();
        scan.citations.push(cite("GHOST-9", "crate::voter::vote", Direction::Satisfies, Origin::Annotation));
        let r = check(&m, &lock, &scan);
        assert!(r.findings.contains(&Finding::OrphanAnnotation {
            label: "GHOST-9".into(),
            item_path: "crate::voter::vote".into(),
        }));
    }

    // verifies: CHECK-KIND
    #[test]
    fn kind_violation_satisfies_on_invariant() {
        let m = manifest::parse(
            "[req.INV-1]\nkind = \"invariant\"\nstatement = \"pure\"\n",
        )
        .unwrap();
        let body = "x";
        let lock = Lock {
            entries: vec![
                hash_entry("INV-1", STATEMENT_MARKER, Origin::Declared,
                    parse_seal(&hash::statement_seal("pure"))),
                hash_entry("INV-1", "crate::f", Origin::Annotation, parse_seal(&hash::body_seal(body))),
                hash_entry("INV-1", "crate::t", Origin::Annotation, parse_seal(&hash::body_seal(body))),
            ],
        };
        let scan = Scan {
            citations: vec![
                cite("INV-1", "crate::f", Direction::Satisfies, Origin::Annotation),
                cite("INV-1", "crate::t", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                item("crate::f", true, false, false, Some(body)),
                item("crate::t", true, true, false, Some(body)),
            ],
        };
        let r = check(&m, &lock, &scan);
        assert!(r.findings.contains(&Finding::KindViolation {
            req_label: "INV-1".into(),
            item_path: "crate::f".into(),
        }));
    }

    // verifies: CHECK-MISPLACED
    #[test]
    fn satisfies_annotation_on_non_fn_is_misplaced() {
        let m = manifest::parse(
            "[req.R]\nkind = \"functional\"\nstatement = \"s\"\nverified_by = [\"crate::t\"]\n",
        )
        .unwrap();
        let mut config = item("crate::Config", true, false, false, None);
        config.kind = ItemKind::Struct;
        config.signature = Some("pub struct Config".into());
        let items = vec![config, item("crate::t", true, true, false, Some("ok"))];

        // A `satisfies` comment on the struct is misplaced.
        let scan = Scan {
            citations: vec![
                cite("R", "crate::Config", Direction::Satisfies, Origin::Annotation),
                cite("R", "crate::t", Direction::Verifies, Origin::Annotation),
            ],
            items: items.clone(),
        };
        assert!(check(&m, &Lock::default(), &scan).findings.contains(
            &Finding::MisplacedAnnotation { label: "R".into(), item_path: "crate::Config".into() }
        ));

        // A declared (cover/satisfied_by) binding on the same struct is not.
        let scan2 = Scan {
            citations: vec![
                cite("R", "crate::Config", Direction::Satisfies, Origin::Declared),
                cite("R", "crate::t", Direction::Verifies, Origin::Annotation),
            ],
            items,
        };
        assert!(!check(&m, &Lock::default(), &scan2)
            .findings
            .iter()
            .any(|f| matches!(f, Finding::MisplacedAnnotation { .. })));
    }

    // verifies: CHECK-SEALMODE
    #[test]
    fn body_seal_on_a_bodyless_item_is_a_mismatch() {
        let mut config = item("crate::Config", true, false, false, None);
        config.kind = ItemKind::Struct;
        config.signature = Some("pub struct Config".into());
        let items = vec![config, item("crate::t", true, true, false, Some("ok"))];
        let cites = vec![
            cite("R", "crate::Config", Direction::Satisfies, Origin::Declared),
            cite("R", "crate::t", Direction::Verifies, Origin::Annotation),
        ];
        let is_mismatch = |f: &Finding| matches!(f, Finding::SealModeMismatch { .. });

        // Default (body) mode: a struct has no body to hash -> mismatch.
        let body = manifest::parse(
            "[req.R]\nkind=\"functional\"\nstatement=\"s\"\n\
             satisfied_by=[\"crate::Config\"]\nverified_by=[\"crate::t\"]\n",
        )
        .unwrap();
        let scan = Scan { citations: cites.clone(), items: items.clone() };
        assert!(check(&body, &Lock::default(), &scan).findings.iter().any(is_mismatch));

        // Signature mode seals the struct's signature -> no mismatch.
        let sig = manifest::parse(
            "[req.R]\nkind=\"functional\"\nstatement=\"s\"\nseal=\"signature\"\n\
             satisfied_by=[\"crate::Config\"]\nverified_by=[\"crate::t\"]\n",
        )
        .unwrap();
        let scan2 = Scan { citations: cites, items };
        assert!(!check(&sig, &Lock::default(), &scan2).findings.iter().any(is_mismatch));
    }

    // verifies: CHECK-EXTSEAL
    #[test]
    fn full_seal_with_only_external_legs_is_a_mismatch() {
        let on_ext = |f: &Finding| matches!(f, Finding::SealModeOnExternal { .. });
        let ext = |path: &str| {
            let mut i = item(path, true, false, false, None);
            i.evidence_seal = Some("file:deadbeef".into());
            i
        };

        // Every leg external under `full` -> the mode can never apply.
        let only_ext = manifest::parse(
            "[req.R]\nkind=\"invariant\"\nstatement=\"s\"\nseal=\"full\"\n\
             verified_by=[\"external:docs/a.pdf\"]\n",
        )
        .unwrap();
        let scan = Scan {
            citations: vec![cite("R", "external:docs/a.pdf", Direction::Verifies, Origin::Declared)],
            items: vec![ext("external:docs/a.pdf")],
        };
        assert!(check(&only_ext, &Lock::default(), &scan).findings.iter().any(on_ext));

        // A source leg alongside the external one -> mixed is fine.
        let mixed = manifest::parse(
            "[req.R]\nkind=\"functional\"\nstatement=\"s\"\nseal=\"full\"\n\
             satisfied_by=[\"crate::f\"]\nverified_by=[\"external:docs/a.pdf\"]\n",
        )
        .unwrap();
        let mut f = item("crate::f", true, false, false, Some("b"));
        f.signature = Some("fn f ( )".into());
        let scan2 = Scan {
            citations: vec![
                cite("R", "crate::f", Direction::Satisfies, Origin::Declared),
                cite("R", "external:docs/a.pdf", Direction::Verifies, Origin::Declared),
            ],
            items: vec![f, ext("external:docs/a.pdf")],
        };
        assert!(!check(&mixed, &Lock::default(), &scan2).findings.iter().any(on_ext));
    }

    /// A `pub fn` item matching a cover pointcut, ready to mutate per test.
    fn covered_item(path: &str, body: &str) -> ItemFacts {
        let mut i = item(path, true, false, false, Some(body));
        i.vis = Visibility::Pub;
        i.kind = ItemKind::Fn;
        i.signature = Some(format!("pub fn {} ( )", path.rsplit("::").next().unwrap()));
        i
    }

    // verifies: CHECK-COVER
    #[test]
    fn uncovered_fires_for_new_pub() {
        let m = manifest::parse(
            "[req.NOPUB]\nkind = \"invariant\"\nstatement = \"surface reviewed\"\n\
             seal = \"full\"\ncover = \"pub fn within crate::api\"\n\
             verified_by = [\"crate::tests::surface\"]\n",
        )
        .unwrap();
        // Statement sealed, but the covered item has no entry yet.
        let lock = Lock {
            entries: vec![hash_entry(
                "NOPUB",
                STATEMENT_MARKER,
                Origin::Declared,
                parse_seal(&hash::statement_seal("surface reviewed")),
            )],
        };
        let scan = Scan {
            citations: vec![
                // The scanner's cover expansion injects this satisfier.
                cite("NOPUB", "crate::api::connect", Direction::Satisfies, Origin::Declared),
                cite("NOPUB", "crate::tests::surface", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                covered_item("crate::api::connect", "a + b"),
                item("crate::tests::surface", true, true, false, Some("ok")),
            ],
        };
        let r = check(&m, &lock, &scan);
        assert!(r.findings.contains(&Finding::Uncovered {
            req_label: "NOPUB".into(),
            item_path: "crate::api::connect".into(),
        }));
        // A cover match on an invariant is not a kind violation.
        assert!(!r.findings.iter().any(|f| matches!(f, Finding::KindViolation { .. })));
    }

    // verifies: CHECK-COVER
    #[test]
    fn covered_drift_is_sealbroken_not_uncovered() {
        let m = manifest::parse(
            "[req.NOPUB]\nkind = \"invariant\"\nstatement = \"s\"\n\
             seal = \"full\"\ncover = \"pub fn within crate::api\"\n\
             verified_by = [\"crate::tests::surface\"]\n",
        )
        .unwrap();
        let sig = "pub fn connect ( )";
        let recorded = hash::full_seal(sig, "old body");
        let lock = Lock {
            entries: vec![
                hash_entry("NOPUB", STATEMENT_MARKER, Origin::Declared,
                    parse_seal(&hash::statement_seal("s"))),
                hash_entry("NOPUB", "crate::api::connect", Origin::Declared, parse_seal(&recorded)),
            ],
        };
        let scan = Scan {
            citations: vec![
                cite("NOPUB", "crate::api::connect", Direction::Satisfies, Origin::Declared),
                cite("NOPUB", "crate::tests::surface", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                covered_item("crate::api::connect", "new body"), // body drifted
                item("crate::tests::surface", true, true, false, Some("ok")),
            ],
        };
        let r = check(&m, &lock, &scan);
        assert!(r.findings.iter().any(|f| matches!(f,
            Finding::SealBroken { item_path, .. } if item_path == "crate::api::connect")));
        assert!(!r.findings.iter().any(|f| matches!(f, Finding::Uncovered { .. })));
    }

    // verifies: CHECK-RECONCILE
    #[test]
    fn root_changes_when_any_leg_changes() {
        let m = manifest::parse(
            "[req.R]\nkind = \"functional\"\nstatement = \"s\"\nreconcile = true\n",
        )
        .unwrap();
        let req = &m.requirements[0];
        let scan = Scan {
            citations: vec![
                cite("R", "crate::f", Direction::Satisfies, Origin::Annotation),
                cite("R", "crate::t", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                item("crate::f", true, false, false, Some("body one")),
                item("crate::t", true, true, false, Some("test one")),
            ],
        };
        let root1 = attestation_root(req, &scan);
        assert!(root1.starts_with("attest:"));

        let mut scan2 = scan.clone();
        for i in &mut scan2.items {
            if i.path == "crate::f" {
                i.body = Some("body two".into());
            }
        }
        assert_ne!(root1, attestation_root(req, &scan2));
    }

    // verifies: CHECK-RECONCILE
    #[test]
    fn unreconciled_until_attest_records_the_root() {
        let m = manifest::parse(
            "[req.R]\nkind = \"functional\"\nstatement = \"s\"\nreconcile = true\n",
        )
        .unwrap();
        let req = &m.requirements[0];
        let scan = Scan {
            citations: vec![
                cite("R", "crate::f", Direction::Satisfies, Origin::Annotation),
                cite("R", "crate::t", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                item("crate::f", true, false, false, Some("b")),
                item("crate::t", true, true, false, Some("tb")),
            ],
        };
        // Legs sealed, but no `<attest>` entry: re-sealed without attestation.
        let lock = Lock {
            entries: vec![
                hash_entry("R", STATEMENT_MARKER, Origin::Declared,
                    parse_seal(&hash::statement_seal("s"))),
                hash_entry("R", "crate::f", Origin::Annotation, parse_seal(&hash::body_seal("b"))),
                hash_entry("R", "crate::t", Origin::Annotation, parse_seal(&hash::body_seal("tb"))),
            ],
        };
        assert!(check(&m, &lock, &scan)
            .findings
            .contains(&Finding::Unreconciled { req_label: "R".into() }));

        // Record the current root: now reconciled.
        let mut lock2 = lock.clone();
        lock2.entries.push(hash_entry(
            "R",
            ATTEST_MARKER,
            Origin::Declared,
            parse_seal(&attestation_root(req, &scan)),
        ));
        assert!(!check(&m, &lock2, &scan)
            .findings
            .iter()
            .any(|f| matches!(f, Finding::Unreconciled { .. })));
    }

    #[test]
    fn sig_only_skips_body_and_statement_seals() {
        let m = manifest::parse(
            "[req.X]\nkind = \"functional\"\nstatement = \"s\"\nsig_only = true\n",
        )
        .unwrap();
        // Lock has Off seals and no statement entry, and sig_only means
        // existence-only, so this must still be clean.
        let lock = Lock {
            entries: vec![
                hash_entry("X", "crate::f", Origin::Annotation, Seal::Off),
                hash_entry("X", "crate::t", Origin::Annotation, Seal::Off),
            ],
        };
        let scan = Scan {
            citations: vec![
                cite("X", "crate::f", Direction::Satisfies, Origin::Annotation),
                cite("X", "crate::t", Direction::Verifies, Origin::Annotation),
            ],
            items: vec![
                item("crate::f", true, false, false, Some("anything")),
                item("crate::t", true, true, false, Some("whatever")),
            ],
        };
        assert!(check(&m, &lock, &scan).is_clean());
    }

    // An invariant with a verifier but no satisfier is clean: the
    // satisfier rule is scoped to functional requirements.
    // verifies: CHECK-COVERAGE
    #[test]
    fn external_verifier_is_not_dead() {
        let m = manifest::parse(
            "[req.R]\nkind = \"invariant\"\nstatement = \"s\"\nverified_by = [\"external:docs/proof.pdf\"]\n",
        )
        .unwrap();
        let lock = Lock {
            entries: vec![
                hash_entry("R", STATEMENT_MARKER, Origin::Declared, parse_seal(&hash::statement_seal("s"))),
                hash_entry("R", "external:docs/proof.pdf", Origin::Declared, Seal::Off),
            ],
        };
        let scan = Scan {
            citations: vec![cite("R", "external:docs/proof.pdf", Direction::Verifies, Origin::Declared)],
            items: vec![item("external:docs/proof.pdf", true, false, false, None)],
        };
        assert!(check(&m, &lock, &scan).is_clean());
    }
}
