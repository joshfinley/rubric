//! Tiny flag parser shared across subcommands.
//!
//! Recognises `--key value` and `--key=value` for a fixed set of long
//! options. Anything unrecognised is returned as an error so typos don't
//! get silently swallowed.

pub struct Flags {
    pub manifest_path: Option<String>,
    pub output: Option<String>,
    pub check: bool,
}

impl Flags {
    pub fn parse(args: &[String], allowed: &[&str]) -> Result<Self, String> {
        let mut flags = Flags { manifest_path: None, output: None, check: false };
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            let (name, inline_val) = match arg.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            if !allowed.contains(&name.as_str()) {
                return Err(format!("unknown flag '{}'", name));
            }
            let take_value = |i: &mut usize| -> Result<String, String> {
                if let Some(v) = inline_val.clone() { Ok(v) }
                else {
                    *i += 1;
                    args.get(*i).cloned().ok_or_else(|| format!("flag '{}' requires a value", name))
                }
            };
            match name.as_str() {
                "--manifest-path" => flags.manifest_path = Some(take_value(&mut i)?),
                "--output" => flags.output = Some(take_value(&mut i)?),
                "--check" => flags.check = true,
                _ => unreachable!(),
            }
            i += 1;
        }
        Ok(flags)
    }
}
