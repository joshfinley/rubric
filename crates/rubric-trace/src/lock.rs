//! `rubric.lock`: the machine-managed attribution chain and seals.
//!
//! Line-oriented TSV, one entry per line, four tab-separated columns:
//! requirement label, item path, origin, seal. Comments begin with `#`;
//! blank lines are ignored. Written by `accept`, read by `check`. The
//! format is the tool's own, so it doesn't ride on `toml_lite`.

/// A chain edge: one requirement cited at one item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub req_label: String,
    pub item_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seal {
    Hash { scheme: String, hex: String },
    /// Explicit opt-out (`sig_only` requirements): tracked by existence only.
    Off,
}

impl Seal {
    pub fn parse(s: &str) -> Result<Self, String> {
        if s == "off" {
            return Ok(Seal::Off);
        }
        let (scheme, hex) = s.split_once(':').ok_or_else(|| {
            format!("seal `{}` is missing a scheme prefix (expected `<scheme>:<hex>` or `off`)", s)
        })?;
        if scheme.is_empty() {
            return Err("empty scheme in seal".to_string());
        }
        if hex.is_empty() {
            return Err(format!("empty hash in seal for scheme `{}`", scheme));
        }
        Ok(Seal::Hash { scheme: scheme.to_string(), hex: hex.to_string() })
    }

    pub fn render(&self) -> String {
        match self {
            Seal::Off => "off".to_string(),
            Seal::Hash { scheme, hex } => format!("{}:{}", scheme, hex),
        }
    }
}

/// Who owns an entry. Scan owns `Annotation` entries and may add or
/// remove them as annotations come and go in source. `Declared` entries
/// mirror `rubric.toml` and are never removed by a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Annotation,
    Declared,
}

impl Origin {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "annotation" => Ok(Origin::Annotation),
            "declared" => Ok(Origin::Declared),
            other => Err(format!("unknown origin `{}` (expected `annotation` or `declared`)", other)),
        }
    }

    pub fn render(&self) -> &'static str {
        match self {
            Origin::Annotation => "annotation",
            Origin::Declared => "declared",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: Key,
    pub seal: Seal,
    pub origin: Origin,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rubric.lock line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

const HEADER: &str = "\
# rubric.lock — managed by `cargo rubric accept`. Do not edit by hand.
# Format: <requirement_label>\\t<item_path>\\t<origin>\\t<seal>
# <origin> is `annotation` or `declared`.
# <seal> is `<scheme>:<hex>` (e.g. `body:a3f2b1c8`, `stmt:...`) or `off`.

";

/// Parse `rubric.lock` source text.
pub fn parse(src: &str) -> Result<Lock, ParseError> {
    let mut entries = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let line = n + 1;
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut cols = raw.split('\t');
        let req = cols.next().filter(|s| !s.is_empty())
            .ok_or_else(|| err(line, "missing requirement label"))?;
        let item = cols.next().filter(|s| !s.is_empty())
            .ok_or_else(|| err(line, "missing item path"))?;
        let origin_raw = cols.next().ok_or_else(|| err(line, "missing origin"))?;
        let seal_raw = cols.next().ok_or_else(|| err(line, "missing seal"))?;
        if cols.next().is_some() {
            return Err(err(line, "too many tab-separated columns"));
        }
        let origin = Origin::parse(origin_raw).map_err(|e| err(line, &e))?;
        let seal = Seal::parse(seal_raw).map_err(|e| err(line, &e))?;
        entries.push(Entry {
            key: Key { req_label: req.to_string(), item_path: item.to_string() },
            seal,
            origin,
        });
    }
    Ok(Lock { entries })
}

/// Render a lock deterministically (stable entry order) so diffs are
/// reviewable and the same tree always produces the same bytes.
pub fn render(lock: &Lock) -> String {
    let mut sorted: Vec<&Entry> = lock.entries.iter().collect();
    sorted.sort_by(|a, b| {
        a.key.cmp(&b.key).then(a.origin.cmp(&b.origin))
    });
    let mut out = String::from(HEADER);
    for e in sorted {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            e.key.req_label,
            e.key.item_path,
            e.origin.render(),
            e.seal.render(),
        ));
    }
    out
}

fn err(line: usize, msg: &str) -> ParseError {
    ParseError { line, msg: msg.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(req: &str, item: &str, origin: Origin, seal: Seal) -> Entry {
        Entry { key: Key { req_label: req.into(), item_path: item.into() }, seal, origin }
    }

    #[test]
    fn round_trips_basic_entries() {
        let lock = Lock {
            entries: vec![
                entry("VOTER-1", "crate::voter::vote", Origin::Annotation,
                    Seal::Hash { scheme: "body".into(), hex: "a3f2b1c8d4e5f607".into() }),
                entry("VOTER-1", "crate::voter::tests::two_against_one", Origin::Annotation,
                    Seal::Hash { scheme: "body".into(), hex: "00112233445566ff".into() }),
                entry("VOTER-2", "external:docs/proof.pdf", Origin::Declared, Seal::Off),
            ],
        };
        let parsed = parse(&render(&lock)).unwrap();
        assert_eq!(parsed.entries.len(), 3);
        // render is sorted, so re-rendering the parse is a fixed point.
        assert_eq!(render(&lock), render(&parsed));
    }

    #[test]
    fn render_is_order_independent() {
        let a = Lock { entries: vec![
            entry("X", "i1", Origin::Annotation, Seal::Off),
            entry("Y", "i2", Origin::Declared, Seal::Off),
        ]};
        let b = Lock { entries: vec![
            entry("Y", "i2", Origin::Declared, Seal::Off),
            entry("X", "i1", Origin::Annotation, Seal::Off),
        ]};
        assert_eq!(render(&a), render(&b));
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        let src = "\n# comment\n\nVOTER-1\tcrate::a\tannotation\tbody:1\n";
        assert_eq!(parse(src).unwrap().entries.len(), 1);
    }

    #[test]
    fn rejects_missing_columns() {
        assert!(parse("only_one\n").is_err());
        assert!(parse("one\ttwo\n").is_err());
        assert!(parse("one\ttwo\tannotation\n").is_err());
    }

    #[test]
    fn rejects_too_many_columns() {
        assert!(parse("a\tb\tannotation\tbody:1\textra\n").is_err());
    }

    #[test]
    fn rejects_bad_origin() {
        assert!(parse("a\tb\tguess\tbody:1\n").is_err());
    }

    #[test]
    fn rejects_malformed_seal() {
        assert!(parse("a\tb\tannotation\tnoscheme\n").is_err());
        assert!(parse("a\tb\tannotation\t:nohex\n").is_err());
    }

    #[test]
    fn parses_off_seal() {
        let lock = parse("a\tb\tdeclared\toff\n").unwrap();
        assert_eq!(lock.entries[0].seal, Seal::Off);
        assert_eq!(lock.entries[0].origin, Origin::Declared);
    }
}
