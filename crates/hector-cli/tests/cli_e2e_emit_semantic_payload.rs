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
    // History:
    //   - 1 → 2 (R5): optional payload.evaluator_model field.
    //   - 2 → 3 (B5, 2026-05-25): per-rule context expansion +
    //     per-call random sentinel in `_evaluator_input` (non-additive
    //     change to the rendered string).
    assert_eq!(v["schema_version"], serde_json::Value::Number(3.into()));
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
    // missing API key makes semantic skip silently. R2 (2026-05-23) made
    // `model:` mandatory for direct-API providers (it was implicitly
    // required before, and the subagent stanza omits it now), so we
    // splice a model field back in here.
    let cfg = CONFIG_YAML
        .replace("claude-code-subagent", "anthropic")
        .replace(
            "provider: anthropic",
            "provider: anthropic\n  model: claude-sonnet-4-6",
        );
    let path = tmp.path().join(".hector.yml");
    fs::write(&path, cfg).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .env_remove("ANTHROPIC_API_KEY")
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

#[test]
fn deterministic_block_suppresses_deferred_envelope() {
    // A script rule that exits non-zero (block) plus a semantic rule
    // that would be deferred. The expected behaviour: the script
    // violation is the verdict, exit 2; no DeferredVerdict on stdout.
    let tmp = tempdir().unwrap();
    let cfg = r#"
schema_version: 2
trust:
  fingerprint: PLACEHOLDER
llm:
  provider: claude-code-subagent
rules:
  no-debug-script:
    description: no DEBUG via grep
    engine: script
    scope: ["**/*.rs"]
    severity: error
    script: "grep -n 'DEBUG' {file} && exit 1 || exit 0"
    capabilities:
      network: false
      writes: none
  no-debug-semantic:
    description: no DEBUG prints in committed code
    engine: semantic
    scope: ["**/*.rs"]
    severity: error
"#;
    let path = tmp.path().join(".hector.yml");
    fs::write(&path, cfg).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    let new = hector_core::trust::write_trust_block(&yaml).unwrap();
    fs::write(&path, new).unwrap();
    let src = tmp.path().join("foo.rs");
    fs::write(&src, "fn main() { println!(\"DEBUG\"); }\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("check")
        .arg("--config")
        .arg(&path)
        .arg("--file")
        .arg(&src)
        .arg("--emit-semantic-payload")
        .arg("--format")
        .arg("json")
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        v.get("deferred").is_none(),
        "block suppresses deferred envelope"
    );
    assert_eq!(v["status"].as_str(), Some("block"));

    // R6 (2026-05-23): the deferred semantic rule must surface as a
    // `deferred_rules` entry on the verdict so the interpreter skill
    // can show the user the rule was configured (just not evaluated
    // this turn). Pre-R6 the rule vanished silently — the worst failure
    // mode for a policy tool.
    let deferred = v["deferred_rules"]
        .as_array()
        .expect("deferred_rules must be present and an array on a blocked verdict");
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0]["rule_id"].as_str(), Some("no-debug-semantic"));
    assert_eq!(deferred[0]["severity"].as_str(), Some("error"));
    assert!(deferred[0]["reason"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));

    // C6 (2026-05-25): additive fields (skip_serializing_if) do NOT bump
    // SCHEMA_VERSION. The R6 bump 2 → 3 was reverted; schema_version stays 2.
    assert_eq!(v["schema_version"], serde_json::Value::Number(2.into()));
}
