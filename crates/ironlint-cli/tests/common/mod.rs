//! Shared helpers for CLI integration tests.
//!
//! Rust integration tests under `tests/` each compile as their own crate, so
//! a `pub fn` here that's only consumed by *some* of them (e.g. the
//! hook-contract suites' fixture helpers vs. `blessed_store` used by the CLI
//! suites) reads as dead code from any one binary's point of view. Allow it
//! at the module level rather than sprinkling `#[allow(dead_code)]` per item.
#![allow(dead_code)]

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Bless `config` in a fresh, isolated trust store and return the `TempDir`
/// that backs it. Keep the returned guard alive for the test, and set
/// `XDG_CONFIG_HOME` to `guard.path()` on every `ironlint` invocation that runs
/// `check`, so they all read the same blessed store.
#[must_use]
pub fn blessed_store(config: &Path) -> TempDir {
    let xdg = tempfile::tempdir().unwrap();
    Command::cargo_bin("ironlint")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(config)
        .assert()
        .success();
    xdg
}

/// Absolute path to `rel`, resolved relative to the repo root (two levels up
/// from this crate's `Cargo.toml`). Used by hook-contract tests to locate
/// `adapters/<harness>/hooks/hook.sh`.
#[must_use]
pub fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Build a controlled minimal `PATH` for a hook-contract test: `stub_dir`
/// first, then only the directories that hold the system tools the hook shells
/// out to (`bash`, `jq`, `python3`, `cat`, `mktemp`, `printf`, `rm`, `sed`).
///
/// Directories are resolved from the ambient `PATH`, deduped, and **any dir that
/// also contains an `ironlint` binary is skipped** — so the stub is the sole
/// source of `ironlint`. This is what makes "no stub → ironlint not on PATH"
/// truthful: prepending to the ambient `PATH` (the old approach) let a real
/// `ironlint` on the contributor's machine (e.g. `~/.cargo/bin/ironlint`)
/// leak through and turn a missing-binary test into a real-binary test.
fn isolated_path(stub_dir: &Path) -> String {
    let tools = [
        "bash", "jq", "python3", "cat", "mktemp", "printf", "rm", "sed",
    ];
    let mut dirs: Vec<PathBuf> = vec![stub_dir.to_path_buf()];
    let ambient = std::env::var("PATH").unwrap_or_default();
    for dir in ambient.split(':').map(PathBuf::from) {
        if dirs.contains(&dir) {
            continue;
        }
        // Skip any dir that carries a real `ironlint` — the stub must be the
        // only `ironlint` on the test's PATH.
        if dir.join("ironlint").exists() {
            continue;
        }
        // Only add the dir if it actually holds one of the tools the hook needs;
        // this keeps the PATH minimal and deterministic.
        if tools.iter().any(|t| dir.join(t).exists()) {
            dirs.push(dir);
        }
    }
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(":")
}

/// Write a stub `ironlint` executable into `dir` that drains stdin, ignores
/// whatever CLI args the hook passes (`check --file … --content - --config …
/// --format json`), writes `stdout` to stdout verbatim, and exits `code`.
fn write_stub_ironlint(dir: &Path, code: i32, stdout: &str) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    // Single-quote the payload for the embedded shell script, escaping any
    // literal single quotes the caller's `stdout` might contain.
    let escaped = stdout.replace('\'', "'\\''");
    let script =
        format!("#!/usr/bin/env bash\ncat >/dev/null\nprintf '%s' '{escaped}'\nexit {code}\n");
    let path = dir.join("ironlint");
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

/// Like [`write_stub_ironlint`], but the stub **captures** whatever the hook
/// pipes on stdin to `capture_path` (via `cat > <file>`) before exiting. The
/// plain `write_stub_ironlint` drains stdin to `/dev/null`, so it can only
/// prove the hook *reached* `ironlint` — never *what* it fed in. Content-fold
/// tests (MultiEdit sequential apply, NotebookEdit `new_source`) read
/// `capture_path` back to assert the exact proposed bytes. If the hook never
/// invokes the stub (e.g. a NotebookEdit `delete` that allows without gating),
/// `capture_path` is never written — its absence is the assertion.
fn write_stub_capturing(dir: &Path, code: i32, stdout: &str, capture_path: &Path) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let escaped_out = stdout.replace('\'', "'\\''");
    let escaped_cap = capture_path.to_str().unwrap().replace('\'', "'\\''");
    let script = format!(
        "#!/usr/bin/env bash\ncat > '{escaped_cap}'\nprintf '%s' '{escaped_out}'\nexit {code}\n"
    );
    let path = dir.join("ironlint");
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

/// True only if both `jq` and `python3` are runnable on `PATH` — both hooks
/// shell out to them (JSON parsing, Edit-content synthesis). Hook-contract
/// tests should `eprintln!` a skip note and return early when this is
/// false, rather than hard-failing on a contributor machine that lacks them.
#[must_use]
pub fn hook_tools_available() -> bool {
    std::process::Command::new("jq")
        .arg("--version")
        .output()
        .is_ok()
        && std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok()
}

/// A temp project (carrying its own `.ironlint.yml`, so a hook under test
/// never silently skips), a temp stub-bin dir standing in for a real
/// `ironlint` install, and a temp `HOME`/`XDG_CONFIG_HOME` — so hook-contract
/// tests never touch the real trust store or the invoking user's `$HOME`.
/// All four temp dirs are torn down together when the fixture drops.
#[cfg(unix)]
pub struct HookFixture {
    pub project: TempDir,
    stub_dir: TempDir,
    home: TempDir,
    xdg: TempDir,
    hook_script: PathBuf,
}

