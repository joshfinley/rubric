//! `build.rs` helpers.
//!
//! Consumers write:
//!
//! ```no_run
//! // build.rs
//! fn main() { rubric_core::build::check_and_warn(); }
//! ```
//!
//! That single call does the full drift pass: walks `src/`, resolves each
//! annotation, compares body hashes to `rubric.lock`, emits one
//! `cargo:warning=…` per finding, and registers `cargo:rerun-if-changed`
//! for every file the check looked at. Release profile (or
//! `[meta] strict = true` in `rubric.toml`) elevates findings to build
//! failures via non-zero exit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::codegen;
use crate::hash;
use crate::lockfile::{Seal, Lockfile};
use crate::manifest::{Direction, Manifest};
use crate::resolver::{AnnotationResolver, SyntacticResolver};
use crate::walker;

/// Drop-in for `build.rs`. Never panics on drift; returns after emitting
/// `cargo:warning=` lines. In strict mode (release profile or manifest
/// override) drift is printed via `cargo:warning=` and the process exits
/// with a non-zero status, which makes `cargo build` fail.
pub fn check_and_warn() {
    if let Err(e) = run() {
        // Surface catastrophic errors (malformed manifest etc.) as
        // warnings rather than panics; cargo doesn't need more.
        println!("cargo:warning=rubric: {}", e);
    }
}

fn run() -> Result<(), String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set (must be invoked from build.rs)".to_string())?;
    let crate_root = PathBuf::from(manifest_dir);

    let manifest_path = crate_root.join("rubric.toml");
    if !manifest_path.is_file() {
        // No rubric.toml — nothing to check. Silent.
        return Ok(());
    }
    let manifest_src = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {}", manifest_path.display(), e))?;
    let manifest = Manifest::parse(&manifest_src).map_err(|e| e.to_string())?;

    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let lock_path = crate_root.join("rubric.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());

    let src_root = crate_root.join("src");
    let scan_root = if src_root.is_dir() { src_root.clone() } else { crate_root.clone() };

    // Collect annotation sites and register every .rs file as a
    // rerun-trigger so edits invalidate the build script's cache.
    let sites = walker::walk_dir(&scan_root).map_err(|e| e.to_string())?;
    emit_rerun_for_tree(&scan_root);

    // Structural gaps.
    let mut findings: Vec<String> = Vec::new();
    let satisfied: BTreeSet<&[String]> = sites.iter()
        .filter(|a| matches!(a.direction, Direction::Satisfies))
        .map(|a| a.label_path.as_slice()).collect();
    let verified: BTreeSet<&[String]> = sites.iter()
        .filter(|a| matches!(a.direction, Direction::Verifies))
        .map(|a| a.label_path.as_slice()).collect();
    let known: BTreeSet<&[String]> = manifest.requirements.iter()
        .map(|r| r.label_path.as_slice()).collect();

    for req in &manifest.requirements {
        let label = req.label();
        if !satisfied.contains(req.label_path.as_slice()) && req.satisfied_by.is_empty() {
            findings.push(format!("unimplemented: `{}` has no satisfying annotation", label));
        }
        if !verified.contains(req.label_path.as_slice()) && req.verified_by.is_empty() {
            findings.push(format!("unverified: `{}` has no verifying annotation", label));
        }
    }
    for site in &sites {
        if !known.contains(site.label_path.as_slice()) {
            findings.push(format!(
                "orphan {}: `{}` at {}:{} (label not in manifest)",
                match site.direction { Direction::Satisfies => "satisfies", Direction::Verifies => "verifies" },
                site.label_path.join("::"),
                site.file.display(), site.line,
            ));
        }
    }

    // Seal drift.
    let lockfile = std::fs::read_to_string(&lock_path).ok()
        .map(|s| Lockfile::parse(&s).unwrap_or_default())
        .unwrap_or_default();
    let crate_name = derive_crate_name(&crate_root);
    let mut resolver = SyntacticResolver::new(crate_name, scan_root.clone());
    let mut src_cache: std::collections::HashMap<PathBuf, String> = std::collections::HashMap::new();
    for site in &sites {
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
            None => findings.push(format!(
                "function body seal missing: `{}` @ `{}` ({}:{}) — run `cargo rubric seal`",
                req_label, item_path, site.file.display(), site.line,
            )),
            Some(Seal::Off) => { /* audit-only; not a finding */ }
            Some(Seal::Hash { scheme, hex }) => {
                let actual = format!("{}:{}", scheme, hex);
                if actual != expected {
                    findings.push(format!(
                        "function body seal broken: `{}` @ `{}` ({}:{}) — lock={} now={} — run `cargo rubric seal`",
                        req_label, item_path, site.file.display(), site.line, actual, expected,
                    ));
                }
            }
        }
    }

    // Regenerate marker module + matrix into OUT_DIR so setup!() can
    // include_str them. Best-effort — emit a warning if OUT_DIR is unset
    // but don't fail.
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let out = PathBuf::from(out_dir);
        if let Err(e) = regen_artifacts(&out, &manifest, &sites, &crate_root) {
            findings.push(format!("failed to regenerate artifacts: {}", e));
        }
    }

    // Emit findings as warnings.
    for f in &findings {
        println!("cargo:warning=rubric: {}", f);
    }

    let strict = is_strict(&manifest);
    if strict && !findings.is_empty() {
        // Exit non-zero to fail the build.
        std::process::exit(1);
    }
    Ok(())
}

