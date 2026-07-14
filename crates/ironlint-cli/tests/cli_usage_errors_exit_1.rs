//! Usage / argument-parsing errors must exit **1** (config/usage tier), not **2**.
//!
//! Exit code 2 is reserved for a real **Block** verdict (a check exited nonzero
//! 1–125). Adapters map exit 2 to a policy block and show its stdout as the
//! reason — so a typo'd flag becoming an *unexplained block* is disqualifying.
//! clap's default parse-error exit is 2; `main.rs` remaps it to 1 so usage
//! errors are distinguishable from a real Block.

use assert_cmd::Command;

/// A typo'd flag (`--fiel` instead of `--file`) is a usage error: exit 1, not 2.
#[test]
fn typoed_flag_exits_1_not_2() {
    let ec = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--fiel", "x"])
        .assert()
        .get_output()
        .status
        .code()
        .expect("process exited");
    assert_eq!(
        ec, 1,
        "usage error must exit 1 (got {ec}); 2 is reserved for Block"
    );
}

/// Bare `ironlint` (no subcommand) is a usage error: exit 1, not 2.
#[test]
fn no_subcommand_exits_1_not_2() {
    let ec = Command::cargo_bin("ironlint")
        .unwrap()
        .assert()
        .get_output()
        .status
        .code()
        .expect("process exited");
    assert_eq!(
        ec, 1,
        "bare `ironlint` must exit 1 (got {ec}); 2 is reserved for Block"
    );
}

/// A missing required value is a usage error: exit 1, not 2.
#[test]
fn missing_value_exits_1_not_2() {
    // `--file` requires a value; omitting it is a parse error.
    let ec = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .assert()
        .get_output()
        .status
        .code()
        .expect("process exited");
    assert_eq!(
        ec, 1,
        "missing value must exit 1 (got {ec}); 2 is reserved for Block"
    );
}

/// `--help` is not an error: still exits 0.
#[test]
fn help_exits_0() {
    Command::cargo_bin("ironlint")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
}

/// `check --help` is not an error: still exits 0.
#[test]
fn check_help_exits_0() {
    Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--help"])
        .assert()
        .success();
}

#[test]
fn removed_arch_subcommand_is_a_usage_error() {
    Command::cargo_bin("ironlint")
        .unwrap()
        .args(["arch", "--help"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("unrecognized subcommand 'arch'"));
}
