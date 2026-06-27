//! Loading a crate's rubric state: manifest, lock, and a fresh scan.
//!
//! Shared by the `check`, `accept`, and `trace` verbs so they all see the
//! same view of the tree.

use std::path::Path;

use rubric_trace::check::Scan;
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
    let scan = scan_files(&files, &manifest);
    Ok(Project { manifest, lock, scan })
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
