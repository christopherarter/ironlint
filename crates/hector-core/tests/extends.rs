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
fn extends_inherits_llm_when_local_omits_it() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = dir.path().join("parent.yml");
    std::fs::write(
        &parent_path,
        "schema_version: 2\nllm:\n  provider: anthropic\n  model: claude-test\nrules: {}\n",
    )
    .unwrap();
    let child_path = dir.path().join("child.yml");
    std::fs::write(
        &child_path,
        "schema_version: 2\nextends: [\"./parent.yml\"]\nrules: {}\n",
    )
    .unwrap();
    let cfg = parse_file_with_extends(&child_path).expect("parse");
    let llm = cfg.llm.expect("llm should be inherited from parent");
    assert_eq!(llm.provider, "anthropic");
    assert_eq!(llm.model, "claude-test");
}

#[test]
fn extends_local_rule_wins_on_id_collision() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = dir.path().join("parent.yml");
    std::fs::write(
        &parent_path,
        "schema_version: 2\nrules:\n  collide:\n    description: \"from parent\"\n    engine: script\n    scope: [\"*\"]\n    severity: warning\n    script: \"echo parent\"\n",
    )
    .unwrap();
    let child_path = dir.path().join("child.yml");
    std::fs::write(
        &child_path,
        "schema_version: 2\nextends: [\"./parent.yml\"]\nrules:\n  collide:\n    description: \"from child\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"echo child\"\n",
    )
    .unwrap();
    let cfg = parse_file_with_extends(&child_path).expect("parse");
    assert_eq!(cfg.rules["collide"].description, "from child");
}

#[test]
fn extends_dedupes_repeated_skip_entries_across_chain() {
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
        "schema_version: 2\nextends: [\"./parent.yml\"]\nskip:\n  - \"*.snap\"\nrules: {}\n",
    )
    .unwrap();
    let cfg = parse_file_with_extends(&child_path).expect("parse");
    let occurrences = cfg.skip.iter().filter(|s| *s == "*.snap").count();
    assert_eq!(occurrences, 1, "duplicate skip entries must be deduped");
}

#[test]
fn extends_errors_for_nonexistent_parent_path() {
    let dir = tempfile::tempdir().unwrap();
    let child_path = dir.path().join("child.yml");
    std::fs::write(
        &child_path,
        "schema_version: 2\nextends: [\"./does-not-exist.yml\"]\nrules: {}\n",
    )
    .unwrap();
    let err = parse_file_with_extends(&child_path).expect_err("missing extends target");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("canonicaliz") || msg.contains("no such file"));
}

#[test]
fn extends_rejects_v1_parent_with_migrate_hint() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = dir.path().join("parent.yml");
    std::fs::write(
        &parent_path,
        "schema_version: 1\nrules: {}\n",
    )
    .unwrap();
    let child_path = dir.path().join("child.yml");
    std::fs::write(
        &child_path,
        "schema_version: 2\nextends: [\"./parent.yml\"]\nrules: {}\n",
    )
    .unwrap();
    let err = parse_file_with_extends(&child_path).expect_err("v1 parent must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("migrate"),
        "error should suggest `hector migrate`; got: {msg}"
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