#[cfg(unix)]
impl HookFixture {
    /// `hook_script` is the repo-root-relative path to the adapter's
    /// `hook.sh` under test (e.g. `"adapters/claude-code/hooks/hook.sh"`).
    #[must_use]
    pub fn new(hook_script: &str) -> Self {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".ironlint.yml"),
            "checks:\n  g:\n    files: \"*.py\"\n    run: \"exit 0\"\n",
        )
        .unwrap();
        Self {
            project,
            stub_dir: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
            xdg: tempfile::tempdir().unwrap(),
            hook_script: repo_path(hook_script),
        }
    }

    /// Absolute path to `name` inside the project dir.
    #[must_use]
    pub fn file(&self, name: &str) -> PathBuf {
        self.project.path().join(name)
    }

    /// Point the stubbed `ironlint` at this fixture's `PATH` so the next
    /// `run` call resolves it instead of a real install.
    pub fn stub(&self, code: i32, stdout: &str) {
        write_stub_ironlint(self.stub_dir.path(), code, stdout);
    }

    /// Like [`Self::stub`], but the stub records the stdin the hook feeds it
    /// (the synthesized proposed content) to `capture_path` before exiting.
    /// Read `capture_path` back to assert the exact bytes; assert its absence
    /// to prove the hook never reached `ironlint`.
    pub fn stub_capturing(&self, code: i32, stdout: &str, capture_path: &Path) {
        write_stub_capturing(self.stub_dir.path(), code, stdout, capture_path);
    }

    /// Spawn `bash <hook_script> <hook_arg>` against this fixture's isolated
    /// project/PATH/HOME/XDG_CONFIG_HOME, with `stdin` written then closed.
    ///
    /// PATH is a **controlled minimal** set: the stub dir first, then only the
    /// system dirs that actually hold the binaries the hook shells out to
    /// (`bash`, `jq`, `python3`, `cat`, `mktemp`, `printf`, `rm`, `sed`). It is
    /// deliberately NOT `stub_dir:$PATH` — prepending to the ambient PATH leaks
    /// any real `ironlint` on the contributor's machine (e.g. `~/.cargo/bin`)
    /// through to the hook, so the "no stub → ironlint not on PATH" tests
    /// (`bash_fails_closed_when_ironlint_missing`) silently ran the real
    /// `gate-bash` instead of simulating a missing binary. Resolving each tool's
    /// directory from the ambient PATH (and never adding an `ironlint`-bearing
    /// dir) keeps the stub as the sole source of `ironlint`.
    pub fn run(
        &self,
        hook_arg: &str,
        stdin: &str,
        extra_env: &[(&str, &str)],
    ) -> assert_cmd::assert::Assert {
        let path = isolated_path(self.stub_dir.path());
        let mut cmd = Command::new("bash");
        cmd.arg(&self.hook_script)
            .arg(hook_arg)
            .current_dir(self.project.path())
            .env("PATH", path)
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.xdg.path())
            .env_remove("IRONLINT_FAIL_CLOSED_ON_INTERNAL");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.write_stdin(stdin).assert()
    }
}

/// A fixture that puts the REAL `ironlint` binary (built by `cargo test`) on
/// PATH instead of a stub — for end-to-end tests that must exercise the actual
/// subcommand the hook shells out to (e.g. `ironlint gate-bash`). Same isolated
/// project/HOME/XDG_CONFIG_HOME guarantees as [`HookFixture`]; the only
/// difference is `ironlint` resolves to `bin_dir` (the cargo-built binary's
/// parent) rather than a stub.
#[cfg(unix)]
pub struct RealBinFixture {
    pub project: TempDir,
    bin_dir: PathBuf,
    home: TempDir,
    xdg: TempDir,
    hook_script: PathBuf,
}

#[cfg(unix)]
impl RealBinFixture {
    /// `hook_script` is the repo-root-relative path to the adapter's hook;
    /// `ironlint_bin` is the path returned by `assert_cmd::cargo::cargo_bin`.
    #[must_use]
    pub fn new(hook_script: &str, ironlint_bin: &Path) -> Self {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".ironlint.yml"),
            "checks:\n  g:\n    files: \"*.py\"\n    run: \"exit 0\"\n",
        )
        .unwrap();
        Self {
            project,
            bin_dir: ironlint_bin.parent().unwrap().to_path_buf(),
            home: tempfile::tempdir().unwrap(),
            xdg: tempfile::tempdir().unwrap(),
            hook_script: repo_path(hook_script),
        }
    }

    /// Absolute path to `name` inside the project dir.
    #[must_use]
    pub fn file(&self, name: &str) -> PathBuf {
        self.project.path().join(name)
    }

    /// Spawn `bash <hook_script> <hook_arg>` with the real `ironlint` on PATH.
    pub fn run(
        &self,
        hook_arg: &str,
        stdin: &str,
        extra_env: &[(&str, &str)],
    ) -> assert_cmd::assert::Assert {
        let path = format!(
            "{}:{}",
            self.bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new("bash");
        cmd.arg(&self.hook_script)
            .arg(hook_arg)
            .current_dir(self.project.path())
            .env("PATH", path)
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.xdg.path())
            .env_remove("IRONLINT_FAIL_CLOSED_ON_INTERNAL");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.write_stdin(stdin).assert()
    }
}
