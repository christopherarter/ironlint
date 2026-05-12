use anyhow::{anyhow, Context, Result};
use std::path::Path;

pub fn run(dir: &Path, clean: bool) -> Result<i32> {
    let bully = dir.join(".bully.yml");
    let hector = dir.join(".hector.yml");

    if !bully.exists() {
        return Err(anyhow!("no .bully.yml found in {}", dir.display()));
    }
    if hector.exists() {
        return Err(anyhow!(
            "{} already exists; refusing to overwrite",
            hector.display()
        ));
    }

    let raw =
        std::fs::read_to_string(&bully).with_context(|| format!("reading {}", bully.display()))?;
    let migrated = raw.replace("schema_version: 1", "schema_version: 2");
    std::fs::write(&hector, migrated)?;

    let bully_dir = dir.join(".bully");
    let hector_dir = dir.join(".hector");
    if bully_dir.exists() && !hector_dir.exists() {
        std::fs::rename(&bully_dir, &hector_dir).with_context(|| {
            format!("moving {} -> {}", bully_dir.display(), hector_dir.display())
        })?;
    }

    if clean {
        std::fs::remove_file(&bully)?;
    }

    println!("migrated: {} -> {}", bully.display(), hector.display());
    if !clean {
        println!("note: .bully.yml preserved. Run with --clean to remove.");
    }
    Ok(0)
}
