//! `ironlint update` — self-update to the latest GitHub release.
//!
//! Shells out to the dist installer (`ironlint-cli-installer.sh` on Unix,
//! `ironlint-cli-installer.ps1` on Windows) rather than embedding an HTTP
//! client. The previous implementation used the `axoupdater` crate, which
//! pulled the entire async TLS stack (`tokio`, `reqwest`, `hyper-rustls`,
//! `rustls`, …) into the binary for a single sync command — the heaviest
//! compile-time cost in the dependency tree. Re-running the same installer
//! the user originally installed with is the operation `axoupdater` performed
//! under the hood; doing it directly removes ~30 transitive crates and keeps
//! the behavior identical from the user's perspective.
//!
//! The installer is only meaningful when this binary was installed *by* the
//! installer — otherwise there's no receipt and no install prefix to target.
//! We detect that the same way `axoupdater` did: look for the dist install
//! receipt (`ironlint-cli-receipt.json`) under the config dir it would have
//! been written to. All decision/formatting logic lives in the pure `render`
//! helper (unit-tested in-process); the only I/O is the small [`perform`]
//! shim that resolves the receipt and execs the installer. The no-receipt
//! branch is covered end-to-end by `tests/cli_e2e_update.rs`.

use std::path::PathBuf;

use anyhow::Result;

/// The dist *app name* (the package name `ironlint-cli`, not the `ironlint`
/// binary) — this is what dist prefixes installer assets with and what the
/// install receipt is keyed to (`~/.config/ironlint-cli/`), so it's the name
/// the receipt file is named after (`ironlint-cli-receipt.json`). Verified
/// against the shipped `dist-manifest.json` and `ironlint-cli-installer.sh`.
const APP_NAME: &str = "ironlint-cli";

/// The Unix installer the no-receipt deferral points at — the same script
/// the curl one-liner runs, and what `perform` execs on a receipt-managed
/// install. Matches the released asset name (`ironlint-cli-installer.sh`).
const INSTALLER_SH_URL: &str =
    "https://github.com/ironlint/ironlint/releases/latest/download/ironlint-cli-installer.sh";

/// The Windows installer — the PowerShell equivalent of [`INSTALLER_SH_URL`].
/// `perform` execs this via `powershell` on a receipt-managed Windows install.
const INSTALLER_PS1_URL: &str =
    "https://github.com/ironlint/ironlint/releases/latest/download/ironlint-cli-installer.ps1";

/// Repo root, for the `cargo install --git` line in the no-receipt deferral.
/// `ironlint-cli` isn't published to crates.io, so a bare `cargo install
/// ironlint-cli` would fail — the git form is the correct one.
const REPO_URL: &str = "https://github.com/ironlint/ironlint";

/// The classified result of an update attempt — everything `render` needs to
/// produce output and an exit code, with no live updater state.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Self-update succeeded (the installer ran and exited 0). We don't
    /// introspect the new version — the installer prints its own progress —
    /// so this carries no version pair, only the success signal.
    Updated,
    /// No install receipt: this build came from Homebrew, `cargo install`, or a
    /// source build, so it can't drive the installer. Defer to its real channel.
    NotInstallerManaged,
    /// Anything else went wrong; carries a human-readable reason.
    Failed { reason: String },
}

/// `ironlint update` entry point. Returns the process exit code: `0` on a
/// successful update, `1` on any failure (including not-installer-managed).
pub fn run() -> Result<i32> {
    let (message, code) = render(&perform());
    if code == 0 {
        println!("{message}");
    } else {
        eprintln!("{message}");
    }
    Ok(code)
}

/// Resolve the receipt and, if present, run the installer. The only
/// network/filesystem surface in this module; kept deliberately tiny so the
/// tested logic sits in the pure helpers below.
fn perform() -> Outcome {
    if !receipt_exists(APP_NAME) {
        return Outcome::NotInstallerManaged;
    }
    run_installer()
}

/// Run the dist installer for the current platform, inheriting stdio. The
/// installer is idempotent (exits 0 when already current) and self-replaces
/// the binary in place. Any nonzero exit is surfaced as a failure carrying
/// the installer's own exit code.
fn run_installer() -> Outcome {
    use std::process::Command;
    let status = if cfg!(windows) {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!("irm {INSTALLER_PS1_URL} | iex"),
            ])
            .status()
    } else {
        Command::new("sh")
            .args(["-c", &format!("curl -LsSf {INSTALLER_SH_URL} | sh")])
            .status()
    };
    match status {
        Ok(s) if s.success() => Outcome::Updated,
        // `ExitStatus`'s `Display` is already "exit status: N", so render the
        // raw code to avoid a doubled "exit status: exit status: N" message.
        Ok(s) => Outcome::Failed {
            reason: format!("installer exited with code {}", s.code().unwrap_or(-1)),
        },
        Err(e) => Outcome::Failed {
            reason: format!("failed to spawn installer: {e}"),
        },
    }
}

