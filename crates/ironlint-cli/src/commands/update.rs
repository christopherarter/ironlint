//! `ironlint update` — self-update to the latest GitHub release.
//!
//! Thin wrapper over [`axoupdater`], which reads the install receipt the dist
//! shell/PowerShell installers write, checks the latest GitHub release, and —
//! when a newer one exists — downloads and runs the same
//! `ironlint-installer.{sh,ps1}` the user originally installed with, then
//! self-replaces the running binary.
//!
//! All decision and formatting logic lives in the pure `render` / `classify_*`
//! helpers (unit-tested in-process). The only network- and filesystem-touching
//! code is the small [`perform`] shim that drives `axoupdater`; its no-receipt
//! branch is covered end-to-end by `tests/cli_e2e_update.rs`.

use anyhow::Result;
use axoupdater::{AxoUpdater, AxoupdateError, AxoupdateResult, UpdateResult};

/// The dist *app name* (the package name `ironlint-cli`, not the `ironlint`
/// binary) — this is what dist prefixes installer assets with and what the
/// install receipt is keyed to (`~/.config/ironlint-cli/`), so it's the name
/// axoupdater must look up. Verified against the shipped `dist-manifest.json`
/// and `ironlint-cli-installer.sh`.
const APP_NAME: &str = "ironlint-cli";

/// The installer the no-receipt deferral points at — the same script the curl
/// one-liner runs. Matches the released asset name (`ironlint-cli-installer.sh`).
const INSTALLER_URL: &str =
    "https://github.com/christopherarter/ironlint/releases/latest/download/ironlint-cli-installer.sh";

/// Repo root, for the `cargo install --git` line in the no-receipt deferral.
/// `ironlint-cli` isn't published to crates.io, so a bare `cargo install
/// ironlint-cli` would fail — the git form is the correct one.
const REPO_URL: &str = "https://github.com/christopherarter/ironlint";

/// The classified result of an update attempt — everything `render` needs to
/// produce output and an exit code, with no live updater state.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Self-update succeeded; `old` is absent only when the receipt didn't
    /// record a prior version.
    Updated { old: Option<String>, new: String },
    /// Already on the latest release — nothing to do.
    AlreadyCurrent,
    /// No install receipt: this build came from Homebrew, `cargo install`, or a
    /// source build, so it can't drive the installer. Defer to its real channel.
    NotInstallerManaged,
    /// Anything else went wrong; carries a human-readable reason.
    Failed { reason: String },
}

/// `ironlint update` entry point. Returns the process exit code: `0` on a
/// successful update or an already-current no-op, `1` on any failure.
pub fn run() -> Result<i32> {
    let (message, code) = render(&perform());
    if code == 0 {
        println!("{message}");
    } else {
        eprintln!("{message}");
    }
    Ok(code)
}

/// Drive `axoupdater` and classify the result. The only network/filesystem
/// surface in this module; kept deliberately tiny so the tested logic sits in
/// the pure helpers below.
fn perform() -> Outcome {
    let mut updater = AxoUpdater::new_for(APP_NAME);
    if let Err(e) = updater.load_receipt() {
        return classify_receipt_error(&e);
    }
    classify_run_result(updater.run_sync())
}

/// Map a receipt-load failure. A *missing* receipt is the common, expected case
/// (non-installer installs) and earns the friendly deferral; every other
/// receipt failure is a genuine error.
fn classify_receipt_error(err: &AxoupdateError) -> Outcome {
    match err {
        AxoupdateError::NoReceipt { .. } => Outcome::NotInstallerManaged,
        other => Outcome::Failed {
            reason: other.to_string(),
        },
    }
}

/// Map the outcome of `run_sync`: `Some` updated, `None` already current, `Err`
/// failed.
fn classify_run_result(result: AxoupdateResult<Option<UpdateResult>>) -> Outcome {
    match result {
        Ok(Some(update)) => Outcome::Updated {
            old: update.old_version.map(|v| v.to_string()),
            new: update.new_version.to_string(),
        },
        Ok(None) => Outcome::AlreadyCurrent,
        Err(e) => Outcome::Failed {
            reason: e.to_string(),
        },
    }
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
        Outcome::Updated {
            old: Some(old),
            new,
        } => (
            format!("updated ironlint v{old} → v{new}\n{REFRESH_HINT}"),
            0,
        ),
        Outcome::Updated { old: None, new } => {
            (format!("updated ironlint to v{new}\n{REFRESH_HINT}"), 0)
        }
        Outcome::AlreadyCurrent => (
            format!(
                "ironlint is already up to date (v{}).",
                env!("CARGO_PKG_VERSION")
            ),
            0,
        ),
        Outcome::NotInstallerManaged => (not_installer_managed_message(), 1),
        Outcome::Failed { reason } => (format!("error: update failed: {reason}"), 1),
    }
}

