//! Helpers shared between `seal` and `check`.

/// Naive `[package] name = "..."` extraction from Cargo.toml.
pub fn derive_crate_name(crate_root: &std::path::Path) -> String {
    let cargo_toml = crate_root.join("Cargo.toml");
    let Ok(src) = std::fs::read_to_string(&cargo_toml) else { return "crate".to_string(); };
    let mut in_package = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') { in_package = t == "[package]"; continue; }
        if !in_package { continue; }
        if let Some(rest) = t.strip_prefix("name") {
            let rest = rest.trim_start_matches([' ', '\t', '=']);
            if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.find('"').map(|e| &s[..e])) {
                return inner.replace('-', "_");
            }
        }
    }
    "crate".to_string()
}
