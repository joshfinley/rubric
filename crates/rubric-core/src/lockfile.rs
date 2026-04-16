//! `rubric.lock` — committed tool-managed seal state.
//!
//! Line-oriented TSV: one entry per line, three tab-separated columns:
//! requirement label, source item path, seal value. Comments begin with
//! `#`; blank lines are ignored. Written and read only by `cargo rubric`;
//! format expressiveness isn't a concern, so we don't ride on toml_lite.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seal {
    /// `body:<hex>` (or future scheme prefixes). Hex is the hash for the
    /// chosen scheme, computed over the annotated item's tokens.
    Hash { scheme: String, hex: String },
    /// Explicit opt-out. Macro surfaces a single audit note so reviewers
    /// see the suppression in compiler output.
    Off,
}

impl Seal {
    pub fn parse(s: &str) -> Result<Self, String> {
        if s == "off" { return Ok(Seal::Off); }
        let (scheme, hex) = s.split_once(':')
            .ok_or_else(|| format!("seal value `{}` is missing a scheme prefix (expected `<scheme>:<hex>` or `off`)", s))?;
        if scheme.is_empty() { return Err("empty scheme in seal value".to_string()); }
        if hex.is_empty() { return Err(format!("empty hash in seal value for scheme `{}`", scheme)); }
        Ok(Seal::Hash { scheme: scheme.to_string(), hex: hex.to_string() })
    }
    pub fn render(&self) -> String {
        match self {
            Seal::Off => "off".to_string(),
            Seal::Hash { scheme, hex } => format!("{}:{}", scheme, hex),
        }
    }
}

/// Keyed by (requirement label, item path) — a single fn can satisfy
/// multiple requirements, and a single requirement can be satisfied at
/// multiple sites, so both components are needed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub req_label: String,
    pub item_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct Lockfile {
    pub entries: BTreeMap<Key, Seal>,
}

impl Lockfile {
    pub fn new() -> Self { Self::default() }

    // satisfies: lockfile::parse_and_write
    pub fn parse(src: &str) -> Result<Self, String> {
        let mut lock = Lockfile::new();
        for (n, line) in src.lines().enumerate() {
            let line_no = n + 1;
            let line = line.trim_end_matches(['\r', '\n']);
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
            let mut parts = line.split('\t');
            let req = parts.next().ok_or_else(|| format!("line {}: missing requirement label", line_no))?;
            let item = parts.next().ok_or_else(|| format!("line {}: missing item path", line_no))?;
            let seal_raw = parts.next().ok_or_else(|| format!("line {}: missing seal value", line_no))?;
            if parts.next().is_some() {
                return Err(format!("line {}: too many tab-separated columns", line_no));
            }
            let seal = Seal::parse(seal_raw).map_err(|e| format!("line {}: {}", line_no, e))?;
            lock.entries.insert(Key { req_label: req.to_string(), item_path: item.to_string() }, seal);
        }
        Ok(lock)
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "# rubric.lock — managed by `cargo rubric seal`. Do not edit by hand.\n\
             # Format: <requirement_label>\\t<item_path>\\t<seal_value>\n\
             # <seal_value> is either `<scheme>:<hex>` (e.g. `body:a3f2b1c8`) or `off`.\n\n",
        );
        for (key, seal) in &self.entries {
            out.push_str(&format!("{}\t{}\t{}\n", key.req_label, key.item_path, seal.render()));
        }
        out
    }

    pub fn get(&self, req_label: &str, item_path: &str) -> Option<&Seal> {
        self.entries.get(&Key {
            req_label: req_label.to_string(),
            item_path: item_path.to_string(),
        })
    }

    pub fn set(&mut self, req_label: String, item_path: String, seal: Seal) {
        self.entries.insert(Key { req_label, item_path }, seal);
    }

    /// Drop entries whose requirement label is not in `known_labels`.
    /// Called by `cargo rubric seal` after manifest reconciliation.
    pub fn prune(&mut self, known_labels: &std::collections::BTreeSet<String>) {
        self.entries.retain(|key, _| known_labels.contains(&key.req_label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn label(s: &str) -> String { s.to_string() }

    #[test]
    #[rubric::verifies(crate::reqs::lockfile::parse_and_write)]
    fn round_trips_basic_entries() {
        let mut lock = Lockfile::new();
        lock.set("parser::hm".into(), "crate::parser::check_magic".into(),
            Seal::Hash { scheme: "body".into(), hex: "a3f2b1c8".into() });
        lock.set("parser::rt".into(), "crate::parser::parse".into(), Seal::Off);
        let rendered = lock.render();
        let parsed = Lockfile::parse(&rendered).unwrap();
        assert_eq!(lock.entries, parsed.entries);
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        let src = "\n# comment\n\n# another\nparser::a\tcrate::a\tbody:1\n";
        let lock = Lockfile::parse(src).unwrap();
        assert_eq!(lock.entries.len(), 1);
    }

    #[test]
    fn rejects_missing_columns() {
        assert!(Lockfile::parse("only_one_column\n").is_err());
        assert!(Lockfile::parse("one\ttwo\n").is_err());
    }

    #[test]
    fn rejects_malformed_seal() {
        assert!(Lockfile::parse("a\tb\tnoscheme\n").is_err());
        assert!(Lockfile::parse("a\tb\t:nohex\n").is_err());
    }

    #[test]
    fn prune_drops_unknown_labels() {
        let mut lock = Lockfile::new();
        lock.set("keep".into(), "i1".into(), Seal::Off);
        lock.set("drop".into(), "i2".into(), Seal::Off);
        let mut known = BTreeSet::new();
        known.insert(label("keep"));
        lock.prune(&known);
        assert_eq!(lock.entries.len(), 1);
        assert!(lock.entries.keys().any(|k| k.req_label == "keep"));
    }

    #[test]
    fn render_is_deterministic() {
        let mut a = Lockfile::new();
        let mut b = Lockfile::new();
        a.set("x::a".into(), "i1".into(), Seal::Off);
        a.set("x::b".into(), "i2".into(), Seal::Off);
        b.set("x::b".into(), "i2".into(), Seal::Off);
        b.set("x::a".into(), "i1".into(), Seal::Off);
        assert_eq!(a.render(), b.render());
    }
}
