//! `cargo rubric init` — scaffold bind into an existing crate or workspace.
//!
//! Creates `rubric.toml` and `build.rs`, updates `Cargo.toml` and
//! `src/lib.rs` / `src/main.rs` idempotently. At a workspace root the
//! default is to refuse unless the caller opts in via `--all-members`
//! or `-p <name>`; this prevents surprise mutation of members the user
//! didn't intend.

use std::path::{Path, PathBuf};

use crate::term::{note, status};

const STARTER_MANIFEST: &str = r#"# rubric.toml — declare your requirements here.
#
# Each [req.a.b.c] section defines one requirement. A requirement is
# identified by its label path (here: a::b::c) and described by its
# `description` field. That is all this file needs for the common case.
#
# Source code declares which functions implement or verify each
# requirement via `#[satisfies(...)] / #[verifies(...)]` annotations — the tool consumes those
# from source directly. You do not duplicate them here.
#
# Run `cargo rubric seal` after edits to refresh per-item hashes in
# rubric.lock. Run `cargo build --release` to block on drift (or set
# [meta] strict = true to block on every build).

[meta]
version = 1

# [req.example.greeting]
# description = "says hello"
#
# Then in src/main.rs:
#
#   #[satisfies(crate::reqs::example::greeting)]
#   fn greet() { println!("hello"); }
"#;

const STARTER_BUILD_RS: &str = r#"fn main() { rubric_core::build::check_and_warn(); }
"#;

/// Parsed init args. Kept local because init's flag set (`--all-members`,
/// repeatable `-p`) differs from the shared Flags parser.
struct InitArgs {
    all_members: bool,
    members: Vec<String>,
}

impl InitArgs {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = InitArgs { all_members: false, members: Vec::new() };
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            match a.as_str() {
                "--all-members" => out.all_members = true,
                "-p" | "--package" => {
                    i += 1;
                    let name = args.get(i).ok_or_else(|| "flag `-p` requires a member name".to_string())?;
                    out.members.push(name.clone());
                }
                other if other.starts_with("-p=") => out.members.push(other[3..].to_string()),
                other if other.starts_with("--package=") => out.members.push(other[10..].to_string()),
                other => return Err(format!("unknown flag `{}`", other)),
            }
            i += 1;
        }
        Ok(out)
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let parsed = InitArgs::parse(args)?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let cargo_toml = cwd.join("Cargo.toml");
    if !cargo_toml.is_file() {
        return Err(format!(
            "no Cargo.toml in {} — run `cargo init` or `cargo new` first",
            cwd.display(),
        ));
    }

    let cargo_src = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
    let is_workspace = cargo_src.lines().any(|l| l.trim() == "[workspace]");
    let is_package = cargo_src.lines().any(|l| l.trim() == "[package]");

    // Workspace root: require explicit opt-in to avoid surprise writes to
    // members the user didn't intend.
    if is_workspace && !is_package {
        let member_paths = discover_members(&cwd, &cargo_src)?;
        if parsed.all_members {
            return init_many(&cwd, &member_paths);
        }
        if !parsed.members.is_empty() {
            let selected = filter_members(&member_paths, &parsed.members)?;
            return init_many(&cwd, &selected);
        }
        // No flag — print help and exit non-zero.
        return Err(workspace_help(&member_paths));
    }

    // Plain crate (or a crate that is both `[package]` and `[workspace]`
    // — treat as crate).
    if !parsed.members.is_empty() || parsed.all_members {
        return Err("`-p` / `--all-members` only apply at a workspace root".to_string());
    }
    init_crate(&cwd)
}

fn workspace_help(members: &[PathBuf]) -> String {
    let mut s = String::from("this is a workspace root, not a crate — pick one:\n");
    s.push_str("  • `cargo rubric init --all-members`  — scaffold in every workspace member\n");
    s.push_str("  • `cargo rubric init -p <name>`       — scaffold in one specific member (repeatable)\n");
    s.push_str("  • `cd <member> && cargo rubric init`  — scaffold from inside a member\n");
    if !members.is_empty() {
        s.push_str("\nMembers discovered in this workspace:\n");
        for m in members {
            s.push_str(&format!("  - {}\n", m.display()));
        }
    }
    s
}

fn init_many(workspace_root: &Path, members: &[PathBuf]) -> Result<(), String> {
    let mut had_errors = false;
    for dir in members {
        status("Initializing", &format!("{}", dir.strip_prefix(workspace_root).unwrap_or(dir).display()));
        match init_crate(dir) {
            Ok(()) => {}
            Err(e) => { had_errors = true; note(&format!("skipped: {}", e)); }
        }
    }
    if had_errors {
        Err("one or more members failed — see notes above".to_string())
    } else {
        Ok(())
    }
}

