//! Contract tests for the Reasonix adapter hook (`adapters/reasonix/hooks/hook.sh`).
//!
//! These exercise the adapter's PreToolUse contract end-to-end against the
//! real compiled `hector` binary: synthetic Reasonix event JSON is piped to
//! the hook on stdin, and we assert on the exit code (0 = pass-through,
//! 2 = block) and stderr — a fast, deterministic, `cargo test`-native check
//! of the integration seam, with no model and no container.
//!
//! Reasonix is a **PreToolUse / pre-write** adapter: it sends the *proposed*
//! content via `hector check --content -`, before the edit lands on disk.
//! The policy therefore uses an **AST** rule (`no-panic`), which evaluates the
//! proposed content. A `script` rule would be wrong here — script rules read
//! the on-disk file, which doesn't yet hold the proposed edit (the documented
//! "script rules can't gate pre-write" limitation in the adapter README).
//!
//! The hook shells out to `hector` from PATH, so each test prepends the built
//! binary's directory to PATH. `jq`, `python3`, and `bash` must be on PATH
//! (the hook's documented requirements).

use assert_cmd::Command as AssertCommand;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::{tempdir, TempDir};

/// Absolute path to the `hector` binary built for this test run. Cargo sets
/// `CARGO_BIN_EXE_<name>` for integration tests of the crate that defines the
/// binary (hector-cli → `hector`).
const HECTOR_BIN: &str = env!("CARGO_BIN_EXE_hector");

/// AST policy: blocks `panic!(...)` in Rust files under `src/`. Evaluated
/// against the proposed `--content`, so it gates pre-write edits.
const NO_PANIC_CONFIG: &str = "schema_version: 2\n\
     rules:\n  \
       no-panic:\n    \
         description: no panics\n    \
         engine: ast\n    \
         language: rust\n    \
         scope: [\"src/**/*.rs\"]\n    \
         severity: error\n    \
         pattern: panic!($$$)\n";

/// Path to the Reasonix hook script, resolved from the workspace root
/// (`crates/hector-cli` → up two → repo root).
fn reasonix_hook() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root is two levels above CARGO_MANIFEST_DIR")
        .join("adapters/reasonix/hooks/hook.sh")
}

/// Existing PATH with the built `hector` binary's directory prepended, so the
/// hook's bare `hector` invocation resolves to the binary under test.
fn path_with_hector() -> String {
    let bindir = Path::new(HECTOR_BIN)
        .parent()
        .expect("binary has a parent dir");
    match std::env::var("PATH") {
        Ok(existing) => format!("{}:{existing}", bindir.display()),
        Err(_) => bindir.display().to_string(),
    }
}

/// A tempdir project with `src/` and a trusted `.hector.yml` carrying the
/// `no-panic` AST rule. Returns the tempdir guard (kept alive by the caller)
/// and the project path.
fn trusted_project() -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let cfg = dir.path().join(".hector.yml");
    std::fs::write(&cfg, NO_PANIC_CONFIG).unwrap();
    AssertCommand::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();
    let project = dir.path().to_path_buf();
    (dir, project)
}

/// Run the Reasonix hook against `event`, with `project` as cwd and the test
/// `hector` binary on PATH. Returns `(exit_code, stdout, stderr)`.
fn run_hook(project: &Path, event: &serde_json::Value) -> (i32, String, String) {
    let mut child = Command::new("bash")
        .arg(reasonix_hook())
        .arg("pre-tool-use")
        .current_dir(project)
        .env("PATH", path_with_hector())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn reasonix hook.sh");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn write_file_violating_content_blocks() {
    let (_dir, project) = trusted_project();
    let event = json!({
        "event": "PreToolUse",
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "path": "src/foo.rs", "content": "fn b() { panic!(); }\n" }
    });
    let (code, _out, err) = run_hook(&project, &event);
    assert_eq!(
        code, 2,
        "write_file of violating content must block; stderr: {err}"
    );
    assert!(
        err.contains("no-panic"),
        "blocked verdict should name the rule; stderr: {err}"
    );
}

#[test]
fn write_file_clean_content_passes() {
    let (_dir, project) = trusted_project();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "path": "src/foo.rs", "content": "fn b() {}\n" }
    });
    let (code, _out, err) = run_hook(&project, &event);
    assert_eq!(code, 0, "clean content must pass through; stderr: {err}");
}

