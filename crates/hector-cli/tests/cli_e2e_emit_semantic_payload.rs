//! H1 — end-to-end coverage that `hector check --emit-semantic-payload`
//! produces the expected envelope on stdout.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

const CONFIG_YAML: &str = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
  model: ignored
rules:
  no-debug:
    description: no DEBUG prints in committed code
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;

fn write_trusted_config(dir: &std::path::Path) {
    let path = dir.join(".hector.yml");
    fs::write(&path, CONFIG_YAML).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
}

#[test]
fn flag_emits_deferred_verdict_envelope() {
    let tmp = tempdir().unwrap();
    write_trusted_config(tmp.path());
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config")
        .arg(tmp.path().join(".hector.yml"))
        .arg("--file")
        .arg(&src)
        .arg("--emit-semantic-payload")
        .arg("--format")
        .arg("json")
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("stdout must be valid JSON, got: {stdout}"));
    assert_eq!(v["deferred"], serde_json::Value::Bool(true));
    assert_eq!(v["schema_version"], serde_json::Value::Number(1.into()));
    assert_eq!(v["payload"]["evaluate"][0]["id"].as_str(), Some("no-debug"));
    assert!(v["payload"]["_evaluator_input"]
        .as_str()
        .unwrap()
        .contains("no-debug"));
}

#[test]
fn flag_rejects_combined_with_session() {
    let tmp = tempdir().unwrap();
    write_trusted_config(tmp.path());
    Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config")
        .arg(tmp.path().join(".hector.yml"))
        .arg("--session")
        .arg("--emit-semantic-payload")
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

#[test]
fn flag_omitted_means_no_envelope() {
    // Sanity: without the flag, the CLI emits the standard Verdict shape,
    // not the DeferredVerdict envelope. Asserts the additive nature of the
    // change — no behaviour drift for existing call-sites.
    let tmp = tempdir().unwrap();
    // Use a non-subagent provider so direct-dispatch is attempted but the
    // missing API key makes semantic skip silently.
    let cfg = CONFIG_YAML.replace("claude-code-subagent", "anthropic");
    let path = tmp.path().join(".hector.yml");
    fs::write(&path, cfg).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config")
        .arg(&path)
        .arg("--file")
        .arg(&src)
        .arg("--format")
        .arg("json")
        .assert()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v.get("deferred").is_none(), "no flag, no envelope");
    assert!(
        v.get("status").is_some(),
        "standard Verdict has status field"
    );
}
