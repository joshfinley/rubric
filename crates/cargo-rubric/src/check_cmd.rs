//! `cargo rubric check`: read-only oracle verdict.
//!
//! Reads `rubric.toml` and `rubric.lock`, scans `src/`, runs the pure
//! oracle, and renders any findings grouped by requirement. Non-zero exit
//! on any finding, so it drops straight into CI.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use rubric_trace::check::{self, Finding, STATEMENT_MARKER};
use rubric_trace::manifest::Manifest;

use crate::members;
use crate::project;

pub fn run() -> ExitCode {
    members::drive(check_one)
}

/// Check one crate. Returns whether its chain is clean.
fn check_one(root: &Path, label: Option<&str>) -> Result<bool, String> {
    if let Some(l) = label {
        println!("== {l} ==");
    }
    let p = project::load(root)?;
    let report = check::check(&p.manifest, &p.lock, &p.scan);

    if report.findings.is_empty() {
        println!(
            "✓ rubric: chain intact ({} requirement{})",
            p.manifest.requirements.len(),
            if p.manifest.requirements.len() == 1 { "" } else { "s" },
        );
        return Ok(true);
    }

    render(&p.manifest, &report);
    Ok(false)
}

/// Group findings under their requirement and print a verdict block.
fn render(manifest: &Manifest, report: &check::Report) {
    let statements: BTreeMap<&str, &str> = manifest
        .requirements
        .iter()
        .map(|r| (r.label.as_str(), r.statement.as_str()))
        .collect();

    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &report.findings {
        let (label, line) = describe(f);
        grouped.entry(label).or_default().push(line);
    }

    for (label, lines) in &grouped {
        match statements.get(label.as_str()) {
            Some(stmt) => println!("✗ {label} \"{stmt}\""),
            None => println!("✗ {label}"),
        }
        for line in lines {
            println!("    {line}");
        }
    }

    let n = report.findings.len();
    eprintln!("\n{n} finding{}", if n == 1 { "" } else { "s" });
}

fn describe(f: &Finding) -> (String, String) {
    match f {
        Finding::MissingSatisfier { req_label } => {
            (req_label.clone(), "no satisfier cited".to_string())
        }
        Finding::MissingVerifier { req_label } => {
            (req_label.clone(), "no verifier cited".to_string())
        }
        Finding::UnresolvedPath { req_label, item_path } => {
            (req_label.clone(), format!("cited path does not resolve — {item_path}"))
        }
        Finding::SealBroken { req_label, item_path, .. } => {
            let what = if item_path == STATEMENT_MARKER {
                "statement changed since last accept".to_string()
            } else {
                format!("{item_path} changed since last accept")
            };
            (req_label.clone(), what)
        }
        Finding::DeadVerifier { req_label, item_path } => (
            req_label.clone(),
            format!("verifier not live (needs #[test], not #[ignore]) — {item_path}"),
        ),
        Finding::OrphanAnnotation { label, item_path } => (
            label.clone(),
            format!("annotation cites a requirement not in rubric.toml — {item_path}"),
        ),
        Finding::KindViolation { req_label, item_path } => (
            req_label.clone(),
            format!("'satisfies' on an invariant requirement — {item_path}"),
        ),
        Finding::Uncovered { req_label, item_path } => (
            req_label.clone(),
            format!("pub item not covered yet; run accept to acknowledge — {item_path}"),
        ),
    }
}
