//! `cargo rubric matrix` — render the traceability matrix as markdown.

use std::path::PathBuf;

use rubric_core::manifest::{Direction, Manifest};

use crate::cli::Flags;
use crate::find_manifest;
use rubric_core::walker;

/// Render a markdown traceability table with stable FNV-1a IDs and annotation file:line cells
#[rubric::satisfies(crate::reqs::matrix::render_markdown)]
pub fn run(args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(args, &["--manifest-path", "--output"])?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let targets = crate::workspace::manifest_targets(flags.manifest_path.as_deref(), &cwd);
    if targets.is_empty() {
        return Err("no rubric.toml found".to_string());
    }
    if targets.len() == 1 {
        return run_one(targets.into_iter().next().unwrap(), flags.output.as_deref());
    }
    // Workspace mode: print each member's matrix to stdout (separated),
    // or if --output is given, write each to <output>.<member>.md.
    let output_base = flags.output.as_deref();
    let mut failures = 0usize;
    for m in &targets {
        let parent = m.parent().unwrap_or(m);
        crate::term::status("Member", &parent.display().to_string());
        let out_override = output_base.map(|b| {
            let name = parent.file_name().and_then(|s| s.to_str()).unwrap_or("member");
            format!("{}.{}.md", b.trim_end_matches(".md"), name)
        });
        if run_one(m.clone(), out_override.as_deref()).is_err() { failures += 1; }
    }
    if failures > 0 { Err(format!("{} member(s) failed", failures)) } else { Ok(()) }
}

fn run_one(path: PathBuf, output: Option<&str>) -> Result<(), String> {
    let src = std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {}", path.display(), e))?;
    let manifest = Manifest::parse(&src).map_err(|e| e.to_string())?;
    let crate_root = path.parent().unwrap();
    let scan_root = crate_root.join("src");
    let scan_root = if scan_root.is_dir() { scan_root } else { crate_root.to_path_buf() };
    let annotations = walker::walk_dir(&scan_root).map_err(|e| e.to_string())?;

    let rendered = render(&manifest, &annotations, crate_root);
    match output {
        Some(out) => {
            let out_path = PathBuf::from(out);
            if let Some(parent) = out_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("creating {}: {}", parent.display(), e))?;
                }
            }
            std::fs::write(&out_path, &rendered)
                .map_err(|e| format!("writing {}: {}", out_path.display(), e))?;
            println!("wrote {}", out_path.display());
        }
        None => print!("{}", rendered),
    }
    Ok(())
}

pub fn render(manifest: &Manifest, annotations: &[walker::Annotation], crate_root: &std::path::Path) -> String {
    let mut out = String::new();
    out.push_str("# Traceability matrix\n\n");
    out.push_str("| ID | Requirement | Description | Satisfied by | Verified by | Doc |\n");
    out.push_str("|----|-------------|-------------|--------------|-------------|-----|\n");
    for req in &manifest.requirements {
        let id = req.id();
        let label = req.label();
        let desc = escape(&req.description);
        let mut satisfies = collect_sites(annotations, &req.label_path, Direction::Satisfies, crate_root);
        let mut verifies = collect_sites(annotations, &req.label_path, Direction::Verifies, crate_root);
        for p in &req.satisfied_by { satisfies.push(format!("`{}` (declared)", p)); }
        for p in &req.verified_by { verifies.push(format!("`{}` (declared)", p)); }
        let sat_cell = if satisfies.is_empty() { "**MISSING**".to_string() } else { satisfies.join("<br>") };
        let ver_cell = if verifies.is_empty() { "**MISSING**".to_string() } else { verifies.join("<br>") };
        let doc_cell = req.doc.as_deref().map(|d| format!("[{}]({})", d, d)).unwrap_or_else(|| "—".to_string());
        // The requirement label becomes an intra-doc link back to the
        // marker struct whose page documents the requirement in detail.
        let label_link = format!("[`{}`][crate::reqs::{}]", label, label);
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            id, label_link, desc, sat_cell, ver_cell, doc_cell,
        ));
    }
    out
}

fn collect_sites(
    anns: &[walker::Annotation],
    label: &[String],
    dir: Direction,
    crate_root: &std::path::Path,
) -> Vec<String> {
    anns.iter()
        .filter(|a| a.label_path == label && std::mem::discriminant(&a.direction) == std::mem::discriminant(&dir))
        .map(|a| {
            let rel = a.file.strip_prefix(crate_root).unwrap_or(&a.file);
            format!("`{}:{}`", rel.display(), a.line)
        })
        .collect()
}

fn escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubric_core::manifest::Manifest;
    use std::path::Path;

    #[test]
    #[rubric::verifies(crate::reqs::matrix::render_markdown)]
    fn render_produces_one_row_per_requirement() {
        let m = Manifest::parse("[meta]\nversion = 1\n[req.a.b]\ndescription = \"first\"\n[req.c.d]\ndescription = \"second\"\n").unwrap();
        let rendered = render(&m, &[], Path::new("/nonexistent"));
        assert!(rendered.contains("| ID | Requirement"));
        assert!(rendered.contains("[`a::b`][crate::reqs::a::b]"));
        assert!(rendered.contains("[`c::d`][crate::reqs::c::d]"));
        // Both annotations cells show MISSING since no sites.
        assert_eq!(rendered.matches("**MISSING**").count(), 4);
    }

    #[test]
    fn render_honors_manifest_declared_paths() {
        let m = Manifest::parse(r#"
[meta]
version = 1
[req.x.y]
description = "desc"
satisfied_by = ["mycrate::impl_fn"]
verified_by = ["mycrate::test_fn"]
"#).unwrap();
        let rendered = render(&m, &[], Path::new("/nonexistent"));
        assert!(rendered.contains("`mycrate::impl_fn` (declared)"));
        assert!(rendered.contains("`mycrate::test_fn` (declared)"));
        assert!(!rendered.contains("**MISSING**"));
    }
}
