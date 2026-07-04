mod common;

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn no_uppercase_error_prefix_in_human_errors() {
    // `explain` with a missing config — explain.rs:36 still emits uppercase
    // `ERROR:` (pre-sweep). After the sweep it must use lowercase `error:`.
    let tmp = tempdir().unwrap();
    let subdir = tmp.path().join("deep");
    fs::create_dir_all(&subdir).unwrap();
    // Run from a subdir with NO .ironlint.yml anywhere up the tree so
    // resolve_config fails. Use `explain` (not `validate`) because Task 4
    // already swept validate; explain is the remaining ERROR: site.
    let any_file = subdir.join("anything.txt");
    fs::write(&any_file, "x").unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["explain"])
        .arg(&any_file)
        .current_dir(&subdir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("ERROR:"),
        "human errors must use lowercase `error:`, not `ERROR:`; saw:\n{stderr}"
    );
    assert!(
        stderr.contains("error:"),
        "expected an `error:` prefix; saw:\n{stderr}"
    );
}

#[test]
fn no_raw_anyhow_caused_by_chain() {
    // An untrusted config in human mode — the trust error must be flattened,
    // not printed as a multi-line `Caused by:` chain.
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join(".ironlint.yml");
    fs::write(
        &cfg,
        "checks:\n  py:\n    files: [\"*.py\"]\n    run: \"true\"\n",
    )
    .unwrap();
    let src = tmp.path().join("x.py");
    fs::write(&src, "x").unwrap();

    let out = Command::cargo_bin("ironlint")
        .unwrap()
        .args(["check", "--file"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("Caused by:"),
        "raw anyhow chains must not leak to the CLI boundary; saw:\n{stderr}"
    );
}
