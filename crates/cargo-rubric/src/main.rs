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
          (audit [<since>] scopes the walk to <since>..HEAD)
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    // Invoked as `cargo rubric <cmd>`, argv[1] is "rubric".
    let mut cmd = args.next();
    if cmd.as_deref() == Some("rubric") {
        cmd = args.next();
    }
    let rest: Vec<String> = args.collect();

    // No arg-parsing library, so handle `--help` by hand before dispatch. A
    // help flag must never run a command.
    match route(cmd.as_deref(), &rest) {
        Route::Help(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Route::Usage => {
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
        Route::Run("check") => check_cmd::run(),
        Route::Run("accept") => accept_cmd::run(),
        Route::Run("attest") => attest_cmd::run(),
        Route::Run("trace") => trace_cmd::run(),
        Route::Run("log") => log_cmd::run(),
        Route::Run("audit") => audit_cmd::run(&rest),
        Route::Run("init") => init_cmd::run(&rest),
        // `route` only returns `Run` for the commands listed above.
        Route::Run(_) => {
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// What `main` should do with a parsed `(command, rest)` pair.
#[derive(Debug, PartialEq, Eq)]
enum Route<'a> {
    /// Print this text to stdout and exit 0.
    Help(&'a str),
    /// Print top-level usage to stderr and exit non-zero (no or unknown command).
    Usage,
    /// Run this known subcommand.
    Run(&'a str),
}

/// Pick the route. A `-h`/`--help` anywhere outranks running the command. An
/// unknown command is `Usage`.
fn route<'a>(cmd: Option<&'a str>, rest: &[String]) -> Route<'a> {
    let is_help = |s: &str| matches!(s, "-h" | "--help");
    match cmd {
        None => Route::Usage,
        Some(c) if is_help(c) || c == "help" => Route::Help(USAGE),
        Some(c) => match help_for(c) {
            Some(text) if rest.iter().any(|a| is_help(a)) => Route::Help(text),
            Some(_) => Route::Run(c),
            None => Route::Usage,
        },
    }
}

/// Per-command help text, and the set of known commands (`None` if unknown).
fn help_for(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "check" => "cargo rubric check\n\n  Read-only oracle verdict. Non-zero exit on any finding (CI).\n",
        "accept" => "cargo rubric accept\n\n  Scan annotations and re-seal the chain. Prints what changed.\n",
        "attest" => "cargo rubric attest\n\n  Record the attestation root for reconcile requirements.\n",
        "trace" => "cargo rubric trace\n\n  Render the traceability matrix as markdown.\n",
        "log" => "cargo rubric log\n\n  Show seal history from the git history of rubric.lock.\n",
        "audit" => "cargo rubric audit [<since>]\n\n  Flag commits that re-sealed a reconcile chain without attesting.\n  With <since>, only commits in <since>..HEAD are audited.\n",
        "init" => "cargo rubric init [--from-annotations]\n\n  Scaffold rubric.toml. With --from-annotations, draft a stub for every\n  label already cited by a // satisfies: / // verifies: marker.\n",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn help_flag_never_runs_a_subcommand() {
        // The bug: `accept --help` used to fall through and run the accept.
        for c in ["check", "accept", "attest", "trace", "log", "audit", "init"] {
            for flag in ["--help", "-h"] {
                let r = route(Some(c), &s(&[flag]));
                assert!(matches!(r, Route::Help(_)), "{c} {flag} routed to {r:?}");
                assert!(!matches!(r, Route::Run(_)), "{c} {flag} would run the command");
            }
        }
    }

    #[test]
    fn bare_subcommand_runs() {
        assert_eq!(route(Some("accept"), &[]), Route::Run("accept"));
        assert_eq!(route(Some("audit"), &s(&["main"])), Route::Run("audit"));
        assert_eq!(route(Some("init"), &s(&["--from-annotations"])), Route::Run("init"));
    }

    #[test]
    fn help_flag_after_real_args_still_wins() {
        assert!(matches!(route(Some("audit"), &s(&["main", "--help"])), Route::Help(_)));
    }

    #[test]
    fn top_level_help_is_usage_text() {
        assert_eq!(route(Some("--help"), &[]), Route::Help(USAGE));
        assert_eq!(route(Some("-h"), &[]), Route::Help(USAGE));
        assert_eq!(route(Some("help"), &[]), Route::Help(USAGE));
    }

    #[test]
    fn no_command_is_usage() {
        assert_eq!(route(None, &[]), Route::Usage);
    }

    #[test]
    fn unknown_command_is_usage_even_with_help() {
        assert_eq!(route(Some("bogus"), &[]), Route::Usage);
        assert_eq!(route(Some("bogus"), &s(&["--help"])), Route::Usage);
    }
}
