//! `cargo rubric seal` — compute function body hashes for every annotated
//! site and write `rubric.lock`. Preserves existing `off` entries; prunes
//! entries whose requirement is no longer in the manifest or whose
//! annotation is no longer in source. Idempotent across repeated
//! invocations.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use rubric_core::hash;
use rubric_core::lockfile::{Seal, Lockfile};
use rubric_core::manifest::Manifest;
use rubric_core::resolver::AnnotationResolver;

use crate::cli::Flags;
use crate::find_manifest;
use rubric_core::resolver::SyntacticResolver;
use rubric_core::walker;

#[rubric::satisfies(crate::reqs::seal::update_lockfile, crate::reqs::seal::off_is_explicit)]
pub fn run(args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(args, &["--manifest-path", "--check"])?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let targets = crate::workspace::manifest_targets(flags.manifest_path.as_deref(), &cwd);
    if targets.is_empty() {
        return Err("no rubric.toml found".to_string());
    }
    if targets.len() == 1 {
        return run_one(targets.into_iter().next().unwrap(), flags.check);
    }
    let mut failures = 0usize;
    for m in &targets {
        let parent = m.parent().unwrap_or(m);
        crate::term::status("Member", &parent.display().to_string());
        if run_one(m.clone(), flags.check).is_err() { failures += 1; }
    }
    if failures > 0 { Err(format!("{} member(s) failed seal", failures)) } else { Ok(()) }
}

fn run_one(manifest_path: PathBuf, check_mode: bool) -> Result<(), String> {
    let crate_root = manifest_path.parent().unwrap().to_path_buf();
    let src_root = crate_root.join("src");
    let src_root = if src_root.is_dir() { src_root } else { crate_root.clone() };

    let manifest_src = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {}", manifest_path.display(), e))?;
    let manifest = Manifest::parse(&manifest_src).map_err(|e| e.to_string())?;
    let sites = walker::walk_dir(&src_root).map_err(|e| e.to_string())?;

    let crate_name = crate::seal_util::derive_crate_name(&crate_root);
    let mut resolver = SyntacticResolver::new(crate_name, src_root.clone());

    let lock_path = crate_root.join("rubric.lock");
    let existing = std::fs::read_to_string(&lock_path).ok()
        .map(|s| Lockfile::parse(&s).unwrap_or_default())
        .unwrap_or_default();

    let mut new_lock = Lockfile::new();
    let mut src_cache: HashMap<PathBuf, String> = HashMap::new();

    for site in &sites {
        let source = if let Some(s) = src_cache.get(&site.file) {
            s.clone()
        } else {
            let s = std::fs::read_to_string(&site.file)
                .map_err(|e| format!("{}: {}", site.file.display(), e))?;
            src_cache.insert(site.file.clone(), s.clone());
            s
        };
        let info = resolver.resolve(site, &source).map_err(|e| e.to_string())?;
        let req_label = site.label_path.join("::");
        let item_path = info.path.clone();

        // Preserve an explicit `off` opt-out.
        if matches!(existing.get(&req_label, &item_path), Some(Seal::Off)) {
            new_lock.set(req_label, item_path, Seal::Off);
            continue;
        }

        // Compute body hash. Sites without a resolvable body (annotations
        // on struct/trait/const) are skipped for v0 — seal tracking applies
        // to function-bearing sites.
        match hash::compute(hash::DEFAULT_SCHEME, &info) {
            Ok(value) => {
                let (scheme, hex) = value.split_once(':').unwrap();
                new_lock.set(req_label, item_path,
                    Seal::Hash { scheme: scheme.to_string(), hex: hex.to_string() });
            }
            Err(_) => continue,
        }
    }

    // Prune: drop entries for requirements the manifest no longer has.
    // (Entries for removed source sites are already absent from new_lock.)
    let known: BTreeSet<String> = manifest.requirements.iter().map(|r| r.label()).collect();
    new_lock.prune(&known);

    let rendered = new_lock.render();

    if check_mode {
        let current = std::fs::read_to_string(&lock_path).unwrap_or_default();
        if current != rendered {
            return Err(format!(
                "{} is out of date — run `cargo rubric seal`",
                lock_path.display()
            ));
        }
        println!("{} is up to date", lock_path.display());
        return Ok(());
    }

    std::fs::write(&lock_path, &rendered)
        .map_err(|e| format!("writing {}: {}", lock_path.display(), e))?;
    println!("wrote {} ({} entries)", lock_path.display(), new_lock.entries.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_tmpdir(tag: &str) -> PathBuf {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("cargo-rubric-{}-{}", tag, ns));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_fixture(dir: &std::path::Path, cargo_name: &str, manifest: &str, lib_rs: &str) {
        std::fs::write(dir.join("Cargo.toml"),
            format!("[package]\nname = \"{}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n", cargo_name)
        ).unwrap();
        std::fs::write(dir.join("rubric.toml"), manifest).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("lib.rs"), lib_rs).unwrap();
    }

    #[test]
    #[rubric::verifies(crate::reqs::seal::update_lockfile)]
    fn writes_lockfile_with_hash_per_site() {
        let dir = fresh_tmpdir("seal-write");
        write_fixture(&dir, "fixture",
            "[meta]\nversion = 1\n[req.p.q]\ndescription = \"x\"\n",
            "#[satisfies(crate::reqs::p::q)]\nfn f() { let _ = 1; }\n",
        );
        run(&["--manifest-path".into(), dir.join("rubric.toml").display().to_string()]).unwrap();
        let lock = std::fs::read_to_string(dir.join("rubric.lock")).unwrap();
        assert!(lock.contains("p::q\tfixture::f\tbody:"), "got lockfile:\n{}", lock);
    }

    #[test]
    #[rubric::verifies(crate::reqs::seal::off_is_explicit)]
    fn preserves_off_entry_across_re_seal() {
        let dir = fresh_tmpdir("seal-off");
        write_fixture(&dir, "fixture",
            "[meta]\nversion = 1\n[req.p.q]\ndescription = \"x\"\n",
            "#[satisfies(crate::reqs::p::q)]\nfn f() { let _ = 1; }\n",
        );
        // Seed an off entry.
        std::fs::write(dir.join("rubric.lock"),
            "# header\np::q\tfixture::f\toff\n"
        ).unwrap();
        run(&["--manifest-path".into(), dir.join("rubric.toml").display().to_string()]).unwrap();
        let lock = std::fs::read_to_string(dir.join("rubric.lock")).unwrap();
        assert!(lock.contains("p::q\tfixture::f\toff"), "off entry was overwritten:\n{}", lock);
    }
}
