//! `rubric.toml`: the human-authored contract.

use crate::pointcut::{self, Pointcut};
use crate::toml_lite::{self, Entry, Value};

/// Requirement kind. Functional requirements have a satisfying function;
/// invariants are verified only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Functional,
    Invariant,
}

/// How much of a cited item a seal covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SealMode {
    /// Existence only, no content hash (the former `sig_only`).
    Off,
    /// The item's block body (the default).
    #[default]
    Body,
    /// The item's signature (visibility, name, params, return).
    Signature,
    /// Signature and body together. A change to either breaks the seal.
    Full,
}

/// One `[req.<LABEL>]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// Bare requirement ID, e.g. `VOTER-1`.
    pub label: String,
    pub kind: Kind,
    /// The statement text. Sealed (`stmt:` scheme) so rewording breaks the chain.
    pub statement: String,
    /// How much of each cited item to seal. `Off` tracks existence only,
    /// for projects with low tolerance for token-hash false positives.
    pub seal: SealMode,
    /// Optional pointcut. When set, matching scanned items are bound as
    /// satisfiers of this requirement. A matched item with no seal yet
    /// is reported as uncovered.
    pub cover: Option<Pointcut>,
    /// Declared satisfier paths the scanner can't reach (integration
    /// tests, bin-only crates, `external:` evidence).
    pub satisfied_by: Vec<String>,
    /// Declared verifier paths, same role as `satisfied_by`.
    pub verified_by: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub requirements: Vec<Requirement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rubric.toml line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

impl From<toml_lite::ParseError> for ParseError {
    fn from(e: toml_lite::ParseError) -> Self {
        ParseError { line: e.line, msg: e.msg }
    }
}

/// Parse `rubric.toml` source text.
pub fn parse(src: &str) -> Result<Manifest, ParseError> {
    let entries = toml_lite::parse(src)?;
    from_entries(&entries)
}

fn from_entries(entries: &[Entry]) -> Result<Manifest, ParseError> {
    // Group consecutive entries that share a section header.
    let mut groups: Vec<(Vec<String>, Vec<&Entry>)> = Vec::new();
    for entry in entries {
        if let Some(last) = groups.last_mut() {
            if last.0 == entry.section {
                last.1.push(entry);
                continue;
            }
        }
        groups.push((entry.section.clone(), vec![entry]));
    }

    let mut manifest = Manifest::default();
    for (section, items) in &groups {
        let line = items.first().map(|e| e.line).unwrap_or(0);
        if section.is_empty() {
            return Err(err(line, "top-level keys are not supported; use [req.<LABEL>]"));
        }
        match section[0].as_str() {
            "req" => {
                let label = req_label(&section[1..], line)?;
                if manifest.requirements.iter().any(|r| r.label == label) {
                    return Err(err(line, &format!("duplicate requirement '{}'", label)));
                }
                manifest.requirements.push(build_requirement(label, items)?);
            }
            other => return Err(err(line, &format!("unknown top-level section '{}'; expected [req.<LABEL>]", other))),
        }
    }
    Ok(manifest)
}

/// A label is a single bare segment after `req`. Paths are not labels.
fn req_label(rest: &[String], line: usize) -> Result<String, ParseError> {
    match rest {
        [label] => Ok(label.clone()),
        [] => Err(err(line, "[req] needs a label, e.g. [req.VOTER-1]")),
        _ => Err(err(line, &format!(
            "requirement label must be a single bare ID; '{}' looks like a path",
            rest.join("."),
        ))),
    }
}

fn build_requirement(label: String, items: &[&Entry]) -> Result<Requirement, ParseError> {
    let line = items.first().map(|e| e.line).unwrap_or(0);
    let mut kind: Option<Kind> = None;
    let mut statement = String::new();
    let mut seal: Option<SealMode> = None;
    let mut cover: Option<Pointcut> = None;
    let mut satisfied_by = Vec::new();
    let mut verified_by = Vec::new();

    for e in items {
        match (e.key.as_str(), &e.value) {
            ("kind", Value::String(s)) => kind = Some(parse_kind(s, e.line)?),
            ("statement", Value::String(s)) => statement = s.clone(),
            ("seal", Value::String(s)) => seal = Some(parse_seal_mode(s, e.line)?),
            ("cover", Value::String(s)) => {
                cover = Some(pointcut::parse(s).map_err(|m| err(e.line, &m))?)
            }
            // Back-compat: `sig_only = true` is the former spelling of `seal = "off"`.
            ("sig_only", Value::Boolean(b)) => {
                if *b {
                    seal = Some(SealMode::Off);
                }
            }
            ("satisfied_by", Value::StringArray(xs)) => satisfied_by = xs.clone(),
            ("verified_by", Value::StringArray(xs)) => verified_by = xs.clone(),
            (k, _) => return Err(err(e.line, &format!("unknown or wrongly-typed key '{}' in [req.{}]", k, label))),
        }
    }

    let kind = kind.ok_or_else(|| err(line, &format!("[req.{}] is missing `kind` (\"functional\" or \"invariant\")", label)))?;
    if statement.is_empty() {
        return Err(err(line, &format!("[req.{}] is missing `statement`", label)));
    }
    let seal = seal.unwrap_or_default();
    Ok(Requirement { label, kind, statement, seal, cover, satisfied_by, verified_by })
}

