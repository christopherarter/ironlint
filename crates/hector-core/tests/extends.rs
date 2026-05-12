use hector_core::config::parse_file_with_extends;
use std::path::PathBuf;

fn workspace_fixture(rel: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/hector-core/`; fixtures live at workspace root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../..").join(rel)
}

#[test]
fn extends_merges_rules() {
    let path = workspace_fixture("tests/fixtures/with_extends/child.hector.yml");
    let cfg = parse_file_with_extends(&path).expect("parse");
    assert!(cfg.rules.contains_key("base-rule"), "base rule inherited");
    assert!(cfg.rules.contains_key("child-rule"), "child rule present");
}

#[test]
fn extends_unions_skip_globs_from_parent_and_child() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = dir.path().join("parent.yml");
    std::fs::write(
        &parent_path,
        "schema_version: 2\nskip:\n  - \"*.snap\"\nrules: {}\n",
    )
    .unwrap();
    let child_path = dir.path().join("child.yml");
    std::fs::write(
        &child_path,
        "schema_version: 2\nextends: [\"./parent.yml\"]\nskip:\n  - \"fixtures/**\"\nrules: {}\n",
    )
    .unwrap();
    let cfg = parse_file_with_extends(&child_path).expect("parse");
    assert!(cfg.skip.contains(&"*.snap".to_string()));
    assert!(cfg.skip.contains(&"fixtures/**".to_string()));
}

#[test]
fn cycle_in_extends_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.yml");
    let b = dir.path().join("b.yml");
    std::fs::write(&a, "schema_version: 2\nextends: [./b.yml]\nrules: {}\n").unwrap();
    std::fs::write(&b, "schema_version: 2\nextends: [./a.yml]\nrules: {}\n").unwrap();
    let result = parse_file_with_extends(&a);
    assert!(result.is_err(), "cycle detection should fail");
    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.to_lowercase().contains("cycle") || err.to_lowercase().contains("loop"),
        "error mentions cycle: {err}"
    );
}

#[test]
fn extends_chain_rejects_untrusted_parent() {
    use hector_core::runner::HectorEngine;
    let tmp = tempfile::tempdir().unwrap();
    // Parent has a script rule but NO trust block — if loaded, this script
    // would run arbitrary code on the host.
    let parent = "schema_version: 2\nrules:\n  exfil:\n    description: bad\n    engine: script\n    scope: [\"**/*\"]\n    severity: error\n    script: \"touch /tmp/PWNED\"\n";
    std::fs::write(tmp.path().join("parent.yml"), parent).unwrap();
    let child_raw = "schema_version: 2\nextends: [\"parent.yml\"]\nrules: {}\n";
    let trusted = hector_core::trust::write_trust_block(child_raw).unwrap();
    let child = tmp.path().join("child.yml");
    std::fs::write(&child, trusted).unwrap();

    let result = HectorEngine::load(&child);
    let err = match result {
        Ok(_) => panic!("must reject untrusted parent"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("trust") || msg.contains("fingerprint"),
        "error should reference trust; got: {msg}"
    );
}
