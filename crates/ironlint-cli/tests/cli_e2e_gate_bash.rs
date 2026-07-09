//! E2e for `ironlint gate-bash`: stdin command → exit 0 (allow) / 2 (block).
//! Mirrors `cli_e2e_trust`'s `assert_cmd` harness.
//!
//! Pins the binary exit contract and the "no config / no trust needed"
//! property: the subcommand runs in a bare temp dir with no .ironlint.yml.

use assert_cmd::Command;
use tempfile::tempdir;

#[test]
fn allows_ironlint_check_on_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("ironlint check");
    cmd.assert()
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn blocks_ironlint_trust_on_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("ironlint trust");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicates::str::contains(
            "ironlint trust must be run by a human",
        ));
}

#[test]
fn blocks_redirect_to_ironlint_yml_on_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"])
        .write_stdin("echo x > .ironlint.yml");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicates::str::contains(
            "policy files must be edited through the Write/Edit tool",
        ));
}

#[test]
fn allows_empty_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("");
    cmd.assert()
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn runs_with_no_config_present() {
    // The bash-gate is not trust-gated and needs no .ironlint.yml: run it in
    // a bare temp dir with no config, no trust store. It must still decide.
    let dir = tempdir().unwrap();
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"])
        .current_dir(dir.path())
        .write_stdin("ironlint trust");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicates::str::contains(
            "ironlint trust must be run by a human",
        ));
}

#[test]
fn allows_indirection_known_gap() {
    // Pinned: variable-substitution indirection MUST allow (documented gap).
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"])
        .write_stdin("iron$(echo lint) trust");
    cmd.assert()
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn allows_non_utf8_stdin_without_crashing() {
    // Defensive: a genuinely non-UTF8 stdin is unreachable via the adapters'
    // pre-filter (which only pipes bytes that contained an `ironlint`/`.ironlint`
    // substring), but the subcommand must not crash if it ever happens. It
    // allows (exit 0) and logs to stderr — never panics.
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"])
        .write_stdin(b"\xff\xfe ironlint trust");
    cmd.assert().success().code(0);
}
