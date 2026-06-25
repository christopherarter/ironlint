//! `--format` is honored by the read-only inspection commands.
//!
//! Regression coverage for the finding that `explain` and
//! `show-resolved-config` bound `--format` and then discarded it, silently
//! emitting the human/tsv text regardless of the requested format.

use assert_cmd::Command;
use tempfile::tempdir;

const TWO_GATE_BODY: &str =
    "gates:\n  ts-gate:\n    files: [\"**/*.ts\"]\n    run: \"true\"\n  rs-gate:\n    files: [\"**/*.rs\"]\n    run: \"true\"\n";

fn write_config(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(&cfg, body).unwrap();
    (dir, cfg)
}

#[test]
fn explain_json_emits_parseable_array() {
    let (dir, cfg) = write_config(TWO_GATE_BODY);
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
            "--format",
            "json",
            "--config",
            cfg.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value =
        serde_json::from_slice(&out).expect("explain --format json must emit valid JSON");
    let arr = value.as_array().expect("explain JSON must be an array");
    assert_eq!(arr.len(), 2, "expected one entry per gate: {value}");

    // Locate the rs-gate entry and assert the full object shape.
    let rs = arr
        .iter()
        .find(|e| e["gate"] == "rs-gate")
        .expect("rs-gate entry must be present");
    assert_eq!(rs["status"], "match", "rs-gate matches a .rs file: {rs}");
    assert_eq!(rs["run"], "true");
    assert!(rs["files"].is_array(), "files must be a JSON array: {rs}");
    assert_eq!(rs["files"][0], "**/*.rs");

    let ts = arr
        .iter()
        .find(|e| e["gate"] == "ts-gate")
        .expect("ts-gate entry must be present");
    assert_eq!(ts["status"], "skip", "ts-gate skips a .rs file: {ts}");
}

#[test]
fn explain_human_emits_text_not_json() {
    let (dir, cfg) = write_config(TWO_GATE_BODY);
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "explain",
            "--format",
            "human",
            "--config",
            cfg.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    // Human form carries the `files=`/`run=` markers and is NOT valid JSON.
    assert!(
        stdout.contains("files=") && stdout.contains("run="),
        "human form must keep the key=value markers: {stdout}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&stdout).is_err(),
        "human form must not parse as JSON: {stdout}"
    );
}

#[test]
fn show_resolved_config_json_is_structured_per_gate() {
    let (_dir, cfg) = write_config(
        "gates:\n  no-todo:\n    files: [\"*.rs\", \"*.txt\"]\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--format",
            "json",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&out)
        .expect("show-resolved-config --format json must emit valid JSON");
    let arr = value.as_array().expect("must be a JSON array");
    let gate = &arr[0];
    assert_eq!(gate["gate"], "no-todo");
    assert!(
        gate["origin"].as_str().unwrap().contains(".hector.yml"),
        "origin must reference the config file: {gate}"
    );
    assert_eq!(gate["files"][0], "*.rs");
    assert_eq!(gate["files"][1], "*.txt");
    assert!(
        gate["run"].as_str().unwrap().contains("grep -q TODO"),
        "run must be carried verbatim: {gate}"
    );
}

#[test]
fn show_resolved_config_yaml_is_parseable() {
    let (_dir, cfg) = write_config("gates:\n  alpha:\n    files: [\"*.rs\"]\n    run: \"true\"\n");

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--format",
            "yaml",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    let value: serde_yaml::Value = serde_yaml::from_str(&stdout)
        .expect("show-resolved-config --format yaml must emit valid YAML");
    let seq = value.as_sequence().expect("YAML must be a sequence");
    let first = seq[0].as_mapping().expect("each entry is a mapping");
    assert_eq!(
        first
            .get(serde_yaml::Value::String("gate".into()))
            .and_then(|v| v.as_str()),
        Some("alpha"),
        "first gate id must be alpha: {stdout}"
    );
}

#[test]
fn show_resolved_config_tsv_rows_are_tab_separated() {
    let (_dir, cfg) =
        write_config("gates:\n  no-todo:\n    files: [\"*.rs\", \"*.txt\"]\n    run: \"true\"\n");

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--format",
            "tsv",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);

    let line = stdout
        .lines()
        .find(|l| l.starts_with("no-todo"))
        .expect("must emit a row for no-todo");
    let cols: Vec<&str> = line.split('\t').collect();
    assert_eq!(
        cols.len(),
        4,
        "TSV row must have 4 tab-separated columns: {line:?}"
    );
    assert_eq!(cols[0], "no-todo");
    assert!(cols[1].contains(".hector.yml"), "col 2 is origin: {line:?}");
    assert_eq!(
        cols[2], "*.rs,*.txt",
        "col 3 is comma-joined files: {line:?}"
    );
    assert_eq!(cols[3], "true", "col 4 is the run command: {line:?}");
}
