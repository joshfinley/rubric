//! `cargo rubric init`: scaffold a `rubric.toml`.
//!
//! Plain `init` writes a commented template (an empty manifest, so the
//! first `check` is clean). `--from-annotations` scans the source and
//! drafts a `[req.<LABEL>]` stub for every label already cited by a
//! `// satisfies:` / `// verifies:` marker, the on-ramp for a crate that
//! is already annotated.

use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::{Citation, Direction};
use rubric_trace::manifest::{Kind, Manifest};

use crate::scan::scan_files;
use crate::workspace;

pub fn run(args: &[String]) -> ExitCode {
    let from_annotations = match parse_args(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cargo rubric init: {e}");
            return ExitCode::FAILURE;
        }
    };
    match try_run(from_annotations) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cargo rubric init: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<bool, String> {
    let mut from_annotations = false;
    for a in args {
        match a.as_str() {
            "--from-annotations" => from_annotations = true,
            other => return Err(format!("unknown flag '{other}'")),
        }
    }
    Ok(from_annotations)
}

fn try_run(from_annotations: bool) -> Result<(), String> {
    let root = Path::new(".");
    let target = root.join("rubric.toml");
    if target.exists() {
        return Err("rubric.toml already exists here; edit it by hand or remove it first".into());
    }

    let (contents, note) = if from_annotations {
        let files = workspace::discover(root).map_err(|e| format!("reading sources: {e}"))?;
        let scan = scan_files(&files, &Manifest::default());
        let stubs = stubs_from_citations(&scan.citations);
        if stubs.is_empty() {
            (template(), "no annotations found; wrote a template".to_string())
        } else {
            let n = stubs.len();
            (from_annotations_manifest(&stubs), format!("drafted {n} requirement stub(s) from annotations"))
        }
    } else {
        (template(), "wrote a template".to_string())
    };

    std::fs::write(&target, contents).map_err(|e| format!("writing rubric.toml: {e}"))?;
    println!("rubric: {note} → rubric.toml");
    if from_annotations {
        println!("next: replace each `statement` placeholder, then `cargo rubric accept`");
    } else {
        println!("next: declare requirements, annotate source, then `cargo rubric accept`");
    }
    Ok(())
}

const HEADER: &str = "\
# rubric.toml — requirements traceability manifest.
#
# Each [req.<LABEL>] declares one requirement. Annotate the satisfying
# function and the verifying test in source with plain comments:
#
#     // satisfies: <LABEL>
#     // verifies:  <LABEL>
#
# Then seal the chain and check it in CI:
#
#     cargo rubric accept   # records seals into rubric.lock
#     cargo rubric check    # non-zero exit on drift
";

/// Empty manifest: a commented example, so the first `check` is clean.
fn template() -> String {
    format!(
        "{HEADER}\n\
# [req.EXAMPLE-1]\n\
# kind = \"functional\"      # or \"invariant\" (verified only, no satisfier)\n\
# statement = \"What this requirement guarantees\"\n",
    )
}

struct Stub {
    label: String,
    kind: Kind,
}

/// One stub per cited label. A label cited by any `satisfies` is treated
/// as functional; a verify-only label is drafted as an invariant (which
/// needs no satisfier), so the generated manifest is more likely to pass
/// `check` as-is. The guess is worth reviewing.
fn stubs_from_citations(citations: &[Citation]) -> Vec<Stub> {
    use std::collections::BTreeMap;
    let mut has_satisfier: BTreeMap<&str, bool> = BTreeMap::new();
    for c in citations {
        let entry = has_satisfier.entry(c.req_label.as_str()).or_insert(false);
        if c.direction == Direction::Satisfies {
            *entry = true;
        }
    }
    has_satisfier
        .into_iter()
        .map(|(label, functional)| Stub {
            label: label.to_string(),
            kind: if functional { Kind::Functional } else { Kind::Invariant },
        })
        .collect()
}

fn from_annotations_manifest(stubs: &[Stub]) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for s in stubs {
        out.push_str(&format!(
            "[req.{}]\nkind = \"{}\"\nstatement = \"TODO: describe {}\"\n\n",
            s.label,
            kind_str(s.kind),
            s.label,
        ));
    }
    out
}

fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Functional => "functional",
        Kind::Invariant => "invariant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubric_trace::lock::Origin;
    use rubric_trace::manifest;

    fn cite(label: &str, dir: Direction) -> Citation {
        Citation {
            req_label: label.into(),
            item_path: "crate::x".into(),
            direction: dir,
            origin: Origin::Annotation,
        }
    }

    #[test]
    fn template_parses_to_empty_clean_manifest() {
        let m = manifest::parse(&template()).unwrap();
        assert!(m.requirements.is_empty());
    }

    #[test]
    fn from_annotations_infers_kind() {
        let cites = vec![
            cite("FUN-1", Direction::Satisfies),
            cite("FUN-1", Direction::Verifies),
            cite("INV-1", Direction::Verifies),
        ];
        let stubs = stubs_from_citations(&cites);
        let manifest = manifest::parse(&from_annotations_manifest(&stubs)).unwrap();

        let fun = manifest.requirements.iter().find(|r| r.label == "FUN-1").unwrap();
        let inv = manifest.requirements.iter().find(|r| r.label == "INV-1").unwrap();
        assert_eq!(fun.kind, Kind::Functional);
        assert_eq!(inv.kind, Kind::Invariant);
    }

    #[test]
    fn stubs_are_sorted_and_unique() {
        let cites = vec![
            cite("B", Direction::Verifies),
            cite("A", Direction::Satisfies),
            cite("A", Direction::Verifies),
        ];
        let stubs = stubs_from_citations(&cites);
        let labels: Vec<_> = stubs.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["A", "B"]);
    }
}
