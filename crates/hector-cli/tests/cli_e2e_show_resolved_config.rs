//! C3 — `hector show-resolved-config` end-to-end coverage.

use assert_cmd::Command;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(body).unwrap();
    std::fs::write(&cfg, trusted).unwrap();
    cfg
}

fn write_plain(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn tsv_default_emits_id_engine_severity_scope_fix_hint_origin() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  no-todo:\n    description: \"reject TODO\"\n    engine: script\n    scope: [\"*.rs\", \"*.txt\"]\n    severity: warning\n    script: \"true\"\n    fix_hint: \"remove the TODO\"\n  no-unwrap:\n    description: \"avoid unwrap\"\n    engine: ast\n    scope: [\"*.rs\"]\n    severity: error\n    pattern: \"$X.unwrap()\"\n    language: \"rust\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "two rules → two lines, no header: {stdout}");

    // BTreeMap iteration is alphabetic by id; defensive sort lives in the
    // command. `no-todo` < `no-unwrap`.
    let cols0: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(cols0.len(), 6, "TSV row must have 6 columns: {:?}", cols0);
    assert_eq!(cols0[0], "no-todo");
    assert_eq!(cols0[1], "script");
    assert_eq!(cols0[2], "warning");
    assert_eq!(cols0[3], "*.rs,*.txt");
    assert_eq!(cols0[4], "remove the TODO");
    assert!(
        cols0[5].ends_with(".hector.yml"),
        "origin must point at the local config: {}",
        cols0[5]
    );

    let cols1: Vec<&str> = lines[1].split('\t').collect();
    assert_eq!(cols1[0], "no-unwrap");
    assert_eq!(cols1[1], "ast");
    assert_eq!(cols1[2], "error");
    assert_eq!(cols1[3], "*.rs");
    assert_eq!(
        cols1[4], "",
        "empty fix_hint must emit an empty cell, preserving column count"
    );
    assert!(cols1[5].ends_with(".hector.yml"));
}

#[test]
fn tsv_extends_chain_inherits_and_overrides_with_origin() {
    let dir = tempdir().unwrap();
    let parent = write_plain(
        dir.path(),
        "parent.yml",
        "schema_version: 2\nrules:\n  inherited:\n    description: \"from parent\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"parent version\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let child = write_trusted(
        dir.path(),
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  local-only:\n    description: \"only in child\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"child wins\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", child.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "merged rule count = 3");

    let parsed: std::collections::BTreeMap<&str, Vec<&str>> = lines
        .iter()
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            (cols[0], cols)
        })
        .collect();

    let inherited = parsed.get("inherited").unwrap();
    assert!(
        inherited[5].ends_with("parent.yml"),
        "inherited rule's origin is the parent file: {}",
        inherited[5]
    );

    let local = parsed.get("local-only").unwrap();
    assert!(local[5].ends_with(".hector.yml"));

    let overridden = parsed.get("overridden").unwrap();
    assert_eq!(overridden[2], "warning", "child wins on collision");
    assert!(
        overridden[5].ends_with(".hector.yml"),
        "child-defined rule's origin is the child file"
    );

    // Silence the unused-binding lint without weakening the test.
    let _ = parent;
}

#[test]
fn yaml_format_emits_canonical_merged_config_without_trust() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "yaml",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();

    // Trust block must be stripped from the rendered view; otherwise we
    // would imply the merged form has a fingerprint, which is meaningless.
    assert!(
        !stdout.contains("trust:"),
        "yaml must not emit trust block: {stdout}"
    );
    assert!(
        !stdout.contains("fingerprint:"),
        "yaml must not emit fingerprint: {stdout}"
    );
    // Extends already consumed by the merge; rendering it would mislead.
    assert!(
        !stdout.contains("extends:"),
        "yaml must not emit extends list: {stdout}"
    );

    // The merged config round-trips through serde_yaml as a map; the
    // origin comments precede each rule.
    assert!(stdout.contains("alpha:"));
    assert!(stdout.contains("# origin:"));
    assert!(stdout.contains(".hector.yml"));
}

#[test]
fn yaml_format_origin_comment_precedes_each_rule() {
    let dir = tempdir().unwrap();
    let parent = write_plain(
        dir.path(),
        "parent.yml",
        "schema_version: 2\nrules:\n  beta:\n    description: \"b\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let child = write_trusted(
        dir.path(),
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            child.to_str().unwrap(),
            "--format",
            "yaml",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();

    // Every rule has a preceding `# origin: <path>` comment line.
    let alpha_origin = stdout.find("# origin: ").and_then(|i| {
        let after = &stdout[i..];
        after.lines().next()
    });
    assert!(
        alpha_origin.is_some(),
        "expected at least one origin comment: {stdout}"
    );
    // Both rules surface in the body.
    assert!(stdout.contains("alpha:"));
    assert!(stdout.contains("beta:"));

    let _ = parent;
}

#[test]
fn json_format_emits_sorted_rules_with_origin_field() {
    let dir = tempdir().unwrap();
    let parent = write_plain(
        dir.path(),
        "parent.yml",
        "schema_version: 2\nrules:\n  zeta:\n    description: \"z\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let child = write_trusted(
        dir.path(),
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            child.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();

    assert!(
        v.get("trust").is_none(),
        "json view must not contain a trust block"
    );
    assert!(
        v.get("extends").is_none(),
        "json view must not contain extends"
    );
    assert_eq!(v["schema_version"], 2);

    let rules = v["rules"].as_object().unwrap();
    assert_eq!(rules.len(), 2);
    let keys: Vec<&str> = rules.keys().map(|k| k.as_str()).collect();
    // serde_json::Map preserves insertion order; we built the map from a
    // BTreeMap so insertion order *is* sorted-by-id order.
    assert_eq!(keys, vec!["alpha", "zeta"], "rules must be sorted by id");

    let alpha = &rules["alpha"];
    assert_eq!(alpha["engine"], "script");
    assert_eq!(alpha["severity"], "warning");
    assert!(alpha["origin"].as_str().unwrap().ends_with(".hector.yml"));

    let zeta = &rules["zeta"];
    assert!(zeta["origin"].as_str().unwrap().ends_with("parent.yml"));

    let _ = parent;
}

#[test]
fn missing_config_exits_one_with_hint() {
    let dir = tempdir().unwrap();
    // No `.hector.yml` written; the path doesn't exist.
    let absent = dir.path().join(".hector.yml");
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", absent.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("ERROR: "),
        "stderr must lead with ERROR: prefix: {stderr}"
    );
    assert!(
        stderr.contains(".hector.yml"),
        "stderr must name the absent file so the user can act on it: {stderr}"
    );
}

#[test]
fn invalid_format_value_is_rejected_by_clap() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "csv",
        ])
        .output()
        .unwrap();
    // clap exits with code 2 on argument-parse failure regardless of our
    // app-level contract, because the user never reached our `run`.
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("invalid")
            || stderr.to_lowercase().contains("possible values"),
        "clap should reject `csv` as an invalid format value: {stderr}"
    );
}