#[test]
fn edit_file_resulting_violation_blocks() {
    let (_dir, project) = trusted_project();
    std::fs::write(project.join("src/app.rs"), "fn a() {}\n").unwrap();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "edit_file",
        "toolArgs": { "path": "src/app.rs", "search": "fn a() {}", "replace": "fn a() { panic!(); }" }
    });
    let (code, _out, err) = run_hook(&project, &event);
    assert_eq!(
        code, 2,
        "edit introducing panic!() must block; stderr: {err}"
    );
    assert!(err.contains("no-panic"), "stderr: {err}");
}

#[test]
fn edit_file_clean_substitution_passes() {
    let (_dir, project) = trusted_project();
    std::fs::write(project.join("src/app.rs"), "fn a() {}\n").unwrap();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "edit_file",
        "toolArgs": { "path": "src/app.rs", "search": "fn a", "replace": "fn b" }
    });
    let (code, _out, err) = run_hook(&project, &event);
    assert_eq!(
        code, 0,
        "clean substitution must pass through; stderr: {err}"
    );
}

#[test]
fn edit_file_non_unique_search_fails_closed() {
    let (_dir, project) = trusted_project();
    std::fs::write(project.join("src/dup.rs"), "x\nx\n").unwrap();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "edit_file",
        "toolArgs": { "path": "src/dup.rs", "search": "x", "replace": "y" }
    });
    let (code, _out, err) = run_hook(&project, &event);
    assert_eq!(
        code, 2,
        "a search string that is not unique must fail closed; stderr: {err}"
    );
    assert!(
        err.contains("appears 2 times") || err.to_lowercase().contains("exactly one"),
        "should explain the non-unique match; stderr: {err}"
    );
}

#[test]
fn missing_config_is_silent_noop() {
    // No .hector.yml in the project → the hook must be a silent pass-through
    // (exit 0) even for content that would otherwise be blocked.
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let project = dir.path();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "path": "src/foo.rs", "content": "fn b() { panic!(); }\n" }
    });
    let (code, _out, _err) = run_hook(project, &event);
    assert_eq!(code, 0, "no .hector.yml must be a silent no-op");
}

#[test]
fn edit_to_policy_file_short_circuits() {
    // Editing .hector.yml itself must short-circuit: mid-edit the on-disk sha
    // won't match `trust:`, so running hector would surface a misleading
    // internal error. The hook exits 0 before invoking hector.
    let (_dir, project) = trusted_project();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "path": ".hector.yml", "content": "schema_version: 2\n" }
    });
    let (code, _out, _err) = run_hook(&project, &event);
    assert_eq!(code, 0, "edits to the policy file are short-circuited");
}

#[test]
fn multi_edit_is_noop() {
    let (_dir, project) = trusted_project();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "multi_edit",
        "toolArgs": { "path": "src/foo.rs", "edits": [{ "search": "a", "replace": "panic!()" }] }
    });
    let (code, _out, _err) = run_hook(&project, &event);
    assert_eq!(
        code, 0,
        "multi_edit is currently not gated (documented no-op)"
    );
}

#[test]
fn missing_path_is_noop() {
    let (_dir, project) = trusted_project();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "content": "fn b() { panic!(); }\n" }
    });
    let (code, _out, _err) = run_hook(&project, &event);
    assert_eq!(code, 0, "no path in toolArgs → nothing to check");
}

#[test]
fn broken_trust_gate_fails_open() {
    // Changing a rule value after `trust` breaks the fingerprint (a comment
    // wouldn't — trust canonicalizes the parsed YAML), so `hector check`
    // exits non-zero/non-2 (trust-gate failure). The hook must fail OPEN
    // (exit 0) so a misconfigured install doesn't brick the agent, while
    // logging a diagnostic to stderr.
    let (_dir, project) = trusted_project();
    let cfg = project.join(".hector.yml");
    let tampered = std::fs::read_to_string(&cfg)
        .unwrap()
        .replace("description: no panics", "description: tampered");
    std::fs::write(&cfg, tampered).unwrap();
    let event = json!({
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "path": "src/foo.rs", "content": "fn b() { panic!(); }\n" }
    });
    let (code, _out, err) = run_hook(&project, &event);
    assert_eq!(code, 0, "a broken gate must fail open; stderr: {err}");
    assert!(
        err.to_lowercase().contains("error"),
        "fail-open should log a diagnostic; stderr: {err}"
    );
}

