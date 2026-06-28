//! Loading a crate's rubric state: manifest, lock, and a fresh scan.
//!
//! Shared by the `check`, `accept`, and `trace` verbs so they all see the
//! same view of the tree.

use std::path::Path;

use rubric_trace::check::Scan;
use rubric_trace::hash;
use rubric_trace::lock::{self, Lock};
use rubric_trace::manifest::{self, Manifest};

use crate::scan::scan_files;
use crate::workspace;

pub struct Project {
    pub manifest: Manifest,
    pub lock: Lock,
    pub scan: Scan,
}

pub fn load(root: &Path) -> Result<Project, String> {
    let manifest = read_manifest(root)?;
    let lock = read_lock(root)?;
    let files = workspace::discover(root).map_err(|e| format!("reading sources: {e}"))?;
    let mut scan = scan_files(&files, &manifest);
    resolve_external(root, &mut scan);
    Ok(Project { manifest, lock, scan })
}

/// Resolve `external:` evidence by reading its file. A readable file is
/// content-sealed under the `file:` scheme. A missing one stays unresolved,
/// and the oracle reports it. The loader does the I/O, keeping the core pure.
fn resolve_external(root: &Path, scan: &mut Scan) {
    for item in &mut scan.items {
        let Some(rel) = item.path.strip_prefix("external:") else {
            continue;
        };
        match std::fs::read(root.join(rel)) {
            Ok(bytes) => {
                item.resolved = true;
                item.evidence_seal = Some(hash::file_seal(&bytes));
            }
            Err(_) => {
                item.resolved = false;
                item.evidence_seal = None;
            }
        }
    }
}

pub fn read_manifest(root: &Path) -> Result<Manifest, String> {
    let path = root.join("rubric.toml");
    let src = std::fs::read_to_string(&path)
        .map_err(|e| format!("no rubric.toml here ({e}); run `cargo rubric init`"))?;
    manifest::parse(&src).map_err(|e| e.to_string())
}

/// A missing lockfile is not an error: it reads as an empty lock, so every
/// seal shows as absent until the first `accept`.
pub fn read_lock(root: &Path) -> Result<Lock, String> {
    match std::fs::read_to_string(root.join("rubric.lock")) {
        Ok(src) => lock::parse(&src).map_err(|e| e.to_string()),
        Err(_) => Ok(Lock::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubric_trace::check::{ItemFacts, ItemKind, Visibility};

    fn ext_item(path: &str) -> ItemFacts {
        ItemFacts {
            path: path.into(),
            resolved: false,
            is_test: false,
            is_ignored: false,
            vis: Visibility::Private,
            kind: ItemKind::Fn,
            body: None,
            signature: None,
            evidence_seal: None,
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rubric-ext-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn external_missing_file_is_unresolved() {
        let root = tmp("missing");
        let mut scan = Scan { citations: vec![], items: vec![ext_item("external:docs/nope.pdf")] };
        resolve_external(&root, &mut scan);
        assert!(!scan.items[0].resolved);
        assert!(scan.items[0].evidence_seal.is_none());
    }

    #[test]
    fn external_content_change_breaks_seal() {
        let root = tmp("change");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let f = root.join("docs/snap.txt");

        std::fs::write(&f, b"surface v1").unwrap();
        let mut scan = Scan { citations: vec![], items: vec![ext_item("external:docs/snap.txt")] };
        resolve_external(&root, &mut scan);
        assert!(scan.items[0].resolved);
        let seal_v1 = scan.items[0].evidence_seal.clone().unwrap();
        assert!(seal_v1.starts_with("file:"));

        std::fs::write(&f, b"surface v2").unwrap();
        let mut scan2 = Scan { citations: vec![], items: vec![ext_item("external:docs/snap.txt")] };
        resolve_external(&root, &mut scan2);
        let seal_v2 = scan2.items[0].evidence_seal.clone().unwrap();
        assert_ne!(seal_v1, seal_v2);

        let _ = std::fs::remove_dir_all(&root);
    }
}
