use std::path::{Path, PathBuf};
use std::process::ExitCode;

rubric::setup!();

mod seal;
mod seal_util;
mod check;
mod cli;
mod gen;
mod init;
mod matrix;
mod term;
mod workspace;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // Cargo invokes external subcommands as `cargo-foo foo <args...>`,
    // so skip the leading "rubric" if present.
    let rest: Vec<String> = match args.get(1).map(|s| s.as_str()) {
        Some("rubric") => args.iter().skip(2).cloned().collect(),
        _ => args.iter().skip(1).cloned().collect(),
    };

    let (sub, sub_args) = match rest.split_first() {
        Some(parts) => parts,
        None => { print_usage(); return ExitCode::from(2); }
    };

    let result = match sub.as_str() {
        "init" => init::run(sub_args),
        "gen" => gen::run(sub_args),
        "check" => check::run(sub_args),
        "matrix" => matrix::run(sub_args),
        "seal" => seal::run(sub_args),
        "help" | "--help" | "-h" => { print_usage(); Ok(()) }
        other => Err(format!("unknown subcommand '{}'", other)),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => { eprintln!("error: {}", msg); ExitCode::from(1) }
    }
}

fn print_usage() {
    eprintln!("\
cargo-rubric — traceability matrix for requirements/source/tests

USAGE:
    cargo rubric <COMMAND>

COMMANDS:
    init     Scaffold rubric.toml + build.rs; print dep lines to add to Cargo.toml
    gen      Regenerate the marker module from rubric.toml
    check    Report gaps across requirements, source, and tests
    matrix   Render the traceability matrix as markdown (--output writes to file for rustdoc include)
    seal     Compute function body hashes for every annotation and write rubric.lock
");
}

/// Walk up from `start` looking for `rubric.toml`.
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join("rubric.toml");
        if candidate.is_file() { return Some(candidate); }
        cur = dir.parent();
    }
    None
}
