//! `cargo rubric check` — structural gaps (unimplemented, unverified,
//! orphan) plus seal drift (missing or stale entries in `rubric.lock`).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use rubric_core::hash;
use rubric_core::lockfile::{Seal, Lockfile};
use rubric_core::manifest::{Direction, Manifest};
use rubric_core::resolver::AnnotationResolver;

use crate::cli::Flags;
use crate::find_manifest;
use rubric_core::resolver::SyntacticResolver;
use rubric_core::walker;

/// Report unimplemented, unverified, and orphan annotation gaps; exit non-zero when any are present
#[rubric::satisfies(crate::reqs::check::detect_unimplemented, crate::reqs::check::detect_unverified, crate::reqs::check::detect_orphan, crate::reqs::check::nonzero_on_gaps)]
pub fn run(args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(args, &["--manifest-path"])?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let targets = crate::workspace::manifest_targets(flags.manifest_path.as_deref(), &cwd);
    if targets.is_empty() {
        return Err("no rubric.toml found".to_string());
    }
    if targets.len() == 1 {
        return run_one(&targets[0]);
    }
    // Workspace mode: iterate, aggregate.
    let mut failures = 0usize;
    for m in &targets {
        let parent = m.parent().unwrap_or(m);
        crate::term::status("Member", &parent.display().to_string());
        if run_one(m).is_err() { failures += 1; }
    }
    if failures > 0 {
        Err(format!("{} member(s) failed check", failures))
    } else {
        Ok(())
    }
}

fn run_one(manifest_path: &std::path::Path) -> Result<(), String> {
    let src = std::fs::read_to_string(manifest_path).map_err(|e| format!("reading {}: {}", manifest_path.display(), e))?;
    let manifest = Manifest::parse(&src).map_err(|e| e.to_string())?;
    let manifest_path = manifest_path.to_path_buf();
    let crate_root = manifest_path.parent().unwrap().to_path_buf();
    let scan_root = crate_root.join("src");
    let scan_root = if scan_root.is_dir() { scan_root } else { crate_root.clone() };

    let annotations = walker::walk_dir(&scan_root).map_err(|e| e.to_string())?;
    let mut report = compute_gaps(&manifest, &annotations);

    // Seal drift: load lockfile (if any), resolve each site, compute expected
    // hash, compare against lockfile entry. Missing entries and hash
    // mismatches both become drift findings.
    let lock_path = crate_root.join("rubric.lock");
    let lockfile = std::fs::read_to_string(&lock_path).ok()
        .map(|s| Lockfile::parse(&s).unwrap_or_default())
        .unwrap_or_default();
    let crate_name = crate::seal_util::derive_crate_name(&crate_root);
    let mut resolver = SyntacticResolver::new(crate_name, scan_root);
    let mut src_cache: HashMap<PathBuf, String> = HashMap::new();
    let seal_report = compute_seal_gaps(&annotations, &mut resolver, &lockfile, &mut src_cache);
    report.seal_drift = seal_report.seal_drift;
    report.off_entries = seal_report.off_entries;

    use crate::term;
    term::status("Checking", &format!("bind requirements in {}", crate_root.display()));

    let findings = report.detailed_findings();
    let total = findings.len();
    for f in &findings {
        term::error(&f.headline);
        term::help(&f.help);
    }
    if !report.off_entries.is_empty() {
        for (r, i) in &report.off_entries {
            term::note(&format!("`{}` @ `{}` has an explicit `off` opt-out (audit note, not failing)", r, i));
        }
    }

    if total > 0 {
        // Summary status in the terminal: keep it short, not redundant with
        // per-finding errors.
        term::status("Found",
            &format!("{} issue{} — see errors above", total, if total == 1 { "" } else { "s" }));
        Err(format!("{} issue(s)", total))
    } else {
        let n = manifest.requirements.len();
        term::status("Finished", &format!("{} requirement{}, no issues", n, if n == 1 { "" } else { "s" }));
        Ok(())
    }
}