fn is_strict(manifest: &Manifest) -> bool {
    if manifest.strict { return true; }
    matches!(std::env::var("PROFILE").as_deref(), Ok("release"))
}

fn regen_artifacts(out: &Path, manifest: &Manifest, sites: &[walker::Annotation], crate_root: &Path) -> Result<(), String> {
    let marker = codegen::render(manifest);
    let marker_path = out.join("bind_marker.rs");
    std::fs::write(&marker_path, &marker)
        .map_err(|e| format!("writing {}: {}", marker_path.display(), e))?;

    let matrix = crate::build::render_matrix(manifest, sites, crate_root);
    let matrix_path = out.join("rubric_matrix.md");
    std::fs::write(&matrix_path, &matrix)
        .map_err(|e| format!("writing {}: {}", matrix_path.display(), e))?;
    Ok(())
}

/// Self-contained matrix renderer so build.rs doesn't need to depend on
/// cargo-rubric's CLI crate.
pub fn render_matrix(manifest: &Manifest, annotations: &[walker::Annotation], crate_root: &Path) -> String {
    let mut out = String::new();
    out.push_str("# Traceability matrix\n\n");
    out.push_str("| ID | Requirement | Description | Satisfied by | Verified by |\n");
    out.push_str("|----|-------------|-------------|--------------|-------------|\n");
    for req in &manifest.requirements {
        let id = req.id();
        let label = req.label();
        let desc = req.description.replace('|', "\\|").replace('\n', " ");
        let mut sat: Vec<String> = annotations.iter()
            .filter(|a| a.label_path == req.label_path && matches!(a.direction, Direction::Satisfies))
            .map(|a| format!("`{}:{}`", a.file.strip_prefix(crate_root).unwrap_or(&a.file).display(), a.line))
            .collect();
        let mut ver: Vec<String> = annotations.iter()
            .filter(|a| a.label_path == req.label_path && matches!(a.direction, Direction::Verifies))
            .map(|a| format!("`{}:{}`", a.file.strip_prefix(crate_root).unwrap_or(&a.file).display(), a.line))
            .collect();
        for p in &req.satisfied_by { sat.push(format!("`{}` (declared)", p)); }
        for p in &req.verified_by { ver.push(format!("`{}` (declared)", p)); }
        let sat_cell = if sat.is_empty() { "**MISSING**".to_string() } else { sat.join("<br>") };
        let ver_cell = if ver.is_empty() { "**MISSING**".to_string() } else { ver.join("<br>") };
        out.push_str(&format!(
            "| `{}` | [`{}`][crate::reqs::{}] | {} | {} | {} |\n",
            id, label, label, desc, sat_cell, ver_cell,
        ));
    }
    out
}

fn emit_rerun_for_tree(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.starts_with('.') || name == "target" { continue; }
                emit_rerun_for_tree(&path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn derive_crate_name(crate_root: &Path) -> String {
    let cargo_toml = crate_root.join("Cargo.toml");
    let Ok(src) = std::fs::read_to_string(&cargo_toml) else { return "crate".to_string(); };
    let mut in_package = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') { in_package = t == "[package]"; continue; }
        if !in_package { continue; }
        if let Some(rest) = t.strip_prefix("name") {
            let rest = rest.trim_start_matches([' ', '\t', '=']);
            if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.find('"').map(|e| &s[..e])) {
                return inner.replace('-', "_");
            }
        }
    }
    "crate".to_string()
}
