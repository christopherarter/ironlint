use assert_cmd::Command;
use std::fs;

/// `hector trust` writes a blessed entry into the XDG-redirected store.
#[test]
fn trust_writes_a_store_entry() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    )
    .unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    let store = xdg.path().join("hector/trust.json");
    assert!(store.exists(), "trust must create the store file");
    let body = fs::read_to_string(&store).unwrap();
    assert!(body.contains("sha256:"), "store must hold a hash: {body}");
}

/// Blessing a config that does not parse fails (exit 1), writes nothing.
#[test]
fn trust_rejects_unparseable_config() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .failure()
        .code(1);

    // The spec's other half: a rejected config must write nothing to the store.
    let store = xdg.path().join("hector/trust.json");
    assert!(
        !store.exists(),
        "bless must not write the store on parse failure: {store:?}"
    );
}

/// An unblessed config makes `check` fail closed with exit 1.
#[test]
fn unblessed_config_check_exits_1() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"exit 0\"\n",
    )
    .unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("not trusted"));
}

/// After `trust`, `check` admits the config and actually runs its checks — not
/// a vacuous exit 0. A blocking check yields exit 2, which is only reachable if
/// trust passed AND the check executed to its verdict (an untrusted config would
/// exit 1; a config whose check never ran would exit 0).
#[test]
fn blessed_config_check_runs() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"exit 2\"\n",
    )
    .unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(2);
}

/// Editing a check script after blessing revokes trust → check exits 1.
#[test]
fn editing_check_after_bless_blocks_check() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \".hector/gates/g.sh\"\n",
    )
    .unwrap();
    let gates = proj.path().join(".hector/gates");
    fs::create_dir_all(&gates).unwrap();
    fs::write(gates.join("g.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    fs::write(gates.join("g.sh"), "#!/bin/sh\nexit 2\n").unwrap(); // tamper

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("not trusted"));
}