/// A single finding packaged for human-friendly rendering: a one-line
/// headline that names the condition in plain terms, and a `help:` line
/// that tells the reader exactly what to do next.
#[derive(Debug)]
pub struct Finding {
    pub headline: String,
    pub help: String,
}

fn load_manifest(explicit: Option<String>) -> Result<(PathBuf, Manifest), String> {
    let path = match explicit {
        Some(p) => PathBuf::from(p),
        None => {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            find_manifest(&cwd).ok_or_else(|| "no rubric.toml found".to_string())?
        }
    };
    let src = std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {}", path.display(), e))?;
    let manifest = Manifest::parse(&src).map_err(|e| e.to_string())?;
    Ok((path, manifest))
}

#[derive(Debug, Clone)]
pub enum SealGap {
    Missing { req_label: String, item_path: String, file: PathBuf, line: usize },
    Broken { req_label: String, item_path: String, expected: String, actual: String, file: PathBuf, line: usize },
}

#[derive(Debug, Default)]
pub struct GapReport {
    pub unimplemented: Vec<String>,
    pub unverified: Vec<String>,
    pub orphan_satisfies: Vec<(String, PathBuf, usize)>,
    pub orphan_verifies: Vec<(String, PathBuf, usize)>,
    pub seal_drift: Vec<SealGap>,
    pub off_entries: Vec<(String, String)>, // (req_label, item_path) — audit note, not a failure
}

impl GapReport {
    pub fn total(&self) -> usize {
        self.unimplemented.len() + self.unverified.len()
            + self.orphan_satisfies.len() + self.orphan_verifies.len()
            + self.seal_drift.len()
    }
    pub fn has_gaps(&self) -> bool { self.total() > 0 }

    /// Render findings in plain, self-teaching language. Each finding is a
    /// headline stating the problem and a help line stating the fix.
    pub fn detailed_findings(&self) -> Vec<Finding> {
        let mut v = Vec::new();
        for label in &self.unimplemented {
            v.push(Finding {
                headline: format!("requirement `{}` has no implementation", label),
                help: format!(
                    "add `#[satisfies(crate::reqs::{})]` to the fn that implements it, or remove `[req.{}]` from rubric.toml if the requirement no longer applies",
                    label, label.replace("::", "."),
                ),
            });
        }
        for label in &self.unverified {
            v.push(Finding {
                headline: format!("requirement `{}` has no verifying test", label),
                help: format!(
                    "add `#[verifies(crate::reqs::{})]` to a test that exercises it, or remove `[req.{}]` from rubric.toml if the requirement no longer applies",
                    label, label.replace("::", "."),
                ),
            });
        }
        for (label, file, line) in &self.orphan_satisfies {
            v.push(Finding {
                headline: format!("annotation refers to `{}` but that requirement isn't in rubric.toml", label),
                help: format!(
                    "at {}:{} — either add `[req.{}]` to rubric.toml with a description, or fix the path in the #[satisfies(…)] attribute to match an existing requirement",
                    file.display(), line, label.replace("::", "."),
                ),
            });
        }
        for (label, file, line) in &self.orphan_verifies {
            v.push(Finding {
                headline: format!("test annotation refers to `{}` but that requirement isn't in rubric.toml", label),
                help: format!(
                    "at {}:{} — either add `[req.{}]` to rubric.toml with a description, or fix the path in the #[verifies(…)] attribute",
                    file.display(), line, label.replace("::", "."),
                ),
            });
        }
        for g in &self.seal_drift {
            match g {
                SealGap::Missing { req_label, item_path, file, line } =>
                    v.push(Finding {
                        headline: format!(
                            "function body seal missing: `{}` @ `{}`",
                            req_label, item_path,
                        ),
                        help: format!(
                            "at {}:{} — this is a new annotation; run `cargo rubric seal` to record its current body hash so future body edits can be detected as drift",
                            file.display(), line,
                        ),
                    }),
                SealGap::Broken { req_label, item_path, expected, actual, file, line } =>
                    v.push(Finding {
                        headline: format!(
                            "function body seal broken: `{}` @ `{}`",
                            req_label, item_path,
                        ),
                        help: format!(
                            "at {}:{} — review the change; if the new behavior is correct, run `cargo rubric seal` to refresh rubric.lock  (was {}, now {})",
                            file.display(), line, expected, actual,
                        ),
                    }),
            }
        }
        v
    }

