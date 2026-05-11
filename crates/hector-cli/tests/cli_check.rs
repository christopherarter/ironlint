use assert_cmd::Command;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    std::fs::write(&cfg, body).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    cfg
}

#[test]
fn check_passing_rule_exits_zero() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config", cfg.to_str().unwrap(),
            "--file", file.to_str().unwrap(),
            "--format", "json",
        ])
        .assert()
        .code(0);
}

#[test]
fn check_blocking_rule_exits_two() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("bad.txt");
    std::fs::write(&file, "forbidden\n").unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  noforbidden:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"grep -q forbidden {file} && exit 1 || exit 0\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config", cfg.to_str().unwrap(),
            "--file", file.to_str().unwrap(),
            "--format", "json",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(parsed["status"], "block");
    assert_eq!(parsed["violations"][0]["rule_id"], "noforbidden");
}

#[test]
fn check_with_untrusted_config_exits_one() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(
        &cfg,
        "schema_version: 2\nrules:\n  noop:\n    description: \"x\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    ).unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args([
            "check",
            "--config", cfg.to_str().unwrap(),
            "--file", file.to_str().unwrap(),
        ])
        .assert()
        .code(1);
}
