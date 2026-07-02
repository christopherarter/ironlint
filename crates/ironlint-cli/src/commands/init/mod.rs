//! `ironlint init` — scaffold a starter `.ironlint.yml`.
//!
//! Emits a universal, stack-agnostic baseline regardless of what toolchain
//! manifests exist in the project root. ironlint knows nothing about any
//! tool — checks own their own messages, read stdin, and block by exiting
//! nonzero. Harness onboarding (wiring ironlint's hook into claude-code,
//! pi, opencode, reasonix) is a separate phase handled by `onboard.rs`.

mod onboard;
mod render;

use anyhow::{anyhow, Result};
use std::path::Path;

// Five bools are required by the CLI surface (harness wiring flags); the
// struct_excessive_bools lint would force a state-machine refactor that
// obscures direct flag mapping.
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    pub harnesses: Vec<String>,
    pub global: bool,
    pub yes: bool,
    pub no_hook: bool,
    pub hook_only: bool,
    pub uninstall: bool,
    pub dry_run: bool,
}

/// The universal, stack-agnostic starter config. Two checks that work on any
/// file (read proposed content from stdin), plus commented examples for the
/// two real authoring patterns. ironlint knows nothing about any toolchain.
const BASELINE: &str = r#"checks:
  no-fixme:
    files: ["*"]
    run: "! grep -nE 'FIXME'"
  no-merge-markers:
    files: ["*"]
    run: "! grep -nE '^(<<<<<<< |=======$|>>>>>>> )'"

# --- examples (uncomment and adapt) ---
#
# Wrap a file-oriented linter via the materialized temp file ($IRONLINT_TMPFILE).
# On a write event ironlint writes the proposed content with the correct extension
# beside the real file, hands it to your linter, then removes it:
#
#   my-linter:
#     files: ["src/**/*.ts"]
#     run: "npx my-linter \"$IRONLINT_TMPFILE\""
#
# A stdin check (proposed content arrives on stdin; nonzero exit blocks):
#
#   no-todo:
#     files: ["src/**/*.ts"]
#     run: "! grep -nE 'TODO'"
"#;

pub fn run(dir: &Path, opts: &Options) -> Result<i32> {
    if opts.no_hook && opts.hook_only {
        return Err(anyhow!("--no-hook and --hook-only are mutually exclusive"));
    }

    if !opts.hook_only && !opts.uninstall {
        if opts.dry_run {
            // `scaffold_config` writes the config AND calls `trust::bless`, so it
            // must be skipped entirely on the dry-run path — a dry-run that
            // mutates the security-critical trust store is disqualifying. The
            // preview must also mirror `scaffold_config`'s existing-config
            // skip, or it lies about what a real run would do.
            let cfg_path = dir.join(".ironlint.yml");
            if cfg_path.exists() {
                println!(
                    "config: {} already present (would skip)",
                    cfg_path.display()
                );
            } else {
                println!("would scaffold and trust: {}", cfg_path.display());
            }
        } else {
            scaffold_config(dir)?;
        }
    }

    if opts.no_hook {
        return Ok(0);
    }

    let env = ironlint_core::adapter::AdapterEnv::from_process(dir.to_path_buf())?;
    onboard::run_hook_phase(&env, opts)
}

/// Scaffold + bless `.ironlint.yml`, treating an existing config as a no-op
/// (previously a hard error).
fn scaffold_config(dir: &Path) -> Result<()> {
    let cfg_path = dir.join(".ironlint.yml");
    if cfg_path.exists() {
        println!("config: {} already present (skipped)", cfg_path.display());
        return Ok(());
    }
    let body = BASELINE.to_string();
    std::fs::write(&cfg_path, body)?;
    ironlint_core::trust::bless(&cfg_path).map_err(|e| {
        anyhow!(
            "scaffolded {} but could not trust it: {e:#}",
            cfg_path.display()
        )
    })?;
    println!("scaffolded and trusted: {}", cfg_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_has_universal_checks_and_tmpfile_example() {
        assert!(BASELINE.contains("no-fixme:"));
        assert!(BASELINE.contains("no-merge-markers:"));
        assert!(BASELINE.contains("$IRONLINT_TMPFILE"));
        for tool in ["biome", "eslint", "ruff"] {
            assert!(!BASELINE.contains(tool));
        }
    }

    #[test]
    fn run_rejects_no_hook_and_hook_only_together() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = Options {
            harnesses: vec![],
            global: false,
            yes: false,
            no_hook: true,
            hook_only: true,
            uninstall: false,
            dry_run: false,
        };
        let err = run(tmp.path(), &opts).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn run_with_existing_config_and_no_hook_is_ok_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".ironlint.yml"), "checks: {}\n").unwrap();
        let opts = Options {
            harnesses: vec![],
            global: false,
            yes: false,
            no_hook: true,
            hook_only: false,
            uninstall: false,
            dry_run: false,
        };
        let code = run(tmp.path(), &opts).unwrap();
        assert_eq!(code, 0); // previously this path returned Err
    }
}
