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
use crate::manifest::{Kind, Manifest, Requirement};

/// Reserved lock item-path for a requirement's statement seal.
pub const STATEMENT_MARKER: &str = "<statement>";

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
// satisfies: CHECK-COVERAGE, CHECK-RESOLVE, CHECK-SEAL, CHECK-LIVE, CHECK-ORPHAN, CHECK-KIND
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

    // Check 4a: statement seals. Every non-`sig_only` requirement has one.
    for (label, req) in &reqs {
        if req.sig_only {
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

        // Check 7: kind violation (satisfies on an invariant).
        if c.direction == Direction::Satisfies && req.kind == Kind::Invariant {
            findings.push(Finding::KindViolation {
                req_label: c.req_label.clone(),
                item_path: c.item_path.clone(),
            });
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

        // Check 4b: body seal. Existence-only for `sig_only` requirements
        // and items without a hashable body (e.g. `external:` evidence).
        if !req.sig_only {
            if let Some(body) = &item.body {
                let current = hash::body_seal(body);
                let recorded = rendered_seal(&seals, &c.req_label, &c.item_path);
                if recorded != current {
                    findings.push(Finding::SealBroken {
                        req_label: c.req_label.clone(),
                        item_path: c.item_path.clone(),
                        recorded,
                        current,
                    });
                }
            }
        }
    }

    Report { findings }
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