/// Whether a dist install receipt exists for `app_name` in any of the
/// locations the dist installer writes one. Mirrors `axoupdater`'s
/// `get_config_paths` resolution order so the no-receipt path is deterministic
/// under the same env vars the e2e test sets (`AXOUPDATER_CONFIG_PATH`,
/// `XDG_CONFIG_HOME`, `HOME`, `LOCALAPPDATA`). Existence-only — we don't
/// parse the receipt, so dist receipt-schema changes can't break us.
fn receipt_exists(app_name: &str) -> bool {
    receipt_exists_in(&receipt_paths(app_name), app_name)
}

/// Pure core of [`receipt_exists`]: true if any candidate dir holds a
/// `<app_name>-receipt.json`. Split out so tests can exercise the resolution
/// without mutating process env (which races under parallel test threads).
fn receipt_exists_in(dirs: &[PathBuf], app_name: &str) -> bool {
    let receipt = format!("{app_name}-receipt.json");
    dirs.iter().any(|d| d.join(&receipt).exists())
}

/// Candidate config dirs that may hold a receipt, in resolution order:
/// `AXOUPDATER_CONFIG_WORKING_DIR` (cwd) → `AXOUPDATER_CONFIG_PATH` (literal
/// dir, returned alone) → `XDG_CONFIG_HOME/<app>` → (Unix) `$HOME/.config/<app>`
/// / (Windows) `%LOCALAPPDATA%/<app>`. Matches `axoupdater::get_config_paths`.
///
/// Two deliberate divergences from axoupdater, both to avoid re-adding a dep:
/// (1) `AXOUPDATER_CONFIG_PATH` early-returns like axoupdater, but the
/// `AXOUPDATER_CONFIG_WORKING_DIR` branch uses `std::env::current_dir` rather
/// than axoupdater's `Utf8PathBuf` conversion (same observable result);
/// (2) the Unix home fallback reads `$HOME` directly instead of
/// `homedir::my_home()` (which falls back to getpwuid when `HOME` is unset).
/// Environments with `HOME` unset (some systemd units, Docker containers) won't
/// find a receipt — accepted as a rare edge case not worth a `homedir` dep.
fn receipt_paths(app_name: &str) -> Vec<PathBuf> {
    // axoupdater checks this first: if set, look in the cwd only.
    if std::env::var("AXOUPDATER_CONFIG_WORKING_DIR").is_ok() {
        return std::env::current_dir().map(|d| vec![d]).unwrap_or_default();
    }
    // If set, axoupdater returns *only* this path — no fallthrough to XDG/HOME.
    if let Ok(p) = std::env::var("AXOUPDATER_CONFIG_PATH") {
        return vec![PathBuf::from(p)];
    }
    let mut paths = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(xdg).join(app_name));
    }
    if cfg!(windows) {
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            paths.push(PathBuf::from(la).join(app_name));
        }
    } else if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(home).join(".config").join(app_name));
    }
    paths
}

/// Appended to a successful-update message. A newer binary can embed newer hook
/// artifacts than the copies already materialized into the user's coding agents,
/// and `update` deliberately touches only the binary — so nudge them to
/// re-materialize the hooks. `ironlint init --hook-only` re-wires hooks
/// idempotently without rescaffolding the config.
const REFRESH_HINT: &str =
    "  hooks may be newer in this build — refresh them: ironlint init --hook-only";

/// Turn an `Outcome` into a user-facing message and exit code. Pure.
fn render(outcome: &Outcome) -> (String, i32) {
    match outcome {
        Outcome::Updated => (format!("updated ironlint\n{REFRESH_HINT}"), 0),
        Outcome::NotInstallerManaged => (not_installer_managed_message(), 1),
        Outcome::Failed { reason } => (format!("error: update failed: {reason}"), 1),
    }
}

