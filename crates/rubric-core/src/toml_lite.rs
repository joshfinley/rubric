//! Minimal TOML subset parser.
//!
//! Supports the slice we need for `rubric.toml`: comments, dotted section
//! headers, bare keys, integer/basic-string/string-array values. No
//! multi-line strings, raw strings, inline tables, or array-of-tables —
//! the manifest doesn't use them.

#[derive(Debug, Clone)]
pub enum Value {
    Integer(i64),
    String(String),
    StringArray(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub section: Vec<String>,
    pub key: String,
    pub value: Value,
    pub line: usize,
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

// satisfies: toml_lite::parses_section_headers, toml_lite::parses_basic_strings, toml_lite::parses_string_arrays, toml_lite::parses_integers, toml_lite::rejects_unterminated_strings
pub fn parse(input: &str) -> Result<Vec<Entry>, ParseError> {
    let mut p = Parser { src: input.as_bytes(), pos: 0, line: 1 };
    let mut out = Vec::new();
    let mut section: Vec<String> = Vec::new();

    loop {
        p.skip_blank();
        match p.peek() {
            None => break,
            Some(b'[') => {
                section = p.parse_section_header()?;
                p.skip_to_eol_after_value()?;
            }
            Some(c) if is_bare_start(c) => {
                let line = p.line;
                let key_path = p.parse_dotted_key()?;
                p.skip_inline_ws();
                p.expect(b'=')?;
                p.skip_inline_ws();
                let value = p.parse_value()?;
                p.skip_to_eol_after_value()?;

                // dotted key on a value: prepend any intermediate parts
                // to the current section (e.g. `a.b = 1` under `[x]`
                // becomes section=["x","a"], key="b").
                let (final_key, extra) = key_path.split_last_owned();
                let mut full_section = section.clone();
                full_section.extend(extra);
                out.push(Entry { section: full_section, key: final_key, value, line });
            }
            Some(_) => return Err(p.err("expected key or section header")),
        }
    }
    Ok(out)
}

trait SplitLastOwned {
    fn split_last_owned(self) -> (String, Vec<String>);
}

impl SplitLastOwned for Vec<String> {
    fn split_last_owned(mut self) -> (String, Vec<String>) {
        let last = self.pop().expect("dotted key must have at least one part");
        (last, self)
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
}

fn is_bare_start(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

fn is_bare_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn err(&self, msg: &str) -> ParseError {
        ParseError { line: self.line, msg: msg.to_string() }
    }

    fn expect(&mut self, c: u8) -> Result<(), ParseError> {
        match self.peek() {
            Some(x) if x == c => { self.bump(); Ok(()) }
            Some(x) => Err(self.err(&format!("expected '{}', found '{}'", c as char, x as char))),
            None => Err(self.err(&format!("expected '{}', found EOF", c as char))),
        }
    }

    fn skip_inline_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c == b' ' || c == b'\t' { self.bump(); } else { break; }
        }
    }

    /// Skip whitespace, newlines, and full-line comments.
    fn skip_blank(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => { self.bump(); }
                Some(b'#') => { while let Some(c) = self.peek() { if c == b'\n' { break; } self.bump(); } }
                _ => break,
            }
        }
    }

    /// After a value, allow trailing inline whitespace, an optional comment,
    /// then a newline or EOF.
    fn skip_to_eol_after_value(&mut self) -> Result<(), ParseError> {
        self.skip_inline_ws();
        if let Some(b'#') = self.peek() {
            while let Some(c) = self.peek() { if c == b'\n' { break; } self.bump(); }
        }
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => { self.bump(); Ok(()) }
            Some(b'\r') => { self.bump(); if let Some(b'\n') = self.peek() { self.bump(); } Ok(()) }
            Some(c) => Err(self.err(&format!("expected newline after value, found '{}'", c as char))),
        }
    }

    fn parse_bare_key(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        match self.peek() {
            Some(c) if is_bare_start(c) => { self.bump(); }
            _ => return Err(self.err("expected key")),
        }
        while let Some(c) = self.peek() { if is_bare_cont(c) { self.bump(); } else { break; } }
        Ok(std::str::from_utf8(&self.src[start..self.pos]).unwrap().to_string())
    }

    fn parse_dotted_key(&mut self) -> Result<Vec<String>, ParseError> {
        let mut parts = vec![self.parse_bare_key()?];
        loop {
            self.skip_inline_ws();
            if self.peek() == Some(b'.') {
                self.bump();
                self.skip_inline_ws();
                parts.push(self.parse_bare_key()?);
            } else {
                break;
            }
        }
        Ok(parts)
    }

    fn parse_section_header(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(b'[')?;
        self.skip_inline_ws();
        let parts = self.parse_dotted_key()?;
        self.skip_inline_ws();
        self.expect(b']')?;
        Ok(parts)
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b'[') => Ok(Value::StringArray(self.parse_string_array()?)),
            Some(c) if c == b'-' || c.is_ascii_digit() => Ok(Value::Integer(self.parse_integer()?)),
            Some(c) => Err(self.err(&format!("unsupported value starting with '{}'", c as char))),
            None => Err(self.err("expected value, found EOF")),
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => return Ok(out),
                Some(b'\\') => match self.bump() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(b'r') => out.push('\r'),
                    Some(b'0') => out.push('\0'),
                    Some(c) => return Err(self.err(&format!("unknown escape \\{}", c as char))),
                    None => return Err(self.err("unterminated escape")),
                },
                Some(b'\n') => return Err(self.err("newline in string")),
                Some(c) => {
                    // accept multi-byte UTF-8 by buffering raw bytes; we
                    // know the source slice is valid UTF-8.
                    out.push(c as char);
                    if c >= 0x80 {
                        // back up and copy the full code point
                        out.pop();
                        let start = self.pos - 1;
                        let len = utf8_len(c);
                        for _ in 1..len { self.bump(); }
                        let s = std::str::from_utf8(&self.src[start..start + len])
                            .map_err(|_| self.err("invalid utf-8 in string"))?;
                        out.push_str(s);
                    }
                }
            }
        }
    }

    fn parse_integer(&mut self) -> Result<i64, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') { self.bump(); }
        let digit_start = self.pos;
        while let Some(c) = self.peek() { if c.is_ascii_digit() || c == b'_' { self.bump(); } else { break; } }
        if self.pos == digit_start { return Err(self.err("expected digit")); }
        let raw: String = self.src[start..self.pos].iter()
            .filter(|&&b| b != b'_')
            .map(|&b| b as char)
            .collect();
        raw.parse::<i64>().map_err(|_| self.err("invalid integer"))
    }

    fn parse_string_array(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(b'[')?;
        let mut out = Vec::new();
        loop {
            self.skip_blank();
            if self.peek() == Some(b']') { self.bump(); return Ok(out); }
            out.push(self.parse_string()?);
            self.skip_blank();
            match self.peek() {
                Some(b',') => { self.bump(); }
                Some(b']') => { self.bump(); return Ok(out); }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 { 1 }
    else if first < 0xc0 { 1 } // continuation byte — treat as 1 to avoid infinite loop
    else if first < 0xe0 { 2 }
    else if first < 0xf0 { 3 }
    else { 4 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[rubric::verifies(crate::reqs::toml_lite::parses_section_headers)]
    #[rubric::verifies(crate::reqs::toml_lite::parses_integers)]
    #[rubric::verifies(crate::reqs::toml_lite::parses_string_arrays)]
    fn parses_section_and_keys() {
        let src = r#"
[meta]
version = 1

[req.parser.header_magic]
description  = "first line"
satisfied_by = ["crate::parser::check_magic"]
verified_by  = ["crate::parser::tests::a", "crate::parser::tests::b"]
"#;
        let entries = parse(src).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].section, vec!["meta"]);
        assert_eq!(entries[0].key, "version");
        match &entries[0].value { Value::Integer(1) => {}, v => panic!("{:?}", v) }
        assert_eq!(entries[1].section, vec!["req", "parser", "header_magic"]);
        assert_eq!(entries[1].key, "description");
    }

    #[test]
    fn parses_inline_comment() {
        let src = "[a]\nk = 1 # trailing\n";
        let e = parse(src).unwrap();
        assert_eq!(e.len(), 1);
    }

    #[test]
    #[rubric::verifies(crate::reqs::toml_lite::parses_basic_strings)]
    fn parses_string_escapes() {
        let src = r#"[a]
k = "hello\nworld"
"#;
        let e = parse(src).unwrap();
        match &e[0].value { Value::String(s) => assert_eq!(s, "hello\nworld"), v => panic!("{:?}", v) }
    }

    #[test]
    #[rubric::verifies(crate::reqs::toml_lite::rejects_unterminated_strings)]
    fn rejects_unterminated_string() {
        assert!(parse("[a]\nk = \"unterminated\n").is_err());
    }
}