/// The deferral shown when there's no install receipt: point the user at the
/// channel that *will* update their binary.
fn not_installer_managed_message() -> String {
    format!(
        "error: this ironlint wasn't installed by the ironlint installer, so it can't self-update.\n  \
         • reinstall (recommended): curl -LsSf {INSTALLER_URL} | sh\n  \
         • or, with cargo:          cargo install --git {REPO_URL} ironlint-cli --force"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(s: &str) -> axoupdater::Version {
        axoupdater::Version::parse(s).unwrap()
    }

    #[test]
    fn render_updated_shows_from_and_to() {
        let (msg, code) = render(&Outcome::Updated {
            old: Some("0.3.0".into()),
            new: "0.4.0".into(),
        });
        assert_eq!(code, 0);
        assert!(
            msg.contains("v0.3.0") && msg.contains("v0.4.0") && msg.contains('→'),
            "{msg}"
        );
        // A fresh binary may ship newer hooks; the update output must nudge the
        // user to re-materialize them via the existing hook-only init path.
        assert!(msg.contains("ironlint init --hook-only"), "{msg}");
    }

    #[test]
    fn render_updated_without_old_version_still_succeeds() {
        let (msg, code) = render(&Outcome::Updated {
            old: None,
            new: "0.4.0".into(),
        });
        assert_eq!(code, 0);
        assert!(msg.contains("v0.4.0") && !msg.contains('→'), "{msg}");
        assert!(msg.contains("ironlint init --hook-only"), "{msg}");
    }

    #[test]
    fn render_already_current_is_zero_and_names_running_version() {
        let (msg, code) = render(&Outcome::AlreadyCurrent);
        assert_eq!(code, 0);
        assert!(msg.contains("up to date"), "{msg}");
        assert!(msg.contains(env!("CARGO_PKG_VERSION")), "{msg}");
        // Nothing changed, so there's nothing to refresh — no hook hint.
        assert!(!msg.contains("hook-only"), "{msg}");
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
            reason: "network unreachable".into(),
        });
        assert_eq!(code, 1);
        assert!(msg.contains("network unreachable"), "{msg}");
        // A failed update leaves the binary as-is — no hook refresh to suggest.
        assert!(!msg.contains("hook-only"), "{msg}");
    }

    #[test]
    fn missing_receipt_defers_instead_of_failing() {
        let err = AxoupdateError::NoReceipt {
            app_name: APP_NAME.to_string(),
        };
        assert_eq!(classify_receipt_error(&err), Outcome::NotInstallerManaged);
    }

    #[test]
    fn other_receipt_error_is_a_failure_carrying_the_reason() {
        let err = AxoupdateError::ReceiptLoadFailed {
            app_name: APP_NAME.to_string(),
        };
        let Outcome::Failed { reason } = classify_receipt_error(&err) else {
            panic!("a non-missing receipt error must classify as Failed");
        };
        // The upstream Display text names the app; asserting it survives guards
        // against a regression that drops `e.to_string()` and ships an empty
        // reason.
        assert!(reason.contains(APP_NAME), "{reason}");
    }

    #[test]
    fn run_result_some_is_updated_with_both_versions() {
        let update = UpdateResult {
            old_version: Some(version("0.3.0")),
            new_version: version("0.4.0"),
            new_version_tag: "v0.4.0".to_string(),
            install_prefix: camino::Utf8PathBuf::from("/tmp/ironlint"),
        };
        assert_eq!(
            classify_run_result(Ok(Some(update))),
            Outcome::Updated {
                old: Some("0.3.0".to_string()),
                new: "0.4.0".to_string(),
            },
        );
    }

    #[test]
    fn run_result_none_is_already_current() {
        assert_eq!(classify_run_result(Ok(None)), Outcome::AlreadyCurrent);
    }

    #[test]
    fn run_result_err_is_failure_carrying_the_reason() {
        let err = AxoupdateError::NoStableReleases {
            app_name: APP_NAME.to_string(),
        };
        let Outcome::Failed { reason } = classify_run_result(Err(err)) else {
            panic!("a run_sync error must classify as Failed");
        };
        assert!(reason.contains(APP_NAME), "{reason}");
    }
}