/// The deferral shown when there's no install receipt: point the user at the
/// channel that *will* update their binary.
fn not_installer_managed_message() -> String {
    format!(
        "error: this ironlint wasn't installed by the ironlint installer, so it can't self-update.\n  \
         • reinstall (recommended): curl -LsSf {INSTALLER_SH_URL} | sh\n  \
         • or, with cargo:          cargo install --git {REPO_URL} ironlint-cli --force"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_updated_includes_refresh_hint_and_exits_zero() {
        let (msg, code) = render(&Outcome::Updated);
        assert_eq!(code, 0);
        assert!(msg.contains("updated ironlint"), "{msg}");
        // A fresh binary may ship newer hooks; the update output must nudge the
        // user to re-materialize them via the existing hook-only init path.
        assert!(msg.contains("ironlint init --hook-only"), "{msg}");
    }

    #[test]
    fn render_not_installer_managed_points_to_real_update_paths_and_exits_one() {
        let (msg, code) = render(&Outcome::NotInstallerManaged);
        assert_eq!(code, 1);
        assert!(msg.contains("can't self-update"), "{msg}");
        // The installer one-liner (also re-establishes the receipt for next time).
        assert!(msg.contains("ironlint-cli-installer.sh"), "{msg}");
        // The cargo path must be the git form — ironlint-cli isn't on crates.io.
        assert!(msg.contains("cargo install --git"), "{msg}");
        assert!(msg.contains("ironlint-cli --force"), "{msg}");
        // brew stays out until a tap actually exists.
        assert!(!msg.contains("brew"), "{msg}");
    }

    #[test]
    fn render_failed_includes_reason_and_exits_one() {
        let (msg, code) = render(&Outcome::Failed {
            reason: "installer exited with code 1".into(),
        });
        assert_eq!(code, 1);
        assert!(msg.contains("installer exited with code 1"), "{msg}");
        // A failed update leaves the binary as-is — no hook refresh to suggest.
        assert!(!msg.contains("hook-only"), "{msg}");
    }

    #[test]
    fn run_installer_failure_reason_has_no_doubled_exit_status_phrase() {
        // ExitStatus Display is "exit status: N"; the reason must render the
        // raw code so the message isn't "installer exited with code exit
        // status: 1". Drive the real format path via a failing `sh -c`.
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .status()
            .unwrap();
        assert!(!status.success());
        let reason = format!("installer exited with code {}", status.code().unwrap_or(-1));
        assert_eq!(reason, "installer exited with code 7");
        assert!(
            !reason.contains("exit status"),
            "doubled phrase leaked into reason: {reason}"
        );
    }

    #[test]
    fn receipt_paths_respects_axoupdater_config_path_first() {
        // When AXOUPDATER_CONFIG_PATH is set, axoupdater returns *only* that
        // path (no XDG/HOME fallthrough). Use a unique sentinel so a real env
        // var on the host can't mask the assertion.
        //
        // NOTE: this mutates process-global env (set_var/remove_var), which is
        // inherently racy under parallel test threads — but it is the only
        // test in this binary that touches AXOUPDATER_CONFIG_PATH, so the race
        // window has no other reader to corrupt.
        let sentinel = "/__ironlint_test_axoupdater_config_path__";
        std::env::set_var("AXOUPDATER_CONFIG_PATH", sentinel);
        let paths = receipt_paths(APP_NAME);
        std::env::remove_var("AXOUPDATER_CONFIG_PATH");
        // Early-return: the candidate list is exactly [sentinel], nothing else.
        assert_eq!(paths, vec![PathBuf::from(sentinel)]);
    }

    #[test]
    fn receipt_exists_in_true_when_receipt_file_present_in_any_dir() {
        let with = tempfile::tempdir().unwrap();
        let without = tempfile::tempdir().unwrap();
        std::fs::write(with.path().join(format!("{APP_NAME}-receipt.json")), "{}").unwrap();
        // Present in the first candidate.
        assert!(receipt_exists_in(
            &[with.path().into(), without.path().into()],
            APP_NAME
        ));
        // Present in a later candidate.
        assert!(receipt_exists_in(
            &[without.path().into(), with.path().into()],
            APP_NAME
        ));
        // Absent everywhere.
        assert!(!receipt_exists_in(&[without.path().into()], APP_NAME));
    }

    #[test]
    fn installer_urls_point_at_ironlint_org_not_stale_christopherarter() {
        // Repo moved from christopherarter/ironlint to ironlint/ironlint;
        // the workspace Cargo.toml `repository` field is already updated, and
        // the installer URLs must match so `update` doesn't 404.
        assert!(
            INSTALLER_SH_URL.starts_with("https://github.com/ironlint/ironlint/"),
            "{INSTALLER_SH_URL}"
        );
        assert!(
            INSTALLER_PS1_URL.starts_with("https://github.com/ironlint/ironlint/"),
            "{INSTALLER_PS1_URL}"
        );
        assert_eq!(REPO_URL, "https://github.com/ironlint/ironlint");
    }
}