// ---------------------------------------------------------------------------
// Golden tests: byte-exact content fidelity (Task 4)
//
// These use a *stub* `hector` that copies its stdin to a capture file and
// exits 0, rather than the real binary. This lets us assert that the hook
// pipes exactly the bytes we expect — no trailing-newline inflation (jq -r)
// and no trailing-newline stripping ($()).
// ---------------------------------------------------------------------------

/// Build a temporary stub `hector` binary (a bash script) that copies its
/// stdin to `capture_path` and exits 0. Returns the `bin/` directory that
/// should be prepended to PATH.
fn stub_hector_dir(tmp: &std::path::Path, capture_path: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = tmp.join("stub_bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("hector");
    std::fs::write(
        &stub,
        format!(
            "#!/usr/bin/env bash\ncat > \"{capture}\"\nexit 0\n",
            capture = capture_path.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// Run the hook with the stub `hector` on PATH (instead of the real binary).
/// Returns `(exit_code, stdout, stderr)`.
fn run_hook_stub(
    project: &Path,
    stub_bin: &Path,
    event: &serde_json::Value,
) -> (i32, String, String) {
    let path_env = format!(
        "{}:{}",
        stub_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut child = std::process::Command::new("bash")
        .arg(reasonix_hook())
        .arg("pre-tool-use")
        .current_dir(project)
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn reasonix hook.sh with stub hector");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn write_file_pipes_byte_exact_content_including_trailing_newline() {
    // A write_file payload whose `content` ends in a single trailing newline
    // must arrive at `hector` stdin with exactly that newline — not two (the
    // `jq -r` bug appends an extra \n).
    let dir = tempdir().unwrap();
    let project = dir.path();
    // Minimal trusted config so the hook doesn't early-exit on missing config.
    let cfg = project.join(".hector.yml");
    std::fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();
    AssertCommand::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    let capture = project.join("captured_stdin");
    let stub_bin = stub_hector_dir(project, &capture);

    let expected = "export const x = 1;\n"; // trailing \n must be preserved exactly
    let event = json!({
        "event": "PreToolUse",
        "cwd": project.to_str().unwrap(),
        "toolName": "write_file",
        "toolArgs": { "path": "src/x.ts", "content": expected },
    });

    let (code, _out, err) = run_hook_stub(project, &stub_bin, &event);
    assert_eq!(code, 0, "stub hector exits 0; err: {err}");

    let captured = std::fs::read(&capture).unwrap();
    assert_eq!(
        captured,
        expected.as_bytes(),
        "hook must pipe byte-exact content: got {:?}, want {:?}",
        String::from_utf8_lossy(&captured),
        expected,
    );
}

#[test]
fn edit_file_pipes_byte_exact_content_including_trailing_newline() {
    // An edit_file whose search/replace produces content ending in \n must
    // arrive at `hector` stdin with exactly that trailing newline — not zero
    // (the $() command-substitution bug strips trailing newlines).
    let dir = tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("src")).unwrap();
    let cfg = project.join(".hector.yml");
    std::fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();
    AssertCommand::cargo_bin("hector")
        .unwrap()
        .args(["trust", "--config", cfg.to_str().unwrap()])
        .assert()
        .success();

    // Seed the file on disk: "fn old() {}\n"
    // After substitution: "fn new() {}\n"  — trailing newline must survive.
    let disk_content = "fn old() {}\n";
    std::fs::write(project.join("src/app.rs"), disk_content).unwrap();
    let expected_proposed = "fn new() {}\n";

    let capture = project.join("captured_stdin");
    let stub_bin = stub_hector_dir(project, &capture);

    let event = json!({
        "event": "PreToolUse",
        "cwd": project.to_str().unwrap(),
        "toolName": "edit_file",
        "toolArgs": {
            "path": "src/app.rs",
            "search": "fn old()",
            "replace": "fn new()"
        },
    });

    let (code, _out, err) = run_hook_stub(project, &stub_bin, &event);
    assert_eq!(code, 0, "stub hector exits 0; err: {err}");

    let captured = std::fs::read(&capture).unwrap();
    assert_eq!(
        captured,
        expected_proposed.as_bytes(),
        "hook must pipe byte-exact proposed content: got {:?}, want {:?}",
        String::from_utf8_lossy(&captured),
        expected_proposed,
    );
}