fn filter_members(all: &[PathBuf], selected: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for name in selected {
        let matched = all.iter().find(|m| {
            // Match by either the member's directory name or the package
            // name declared in its Cargo.toml.
            m.file_name().and_then(|s| s.to_str()) == Some(name.as_str())
                || member_package_name(m).as_deref() == Some(name.as_str())
        });
        match matched {
            Some(p) => out.push(p.clone()),
            None => unresolved.push(name.clone()),
        }
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "unknown member(s): {}. Discovered members: {}",
            unresolved.join(", "),
            all.iter().map(|p| p.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string())
                .collect::<Vec<_>>().join(", "),
        ));
    }
    Ok(out)
}

fn member_package_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') { in_package = t == "[package]"; continue; }
        if !in_package { continue; }
        if let Some(rest) = t.strip_prefix("name") {
            let rest = rest.trim_start_matches([' ', '\t', '=']);
            if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.find('"').map(|e| &s[..e])) {
                return Some(inner.to_string());
            }
        }
    }
    None
}

/// Very small glob expansion: handles the common `"crates/*"` pattern
/// (a single trailing `*`). Non-glob strings pass through as literal
/// relative paths. Anything exotic is left to `cargo` proper.
fn discover_members(workspace_root: &Path, cargo_toml: &str) -> Result<Vec<PathBuf>, String> {
    let mut raw: Vec<String> = Vec::new();
    let mut in_workspace = false;
    let mut in_members = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_workspace = t == "[workspace]";
            in_members = false;
            continue;
        }
        if !in_workspace { continue; }
        if let Some(rest) = t.strip_prefix("members") {
            let after_eq = rest.trim_start_matches([' ', '\t', '=']);
            if after_eq.starts_with('[') { in_members = true; }
            // Consume items on this same line.
            for s in after_eq.split(&['[', ']', ',']) {
                let s = s.trim().trim_matches('"');
                if !s.is_empty() { raw.push(s.to_string()); }
            }
            if t.ends_with(']') { in_members = false; }
        } else if in_members {
            for s in t.split(&['[', ']', ',']) {
                let s = s.trim().trim_matches('"');
                if !s.is_empty() { raw.push(s.to_string()); }
            }
            if t.ends_with(']') { in_members = false; }
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    for entry in raw {
        if let Some(prefix) = entry.strip_suffix("/*") {
            let dir = workspace_root.join(prefix);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() && p.join("Cargo.toml").is_file() { out.push(p); }
                }
            }
        } else {
            let p = workspace_root.join(&entry);
            if p.join("Cargo.toml").is_file() { out.push(p); }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn init_crate(crate_root: &Path) -> Result<(), String> {
    let cargo_toml = crate_root.join("Cargo.toml");
    if !cargo_toml.is_file() {
        return Err(format!("{} is not a crate (no Cargo.toml)", crate_root.display()));
    }

    // rubric.toml
    let manifest_path = crate_root.join("rubric.toml");
    if manifest_path.exists() {
        status("Skipped", &format!("{} (already present)", rel(&manifest_path, crate_root)));
    } else {
        std::fs::write(&manifest_path, STARTER_MANIFEST)
            .map_err(|e| format!("writing {}: {}", manifest_path.display(), e))?;
        status("Creating", &rel(&manifest_path, crate_root));
    }

    // build.rs
    let build_rs = crate_root.join("build.rs");
    let mut build_rs_needs_line = false;
    if build_rs.exists() {
        let contents = std::fs::read_to_string(&build_rs).unwrap_or_default();
        if contents.contains("rubric_core::build::check_and_warn") {
            status("Skipped", &format!("{} (already wired)", rel(&build_rs, crate_root)));
        } else {
            status("Skipped", &format!("{} (present but not wired)", rel(&build_rs, crate_root)));
            build_rs_needs_line = true;
        }
    } else {
        std::fs::write(&build_rs, STARTER_BUILD_RS)
            .map_err(|e| format!("writing {}: {}", build_rs.display(), e))?;
        status("Creating", &rel(&build_rs, crate_root));
    }

    // Cargo.toml — edit in place.
    let original = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
    let mut edited = original.clone();
    let mut changed = false;
    if !has_dep(&edited, "rubric") {
        edited = upsert_dep_inline(&edited, "[dependencies]",
            r#"rubric = { package = "rubric-attr", version = "0.1" }"#);
        changed = true;
    }
    if !has_dep(&edited, "rubric-core") {
        edited = upsert_dep(&edited, "[build-dependencies]", "rubric-core", "0.1");
        changed = true;
    }
    if changed {
        std::fs::write(&cargo_toml, &edited)
            .map_err(|e| format!("writing {}: {}", cargo_toml.display(), e))?;
        status("Updating", &rel(&cargo_toml, crate_root));
    } else {
        status("Skipped", &format!("{} (rubric deps already present)", rel(&cargo_toml, crate_root)));
    }

    // lib.rs / main.rs setup!() call.
    let lib_rs = crate_root.join("src").join("lib.rs");
    let main_rs = crate_root.join("src").join("main.rs");
    let setup_target = [lib_rs, main_rs].into_iter().find(|p| p.exists());
    if let Some(p) = &setup_target {
        let original = std::fs::read_to_string(p).unwrap_or_default();
        if original.contains("rubric::setup!") {
            status("Skipped", &format!("{} (rubric::setup!() already present)", rel(p, crate_root)));
        } else {
            let edited = insert_setup_call(&original);
            std::fs::write(p, &edited)
                .map_err(|e| format!("writing {}: {}", p.display(), e))?;
            status("Updating", &rel(p, crate_root));
        }
    } else {
        note("add to src/lib.rs or src/main.rs: `rubric::setup!();`");
    }

    if build_rs_needs_line {
        note(&format!(
            "add to {}: `rubric_core::build::check_and_warn();`",
            rel(&build_rs, crate_root),
        ));
    }
    note("run `cargo rubric seal` after adding requirements and annotations");

    Ok(())
}

fn rel(p: &Path, root: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

/// True if the given dep name appears as a bare key under any
/// `[dependencies]` / `[build-dependencies]` / `[dev-dependencies]`
/// section, or as a dotted-table header like `[dependencies.<name>]`.
fn has_dep(cargo_toml: &str, name: &str) -> bool {
    let dotted_header_prefix_variants = [
        format!("[dependencies.{}", name),
        format!("[build-dependencies.{}", name),
        format!("[dev-dependencies.{}", name),
    ];
    let mut in_deps_section = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if dotted_header_prefix_variants.iter().any(|p| t.starts_with(p.as_str())) {
            return true;
        }
        if t.starts_with('[') {
            in_deps_section = matches!(t,
                "[dependencies]" | "[build-dependencies]" | "[dev-dependencies]");
            continue;
        }
        if in_deps_section {
            let key = t.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
            let key = key.trim_matches('"');
            if key == name { return true; }
        }
    }
    false
}

/// Append a pre-formatted dependency line to the given section, creating
/// the section at the end of the file if it doesn't exist.
fn upsert_dep_inline(cargo_toml: &str, section: &str, line: &str) -> String {
    upsert_dep_line(cargo_toml, section, line)
}

/// Append `name = "version"` to the given section, creating the section
/// at the end of the file if it doesn't exist. Preserves other content.
fn upsert_dep(cargo_toml: &str, section: &str, name: &str, version: &str) -> String {
    let line = format!("{} = \"{}\"", name, version);
    upsert_dep_line(cargo_toml, section, &line)
}

fn upsert_dep_line(cargo_toml: &str, section: &str, line: &str) -> String {
    let header = section.trim();

    let mut lines: Vec<&str> = cargo_toml.lines().collect();
    let mut section_start: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if l.trim() == header { section_start = Some(i); break; }
    }
    if let Some(start) = section_start {
        let mut end = lines.len();
        for (i, l) in lines.iter().enumerate().skip(start + 1) {
            if l.trim().starts_with('[') { end = i; break; }
        }
        while end > start + 1 && lines[end - 1].trim().is_empty() { end -= 1; }
        lines.insert(end, &line);
        let mut out = lines.join("\n");
        if !out.ends_with('\n') { out.push('\n'); }
        return out;
    }

    let mut out = cargo_toml.to_string();
    if !out.ends_with('\n') { out.push('\n'); }
    if !out.ends_with("\n\n") { out.push('\n'); }
    out.push_str(header);
    out.push('\n');
    out.push_str(&line);
    out.push('\n');
    out
}

/// Insert `rubric::setup!();` after the last run of leading crate-level
/// attributes (`#![...]`), blank lines, and line-comments.
fn insert_setup_call(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut insert_at = 0;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("#!") || t.starts_with("//") || t.is_empty() {
            insert_at = i + 1;
        } else {
            break;
        }
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == insert_at {
            out.push_str("rubric::setup!();\n\n");
        }
        out.push_str(line);
        out.push('\n');
    }
    if insert_at >= lines.len() {
        out.push_str("rubric::setup!();\n");
    }
    out
}