    pub fn render(&self) -> String {
        let mut out = String::from("bind check\n");
        let section = |out: &mut String, title: &str, items: Vec<String>| {
            if items.is_empty() {
                out.push_str(&format!("  {}: none\n", title));
            } else {
                out.push_str(&format!("  {}:\n", title));
                for item in items { out.push_str(&format!("    - {}\n", item)); }
            }
        };
        section(&mut out, "unimplemented requirements (no `satisfies` annotation)", self.unimplemented.clone());
        section(&mut out, "unverified requirements (no `verifies` annotation)", self.unverified.clone());
        section(&mut out, "orphan `satisfies` (label not in manifest)",
            self.orphan_satisfies.iter().map(|(l, f, n)| format!("{} ({}:{})", l, f.display(), n)).collect());
        section(&mut out, "orphan `verifies` (label not in manifest)",
            self.orphan_verifies.iter().map(|(l, f, n)| format!("{} ({}:{})", l, f.display(), n)).collect());
        section(&mut out, "seal drift (run `cargo rubric seal` to refresh rubric.lock)",
            self.seal_drift.iter().map(|g| match g {
                SealGap::Missing { req_label, item_path, file, line } =>
                    format!("missing: {} @ {} ({}:{})", req_label, item_path, file.display(), line),
                SealGap::Broken { req_label, item_path, expected, actual, file, line } =>
                    format!("broken:  {} @ {} (lock={}, now={}) ({}:{})",
                        req_label, item_path, expected, actual, file.display(), line),
            }).collect());
        if !self.off_entries.is_empty() {
            out.push_str("  explicit `off` opt-outs (audit note — not failing):\n");
            for (r, i) in &self.off_entries {
                out.push_str(&format!("    - {} @ {}\n", r, i));
            }
        }
        out
    }
}

pub fn compute_gaps(manifest: &Manifest, annotations: &[walker::Annotation]) -> GapReport {
    let mut report = GapReport::default();

    let satisfied: BTreeSet<&[String]> = annotations.iter()
        .filter(|a| matches!(a.direction, Direction::Satisfies))
        .map(|a| a.label_path.as_slice())
        .collect();
    let verified: BTreeSet<&[String]> = annotations.iter()
        .filter(|a| matches!(a.direction, Direction::Verifies))
        .map(|a| a.label_path.as_slice())
        .collect();
    let known: BTreeSet<&[String]> = manifest.requirements.iter()
        .map(|r| r.label_path.as_slice())
        .collect();

    for req in &manifest.requirements {
        let label = req.label();
        // A requirement is "satisfied" when an annotation matches OR the
        // manifest declares a satisfier path explicitly. The latter covers
        // crates that can't consume the proc-macros (e.g. dependency
        // cycles), at the cost of no automatic drift tracking on the
        // declared path.
        let has_annotation = satisfied.contains(req.label_path.as_slice());
        let has_assertion = !req.satisfied_by.is_empty();
        if !has_annotation && !has_assertion {
            report.unimplemented.push(label.clone());
        }
        let has_verify_annotation = verified.contains(req.label_path.as_slice());
        let has_verify_assertion = !req.verified_by.is_empty();
        if !has_verify_annotation && !has_verify_assertion {
            report.unverified.push(label);
        }
    }
    for ann in annotations {
        if !known.contains(ann.label_path.as_slice()) {
            let label = ann.label_path.join("::");
            match ann.direction {
                Direction::Satisfies => report.orphan_satisfies.push((label, ann.file.clone(), ann.line)),
                Direction::Verifies => report.orphan_verifies.push((label, ann.file.clone(), ann.line)),
            }
        }
    }
    report
}

