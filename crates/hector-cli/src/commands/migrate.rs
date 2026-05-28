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

    // Parse-then-set instead of a naive string replace, which would also
    // rewrite `schema_version: 1` inside comments and string values. Comments
    // are lost by the serde round-trip; that's an explicit one-shot tradeoff
    // for migration (and we tell the user below).
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing {} as YAML", bully.display()))?;
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("{} root is not a YAML mapping", bully.display()))?;
    map.insert(
        serde_yaml::Value::String("schema_version".into()),
        serde_yaml::Value::Number(2.into()),
    );
    let migrated = serde_yaml::to_string(&doc)
        .with_context(|| format!("re-serializing migrated {}", bully.display()))?;
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
    println!(
        "note: migration parsed and re-serialized the YAML; comments and \
         non-essential formatting were not preserved."
    );
    println!("note: run `hector trust` next to sign the migrated config.");
    if !clean {
        println!("note: .bully.yml preserved. Run with --clean to remove.");
    }
    Ok(0)
}
