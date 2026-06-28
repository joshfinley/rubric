//! `cargo rubric`: standalone traceability oracle and chain editor.
//!
//! Verification logic lives in `rubric-trace`. This binary is the
//! I/O shell plus the source scanner.

mod accept_cmd;
mod attest_cmd;
mod audit_cmd;
mod check_cmd;
mod git_history;
mod init_cmd;
mod log_cmd;
mod members;
mod project;
mod scan;
mod trace_cmd;
mod workspace;

use std::process::ExitCode;

const USAGE: &str = "\
cargo rubric <command>

Commands:
  init    Scaffold rubric.toml in a crate or workspace member
  check   Read-only oracle verdict; non-zero exit on any finding (CI)
  accept  Scan annotations + re-seal the chain; prints what changed
  attest  Record the attestation root for reconcile requirements
  trace   Render the traceability matrix as markdown
  log     Seal history from the git history of rubric.lock
  audit   Flag commits that re-sealed a reconcile chain without attesting
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    // Invoked as `cargo rubric <cmd>`, argv[1] is "rubric".
    let mut cmd = args.next();
    if cmd.as_deref() == Some("rubric") {
        cmd = args.next();
    }
    let rest: Vec<String> = args.collect();

    match cmd.as_deref() {
        Some("check") => check_cmd::run(),
        Some("accept") => accept_cmd::run(),
        Some("attest") => attest_cmd::run(),
        Some("trace") => trace_cmd::run(),
        Some("log") => log_cmd::run(),
        Some("audit") => audit_cmd::run(),
        Some("init") => init_cmd::run(&rest),
        _ => {
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}
