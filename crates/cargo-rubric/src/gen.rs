//! `cargo rubric gen` — render the marker module from rubric.toml.

use std::path::PathBuf;

use rubric_core::{codegen, manifest::Manifest};

use crate::cli::Flags;
use crate::find_manifest;

#[rubric::satisfies(crate::reqs::gen::render_marker_module, crate::reqs::gen::detect_drift)]
pub fn run(args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(args, &["--manifest-path", "--output", "--check"])?;

    let manifest_path = match flags.manifest_path {
        Some(p) => PathBuf::from(p),
        None => {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            find_manifest(&cwd).ok_or_else(|| "no rubric.toml found in this or any parent directory".to_string())?
        }
    };

    let src = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {}", manifest_path.display(), e))?;
    let manifest = Manifest::parse(&src).map_err(|e| e.to_string())?;
    let rendered = codegen::render(&manifest);

    let output = match flags.output {
        Some(p) => PathBuf::from(p),
        None => manifest_path.parent().unwrap().join("src").join("__bind.rs"),
    };

    if flags.check {
        let current = std::fs::read_to_string(&output).unwrap_or_default();
        if current != rendered {
            return Err(format!("{} is out of date — re-run `cargo rubric gen`", output.display()));
        }
        println!("{} is up to date", output.display());
        return Ok(());
    }

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {}", parent.display(), e))?;
    }
    std::fs::write(&output, &rendered)
        .map_err(|e| format!("writing {}: {}", output.display(), e))?;
    println!("wrote {}", output.display());
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

    #[test]
    #[rubric::verifies(crate::reqs::gen::render_marker_module)]
    fn gen_writes_marker_module_from_manifest() {
        let dir = fresh_tmpdir("gen-render");
        let manifest = dir.join("rubric.toml");
        let output = dir.join("out.rs");
        std::fs::write(&manifest, "[meta]\nversion = 1\n[req.parser.hm]\ndescription = \"x\"\n").unwrap();
        run(&[
            "--manifest-path".into(), manifest.display().to_string(),
            "--output".into(), output.display().to_string(),
        ]).unwrap();
        let contents = std::fs::read_to_string(&output).unwrap();
        assert!(contents.contains("pub mod parser"));
        assert!(contents.contains("pub struct hm;"));
    }

    #[test]
    #[rubric::verifies(crate::reqs::gen::detect_drift)]
    fn gen_check_fails_when_output_is_stale() {
        let dir = fresh_tmpdir("gen-drift");
        let manifest = dir.join("rubric.toml");
        let output = dir.join("out.rs");
        std::fs::write(&manifest, "[meta]\nversion = 1\n[req.a]\ndescription = \"x\"\n").unwrap();
        std::fs::write(&output, "stale contents").unwrap();
        let r = run(&[
            "--manifest-path".into(), manifest.display().to_string(),
            "--output".into(), output.display().to_string(),
            "--check".into(),
        ]);
        assert!(r.is_err(), "expected drift error, got {:?}", r);
    }
}