fn parse_kind(s: &str, line: usize) -> Result<Kind, ParseError> {
    match s {
        "functional" => Ok(Kind::Functional),
        "invariant" => Ok(Kind::Invariant),
        other => Err(err(line, &format!("unknown kind '{}'; expected \"functional\" or \"invariant\"", other))),
    }
}

fn parse_seal_mode(s: &str, line: usize) -> Result<SealMode, ParseError> {
    match s {
        "off" => Ok(SealMode::Off),
        "body" => Ok(SealMode::Body),
        "signature" => Ok(SealMode::Signature),
        "full" => Ok(SealMode::Full),
        other => Err(err(line, &format!(
            "unknown seal '{}'; expected \"off\", \"body\", \"signature\", or \"full\"",
            other
        ))),
    }
}

fn err(line: usize, msg: &str) -> ParseError {
    ParseError { line, msg: msg.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[req.VOTER-1]
kind = "functional"
statement = "Two identical inputs always out-vote the third"
satisfied_by = ["crate::voter::vote"]
verified_by = ["crate::voter::tests::two_against_one"]

[req.VOTER-2]
kind = "invariant"
statement = "The voter is pure and side-effect free"
sig_only = true
"#;

    #[test]
    fn parses_sample() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.requirements.len(), 2);
        let r0 = &m.requirements[0];
        assert_eq!(r0.label, "VOTER-1");
        assert_eq!(r0.kind, Kind::Functional);
        assert_eq!(r0.statement, "Two identical inputs always out-vote the third");
        assert_eq!(r0.seal, SealMode::Body);
        assert_eq!(r0.satisfied_by, vec!["crate::voter::vote"]);
        let r1 = &m.requirements[1];
        assert_eq!(r1.label, "VOTER-2");
        assert_eq!(r1.kind, Kind::Invariant);
        assert_eq!(r1.seal, SealMode::Off);
        assert!(r1.satisfied_by.is_empty());
    }

    #[test]
    fn rejects_path_label() {
        let src = "[req.parser.magic]\nkind = \"functional\"\nstatement = \"x\"\n";
        let e = parse(src).unwrap_err();
        assert!(e.msg.contains("single bare ID"), "{}", e.msg);
    }

    #[test]
    fn requires_kind() {
        let src = "[req.X]\nstatement = \"x\"\n";
        assert!(parse(src).unwrap_err().msg.contains("kind"));
    }

    #[test]
    fn requires_statement() {
        let src = "[req.X]\nkind = \"functional\"\n";
        assert!(parse(src).unwrap_err().msg.contains("statement"));
    }

    #[test]
    fn rejects_unknown_kind() {
        let src = "[req.X]\nkind = \"performance\"\nstatement = \"x\"\n";
        assert!(parse(src).unwrap_err().msg.contains("unknown kind"));
    }

    #[test]
    fn rejects_duplicate_label() {
        let src = "[req.X]\nkind=\"functional\"\nstatement=\"a\"\n[req.Y]\nkind=\"functional\"\nstatement=\"b\"\n[req.X]\nkind=\"functional\"\nstatement=\"c\"\n";
        assert!(parse(src).unwrap_err().msg.contains("duplicate"));
    }

    #[test]
    fn rejects_unknown_section() {
        let src = "[meta]\nversion = 1\n";
        assert!(parse(src).unwrap_err().msg.contains("unknown top-level section"));
    }

    #[test]
    fn parses_seal_modes() {
        let src = "[req.A]\nkind=\"functional\"\nstatement=\"a\"\nseal=\"full\"\n\
                   [req.B]\nkind=\"functional\"\nstatement=\"b\"\nseal=\"signature\"\n\
                   [req.C]\nkind=\"functional\"\nstatement=\"c\"\n";
        let m = parse(src).unwrap();
        assert_eq!(m.requirements[0].seal, SealMode::Full);
        assert_eq!(m.requirements[1].seal, SealMode::Signature);
        assert_eq!(m.requirements[2].seal, SealMode::Body); // default
    }

    #[test]
    fn sig_only_true_maps_to_off() {
        let src = "[req.X]\nkind=\"invariant\"\nstatement=\"x\"\nsig_only = true\n";
        assert_eq!(parse(src).unwrap().requirements[0].seal, SealMode::Off);
    }

    #[test]
    fn rejects_unknown_seal_mode() {
        let src = "[req.X]\nkind=\"functional\"\nstatement=\"x\"\nseal=\"partial\"\n";
        assert!(parse(src).unwrap_err().msg.contains("unknown seal"));
    }

    #[test]
    fn parses_cover_pointcut() {
        let src = "[req.X]\nkind=\"invariant\"\nstatement=\"s\"\ncover=\"pub fn within crate::api\"\n";
        assert!(parse(src).unwrap().requirements[0].cover.is_some());
    }

    #[test]
    fn rejects_malformed_cover() {
        let src = "[req.X]\nkind=\"invariant\"\nstatement=\"s\"\ncover=\"pub fn crate::api\"\n";
        assert!(parse(src).unwrap_err().msg.contains("within"));
    }
}
