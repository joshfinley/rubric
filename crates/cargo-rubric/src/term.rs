//! Cargo-styled terminal output: right-aligned verbs in bold green,
//! `note:` in bold cyan, `warning:` in bold yellow, `error:` in bold red.
//! Honors CARGO_TERM_COLOR and NO_COLOR; auto-detects TTY otherwise.

pub fn status(verb: &str, target: &str) {
    if color() {
        println!("\x1b[1;32m{:>12}\x1b[0m {}", verb, target);
    } else {
        println!("{:>12} {}", verb, target);
    }
}

pub fn note(msg: &str) {
    if color() {
        println!("\x1b[1;36mnote:\x1b[0m {}", msg);
    } else {
        println!("note: {}", msg);
    }
}

pub fn warning(msg: &str) {
    if color() {
        eprintln!("\x1b[1;33mwarning:\x1b[0m {}", msg);
    } else {
        eprintln!("warning: {}", msg);
    }
}

pub fn error(msg: &str) {
    if color() {
        eprintln!("\x1b[1;31merror:\x1b[0m {}", msg);
    } else {
        eprintln!("error: {}", msg);
    }
}

pub fn help(msg: &str) {
    if color() {
        eprintln!("\x1b[1;34mhelp:\x1b[0m {}", msg);
    } else {
        eprintln!("help: {}", msg);
    }
}

pub fn color() -> bool {
    match std::env::var("CARGO_TERM_COLOR").as_deref() {
        Ok("never") => return false,
        Ok("always") => return true,
        _ => {}
    }
    if std::env::var_os("NO_COLOR").is_some() { return false; }
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}
