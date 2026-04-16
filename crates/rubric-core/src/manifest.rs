//! Manifest types and builder that consumes parsed TOML entries.
//!
//! A requirement lives at `[req.<a>.<b>.<leaf>]`; its label path is
//! `[a, b, leaf]` and its stable id is FNV-1a of the path joined by `::`.

use crate::fnv::fnv1a_64;
use crate::toml_lite::{self, Entry, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Satisfies,
    Verifies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReqId(pub u64);

impl ReqId {
    pub fn from_label_path(parts: &[String]) -> Self {
        let joined = parts.join("::");
        ReqId(fnv1a_64(joined.as_bytes()))
    }
}

impl std::fmt::Display for ReqId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct Requirement {
    pub label_path: Vec<String>,
    pub description: String,
    pub satisfied_by: Vec<String>,
    pub verified_by: Vec<String>,
    pub doc: Option<String>,
}

impl Requirement {
    pub fn id(&self) -> ReqId {
        ReqId::from_label_path(&self.label_path)
    }

    pub fn label(&self) -> String {
        self.label_path.join("::")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub version: u32,
    /// When true, every `build.rs` check elevates drift findings to build
    /// failures via non-zero exit. When false (default), release-profile
    /// builds still elevate but dev builds warn-only.
    pub strict: bool,
    pub requirements: Vec<Requirement>,
}

#[derive(Debug)]
pub struct ManifestError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "manifest error at line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ManifestError {}

impl From<toml_lite::ParseError> for ManifestError {
    fn from(e: toml_lite::ParseError) -> Self {
        ManifestError { line: e.line, msg: e.msg }
    }
}

impl Manifest {
    // satisfies: manifest::builds_from_entries, manifest::preserves_label_path
    pub fn parse(src: &str) -> Result<Self, ManifestError> {
        let entries = toml_lite::parse(src)?;
        Self::from_entries(&entries)
    }

    fn from_entries(entries: &[Entry]) -> Result<Self, ManifestError> {
        let mut manifest = Manifest::default();
        // Group entries by section. Vec of (section, [entries]).
        // Order is preserved; sections may appear once each in practice.
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

        for (section, items) in &groups {
            if section.is_empty() {
                return Err(err(items[0].line, "top-level keys are not supported"));
            }
            match section[0].as_str() {
                "meta" => apply_meta(&mut manifest, &section[1..], items)?,
                "req" => {
                    let label_path = &section[1..];
                    if label_path.is_empty() {
                        return Err(err(items[0].line, "[req] needs a label path, e.g. [req.parser.header_magic]"));
                    }
                    let req = build_requirement(label_path.to_vec(), items)?;
                    manifest.requirements.push(req);
                }
                other => return Err(err(items[0].line, &format!("unknown top-level section '{}'", other))),
            }
        }
        Ok(manifest)
    }
}

fn apply_meta(m: &mut Manifest, sub: &[String], items: &[&Entry]) -> Result<(), ManifestError> {
    if !sub.is_empty() {
        return Err(err(items[0].line, "[meta] takes no sub-sections"));
    }
    for e in items {
        match (e.key.as_str(), &e.value) {
            ("version", Value::Integer(n)) => {
                if *n < 0 || *n > u32::MAX as i64 {
                    return Err(err(e.line, "version out of range"));
                }
                m.version = *n as u32;
            }
            ("strict", Value::Integer(n)) => m.strict = *n != 0,
            (k, _) => return Err(err(e.line, &format!("unknown meta key '{}'", k))),
        }
    }
    Ok(())
}

// satisfies: manifest::requires_description
fn build_requirement(label_path: Vec<String>, items: &[&Entry]) -> Result<Requirement, ManifestError> {
    let mut req = Requirement {
        label_path,
        description: String::new(),
        satisfied_by: Vec::new(),
        verified_by: Vec::new(),
        doc: None,
    };
    for e in items {
        match (e.key.as_str(), &e.value) {
            ("description", Value::String(s)) => req.description = s.clone(),
            ("satisfied_by", Value::StringArray(xs)) => req.satisfied_by = xs.clone(),
            ("verified_by", Value::StringArray(xs)) => req.verified_by = xs.clone(),
            ("doc", Value::String(s)) => req.doc = Some(s.clone()),
            (k, _) => return Err(err(e.line, &format!("unknown or wrongly-typed key '{}'", k))),
        }
    }
    if req.description.is_empty() {
        return Err(err(items[0].line, "requirement is missing a description"));
    }
    Ok(req)
}

fn err(line: usize, msg: &str) -> ManifestError {
    ManifestError { line, msg: msg.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[meta]
version = 1

[req.parser.header_magic]
description  = "Parser rejects files without the magic header"
satisfied_by = ["crate::parser::check_magic"]
verified_by  = ["crate::parser::tests::rejects_bad_magic"]

[req.parser.rejects_truncated]
description  = "Parser returns Truncated on short input"
satisfied_by = ["crate::parser::parse"]
verified_by  = ["crate::parser::tests::truncated_input"]
doc          = "requirements/parser/rejects_truncated.md"
"#;

    #[test]
    #[rubric::verifies(crate::reqs::manifest::builds_from_entries)]
    #[rubric::verifies(crate::reqs::manifest::preserves_label_path)]
    fn parses_sample() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(m.requirements.len(), 2);
        assert_eq!(m.requirements[0].label_path, vec!["parser", "header_magic"]);
        assert_eq!(m.requirements[1].doc.as_deref(), Some("requirements/parser/rejects_truncated.md"));
    }

    #[test]
    fn id_is_stable() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let id = m.requirements[0].id();
        // Recompute manually.
        assert_eq!(id.0, fnv1a_64(b"parser::header_magic"));
    }

    #[test]
    #[rubric::verifies(crate::reqs::manifest::requires_description)]
    fn missing_description_errors() {
        let src = "[req.x.y]\nsatisfied_by = []\nverified_by = []\n";
        assert!(Manifest::parse(src).is_err());
    }
}
