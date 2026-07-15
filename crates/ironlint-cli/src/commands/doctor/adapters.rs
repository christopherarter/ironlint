use ironlint_core::adapter::{
    all_harnesses, status, AdapterEnv, HarnessKind, HarnessStatus, Scope,
};
use std::path::Path;

use super::{CheckResult, Status};

/// Decide the (status, detail, remediation) triple for a harness that is
/// detected or installed. Split out of `adapter_check` so the if/else-if ladder
/// stays under the cognitive-complexity cap.
fn adapter_verdict(s: &HarnessStatus) -> (Status, String, Option<String>) {
    if s.registered && !s.installed {
        (
            Status::Fail,
            "registered in settings but hook artifact is missing (broken)".to_string(),
            Some(format!("re-run `ironlint init --harness {}`", s.harness)),
        )
    } else if !s.installed {
        (
            Status::Warn,
            "harness detected; ironlint hook not installed".to_string(),
            Some(format!("run `ironlint init --harness {}`", s.harness)),
        )
    } else if !s.registered {
        (
            Status::Warn,
            "hook artifact present but not registered in settings".to_string(),
            Some(format!("re-run `ironlint init --harness {}`", s.harness)),
        )
    } else if s.intact == Some(false) {
        (
            Status::Warn,
            "hook artifact modified since install".to_string(),
            Some("re-run `ironlint init` to restore".to_string()),
        )
    } else if s.current == Some(false) {
        (
            Status::Warn,
            "hook artifact outdated".to_string(),
            Some("re-run `ironlint init` to update".to_string()),
        )
    } else {
        (Status::Pass, "installed and registered".to_string(), None)
    }
}

/// Map one harness's status to a doctor CheckResult. Returns None for a
/// harness that is neither present nor installed (no signal worth a line).
pub(super) fn adapter_check(s: &HarnessStatus) -> Option<CheckResult> {
    if !s.detected && !s.installed && !s.registered {
        return None;
    }
    let (status, detail, remediation) = adapter_verdict(s);
    Some(CheckResult {
        name: s.harness,
        status,
        detail,
        remediation,
    })
}

/// Always-present summary row over the per-harness adapter rows. Warns when
/// zero coding-agent hooks are wired — the most common first-run failure mode,
/// since the tool's entire effect happens through hooks. Only a `Pass` adapter
/// row (installed AND registered) counts as wired: a `Warn` row (detected but
/// ironlint not installed) or a `Fail` row (registered but broken) is present
/// but NOT wired, so a machine with, e.g., Claude Code installed and `ironlint
/// init` never run must still warn — not report a healthy install.
pub(super) fn hooks_row(adapter_rows: &[CheckResult]) -> CheckResult {
    let wired = adapter_rows
        .iter()
        .filter(|r| r.status == Status::Pass)
        .count();
    if wired == 0 {
        CheckResult {
            name: "hooks",
            status: Status::Warn,
            detail: "no coding-agent hooks detected".into(),
            remediation: Some("run `ironlint init`".into()),
        }
    } else {
        CheckResult {
            name: "hooks",
            status: Status::Pass,
            detail: format!("{wired} harness(es) wired"),
            remediation: None,
        }
    }
}

/// Per-harness adapter checks. Uses Local scope because `ironlint init` defaults
/// to a project-local install and `doctor` runs in a project; a status() error
/// for a harness is skipped rather than failing the whole report.
pub(super) fn check_adapters(env: &AdapterEnv) -> Vec<CheckResult> {
    all_harnesses()
        .iter()
        .filter_map(|h| status(h, env, Scope::Local).ok())
        .filter_map(|s| adapter_check(&s))
        .collect()
}

/// The adapter block of the report: per-harness rows, the `hook deps` row (only
/// when a JSON-hook adapter is wired), and the always-present `hooks` summary.
/// Ordering reads as: each adapter, then its runtime deps, then the summary.
/// When the adapter environment can't be resolved, no adapter is detectable,
/// so only the (warning) hooks summary over an empty set is emitted.
pub(super) fn adapter_section(dir: &Path) -> Vec<CheckResult> {
    let Ok(env) = AdapterEnv::from_process(dir.to_path_buf()) else {
        return vec![hooks_row(&[])];
    };
    let adapter_rows = check_adapters(&env);
    let hooks = hooks_row(&adapter_rows);
    let mut section = adapter_rows;
    if let Some(deps) = hook_deps_row(&env) {
        section.push(deps);
    }
    section.push(hooks);
    section
}

/// True when at least one JSON-hook adapter (claude-code / codex) is installed
/// or registered on this machine. Those hooks shell out to `jq` and `python3`;
/// the TS-plugin harnesses (pi / opencode) don't, so they're excluded. A merely
/// *detected* harness (ironlint hook not wired in) doesn't count — the hook
/// can't fire, so its deps aren't yet relevant.
pub(super) fn json_hook_adapter_wired(env: &AdapterEnv) -> bool {
    all_harnesses()
        .iter()
        .filter(|h| matches!(h.kind, HarnessKind::JsonHook(_)))
        .filter_map(|h| status(h, env, Scope::Local).ok())
        .any(|s| s.installed || s.registered)
}

/// Pure decision for the `hook deps` row, split from PATH probing so it's
/// unit-testable without depending on what's installed on the test machine.
/// - not wired → `None` (no JSON-hook adapter → jq/python3 irrelevant, no row)
/// - wired, both present → `Some(Pass, [])`
/// - wired, any missing → `Some(Fail, [missing names in a stable order])`
pub(super) fn hook_deps_verdict(
    wired: bool,
    jq_present: bool,
    python3_present: bool,
) -> Option<(Status, Vec<&'static str>)> {
    if !wired {
        return None;
    }
    let mut missing: Vec<&'static str> = Vec::new();
    if !jq_present {
        missing.push("jq");
    }
    if !python3_present {
        missing.push("python3");
    }
    let status = if missing.is_empty() {
        Status::Pass
    } else {
        Status::Fail
    };
    Some((status, missing))
}

/// Render the pure [`hook_deps_verdict`] decision into a `CheckResult`. A Fail
/// names the missing binaries and spells out the consequence: a missing dep
/// makes the JSON-hook adapters fail OPEN, silently un-gating every edit.
pub(super) fn build_hook_deps_result(
    (status, missing): (Status, Vec<&'static str>),
) -> CheckResult {
    if status == Status::Pass {
        return CheckResult {
            name: "hook deps",
            status: Status::Pass,
            detail: "`jq` and `python3` found on PATH".into(),
            remediation: None,
        };
    }
    let pronoun = if missing.len() > 1 { "them" } else { "it" };
    CheckResult {
        name: "hook deps",
        status: Status::Fail,
        detail: format!("missing on PATH: {}", missing.join(", ")),
        remediation: Some(format!(
            "install {} — the claude-code/codex hook needs {pronoun} or it fails open \
             (every edit is silently un-gated)",
            missing.join(" and "),
        )),
    }
}

/// The `hook deps` row: probe `jq`/`python3` on PATH, but only when a JSON-hook
/// adapter is actually wired (otherwise the deps are irrelevant → `None`).
pub(super) fn hook_deps_row(env: &AdapterEnv) -> Option<CheckResult> {
    let verdict = hook_deps_verdict(
        json_hook_adapter_wired(env),
        crate::commands::check::binary_available("jq"),
        crate::commands::check::binary_available("python3"),
    )?;
    Some(build_hook_deps_result(verdict))
}
