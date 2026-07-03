use assert_cmd::Command;
use std::fs;

/// `ironlint trust` writes a blessed entry into the XDG-redirected store.
#[test]
fn trust_writes_a_store_entry() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n",
    )
    .unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    let store = xdg.path().join("ironlint/trust.json");
    assert!(store.exists(), "trust must create the store file");
    let body = fs::read_to_string(&store).unwrap();
    assert!(body.contains("sha256:"), "store must hold a hash: {body}");
}

/// Blessing a config that does not parse fails (exit 1), writes nothing.
#[test]
fn trust_rejects_unparseable_config() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .failure()
        .code(1);

    // The spec's other half: a rejected config must write nothing to the store.
    let store = xdg.path().join("ironlint/trust.json");
    assert!(
        !store.exists(),
        "bless must not write the store on parse failure: {store:?}"
    );
}

/// An unblessed (but well-formed, parseable) config makes `check` fail
/// closed with exit **4** — its own code, distinct from exit 1 (config/parse
/// error). Before Task 3.2 this was exit 1, the same code a parse error
/// uses, so an adapter mapping exit 1 -> allow would silently un-gate every
/// edit for a config nobody ever blessed. See `parse_error_config_check_exits_1`
/// below for the sibling guard that a genuine parse error keeps exit 1.
#[test]
fn unblessed_config_check_exits_4() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"exit 0\"\n",
    )
    .unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(4)
        .stderr(predicates::str::contains("not trusted"));
}

/// Sibling guard for `unblessed_config_check_exits_4`: a config that fails to
/// **parse** (legacy pre-0.3 schema) must keep exit **1**, not collapse into
/// the untrusted-config exit 4. It can never even be blessed (`ironlint
/// trust` itself refuses to bless anything that doesn't parse — see
/// `trust_rejects_unparseable_config` above), so this hits `check` directly:
/// the trust layer can't compute a hash over content that doesn't parse, and
/// that failure is a structural config problem, not a "this config was never
/// reviewed" problem — it must surface through the same exit code a load
/// failure would use.
#[test]
fn parse_error_config_check_exits_1() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap(); // legacy -> parser rejects
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(1);
}

/// After `trust`, `check` admits the config and actually runs its checks — not
/// a vacuous exit 0. A blocking check yields exit 2, which is only reachable if
/// trust passed AND the check executed to its verdict (an untrusted config would
/// exit 1; a config whose check never ran would exit 0).
#[test]
fn blessed_config_check_runs() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \"exit 2\"\n",
    )
    .unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    Command::cargo_bin("ironlint")
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

/// Editing a check script after blessing revokes trust → check exits 4 (the
/// blessed hash no longer matches, so this is the untrusted/mismatch case,
/// not a parse error).
#[test]
fn editing_check_after_bless_blocks_check() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  g:\n    files: \"*.rs\"\n    run: \".ironlint/gates/g.sh\"\n",
    )
    .unwrap();
    let gates = proj.path().join(".ironlint/gates");
    fs::create_dir_all(&gates).unwrap();
    fs::write(gates.join("g.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    fs::write(gates.join("g.sh"), "#!/bin/sh\nexit 2\n").unwrap(); // tamper

    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(4)
        .stderr(predicates::str::contains("not trusted"));
}