/// For every annotation that resolves to a hashable body, compare its
/// current hash against the lockfile entry. Return missing / broken
/// seal findings, plus the list of `off` opt-outs seen (audit note).
pub fn compute_seal_gaps(
    annotations: &[walker::Annotation],
    resolver: &mut dyn AnnotationResolver,
    lockfile: &Lockfile,
    src_cache: &mut HashMap<PathBuf, String>,
) -> GapReport {
    let mut report = GapReport::default();
    for site in annotations {
        let source = match src_cache.get(&site.file) {
            Some(s) => s.clone(),
            None => match std::fs::read_to_string(&site.file) {
                Ok(s) => { src_cache.insert(site.file.clone(), s.clone()); s }
                Err(_) => continue,
            }
        };
        let Ok(info) = resolver.resolve(site, &source) else { continue; };
        let Ok(expected) = hash::compute(hash::DEFAULT_SCHEME, &info) else { continue; };
        let req_label = site.label_path.join("::");
        let item_path = info.path.clone();
        match lockfile.get(&req_label, &item_path) {
            None => report.seal_drift.push(SealGap::Missing {
                req_label, item_path,
                file: site.file.clone(), line: site.line,
            }),
            Some(Seal::Off) => {
                report.off_entries.push((req_label, item_path));
            }
            Some(Seal::Hash { scheme, hex }) => {
                let actual = format!("{}:{}", scheme, hex);
                if actual != expected {
                    report.seal_drift.push(SealGap::Broken {
                        req_label, item_path,
                        expected: actual, actual: expected,
                        file: site.file.clone(), line: site.line,
                    });
                }
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn manifest_with(reqs: &[&str]) -> Manifest {
        let mut src = String::from("[meta]\nversion = 1\n");
        for r in reqs {
            src.push_str(&format!("[req.{}]\ndescription = \"x\"\n", r));
        }
        Manifest::parse(&src).unwrap()
    }
    fn ann(dir: Direction, label: &[&str]) -> walker::Annotation {
        walker::Annotation {
            direction: dir,
            label_path: label.iter().map(|s| s.to_string()).collect(),
            file: Path::new("x.rs").to_path_buf(),
            line: 1,
            byte_offset: 0,
            explicit_id: None,
        }
    }

    #[test]
    #[rubric::verifies(crate::reqs::check::detect_unimplemented)]
    fn unimplemented_when_no_satisfies() {
        let m = manifest_with(&["a.b"]);
        let g = compute_gaps(&m, &[ann(Direction::Verifies, &["a", "b"])]);
        assert_eq!(g.unimplemented, vec!["a::b"]);
        assert!(g.unverified.is_empty());
    }
    #[test]
    #[rubric::verifies(crate::reqs::check::detect_unverified)]
    fn unverified_when_no_verifies() {
        let m = manifest_with(&["a.b"]);
        let g = compute_gaps(&m, &[ann(Direction::Satisfies, &["a", "b"])]);
        assert_eq!(g.unverified, vec!["a::b"]);
    }
    #[test]
    #[rubric::verifies(crate::reqs::check::detect_orphan)]
    fn orphan_when_label_unknown() {
        let m = manifest_with(&["a.b"]);
        let g = compute_gaps(&m, &[ann(Direction::Satisfies, &["a", "b"]), ann(Direction::Verifies, &["a", "b"]), ann(Direction::Satisfies, &["zzz"])]);
        assert_eq!(g.orphan_satisfies.len(), 1);
        assert!(g.unimplemented.is_empty());
    }
    #[test]
    fn no_gaps_when_complete() {
        let m = manifest_with(&["a.b"]);
        let g = compute_gaps(&m, &[ann(Direction::Satisfies, &["a", "b"]), ann(Direction::Verifies, &["a", "b"])]);
        assert!(!g.has_gaps());
    }

    #[test]
    #[rubric::verifies(crate::reqs::check::nonzero_on_gaps)]
    fn report_surfaces_gaps_for_ci() {
        // Any category non-empty → has_gaps true → total >= 1 → caller exits non-zero
        let m = manifest_with(&["a.b"]);
        let g = compute_gaps(&m, &[]);
        assert!(g.has_gaps());
        assert!(g.total() >= 1);
    }
}
