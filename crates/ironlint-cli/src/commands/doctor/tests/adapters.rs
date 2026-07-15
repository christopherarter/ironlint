use super::super::adapters::{
    adapter_check, build_hook_deps_result, check_adapters, hook_deps_row, hook_deps_verdict,
    hooks_row, json_hook_adapter_wired,
};
use super::super::{CheckResult, Status};
use super::adapter_env;
use ironlint_core::adapter::{all_harnesses, install, HarnessStatus, Scope};
use tempfile::tempdir;

#[test]
fn check_adapters_reports_installed_codex_as_pass() {
    let tmp = tempdir().unwrap();
    let env = adapter_env(tmp.path());
    let h = all_harnesses()
        .into_iter()
        .find(|h| h.name == "codex")
        .unwrap();
    // check_adapters reads status at Scope::Local; codex (unlike the prior global-only harness)
    // has a real project-local settings file, so install must match scope.
    install(&h, &env, Scope::Local).unwrap();
    let checks = check_adapters(&env);
    let r = checks
        .iter()
        .find(|c| c.name == "codex")
        .expect("codex reported");
    assert_eq!(r.status, Status::Pass);
    assert!(r.detail.contains("installed"));
}

#[test]
fn check_adapters_reports_broken_adapter_as_fail() {
    let tmp = tempdir().unwrap();
    let env = adapter_env(tmp.path());
    let h = all_harnesses()
        .into_iter()
        .find(|h| h.name == "codex")
        .unwrap();
    // check_adapters reads status at Scope::Local; codex (unlike the prior global-only harness)
    // has a real project-local settings file, so install must match scope.
    install(&h, &env, Scope::Local).unwrap();
    // Delete the materialized artifact dir but leave the settings entry → broken.
    std::fs::remove_dir_all(env.config_home.join("ironlint/adapters/codex")).unwrap();
    let checks = check_adapters(&env);
    let r = checks
        .iter()
        .find(|c| c.name == "codex")
        .expect("codex reported");
    assert_eq!(r.status, Status::Fail);
}

fn harness_status(detected: bool, installed: bool, registered: bool) -> HarnessStatus {
    HarnessStatus {
        harness: "codex",
        detected,
        installed,
        registered,
        intact: Some(true),
        current: Some(true),
    }
}

#[test]
fn adapter_check_skips_when_neither_detected_nor_installed() {
    let s = harness_status(false, false, false);
    assert!(adapter_check(&s).is_none());
}

#[test]
fn adapter_check_reports_registered_but_absent_as_fail() {
    // registered in settings but artifact gone AND harness dir absent:
    // must still surface as a broken (Fail) row, not be skipped.
    let s = HarnessStatus {
        harness: "codex",
        detected: false,
        installed: false,
        registered: true,
        intact: None,
        current: None,
    };
    let c = adapter_check(&s).expect("registered-but-absent must not be skipped");
    assert_eq!(c.status, Status::Fail);
}

#[test]
fn adapter_check_warns_when_detected_but_not_installed() {
    let s = harness_status(true, false, false);
    let r = adapter_check(&s).expect("detected harness reported");
    assert_eq!(r.status, Status::Warn);
    assert!(r.detail.contains("not installed"));
    assert!(r
        .remediation
        .unwrap()
        .contains("ironlint init --harness codex"));
}

#[test]
fn adapter_check_warns_when_installed_but_not_registered() {
    let s = harness_status(true, true, false);
    let r = adapter_check(&s).expect("installed harness reported");
    assert_eq!(r.status, Status::Warn);
    assert!(r.detail.contains("not registered"));
}

#[test]
fn adapter_check_warns_when_artifact_modified() {
    let mut s = harness_status(true, true, true);
    s.intact = Some(false);
    let r = adapter_check(&s).expect("modified harness reported");
    assert_eq!(r.status, Status::Warn);
    assert!(r.detail.contains("modified"));
}

#[test]
fn adapter_check_warns_when_artifact_outdated() {
    let mut s = harness_status(true, true, true);
    s.current = Some(false);
    let r = adapter_check(&s).expect("outdated harness reported");
    assert_eq!(r.status, Status::Warn);
    assert!(r.detail.contains("outdated"));
}

#[test]
fn adapter_check_passes_when_installed_and_registered() {
    let s = harness_status(true, true, true);
    let r = adapter_check(&s).expect("healthy harness reported");
    assert_eq!(r.status, Status::Pass);
    assert!(r.remediation.is_none());
}

fn row(name: &'static str, status: Status) -> CheckResult {
    CheckResult {
        name,
        status,
        detail: String::new(),
        remediation: None,
    }
}

#[test]
fn hooks_row_warns_when_no_adapter_rows() {
    let r = hooks_row(&[]);
    assert_eq!(r.status, Status::Warn);
    assert!(r.detail.contains("no coding-agent hooks"));
    assert!(r.remediation.unwrap().contains("ironlint init"));
}

#[test]
fn hooks_row_warns_when_only_unwired_rows() {
    // A detected-but-not-installed harness surfaces as a Warn adapter row; it
    // is NOT wired, so the summary must warn, not report a healthy install.
    let rows = [row("codex", Status::Warn)];
    let r = hooks_row(&rows);
    assert_eq!(r.status, Status::Warn);
    assert!(r.detail.contains("no coding-agent hooks"));
}

#[test]
fn hooks_row_warns_when_only_broken_rows() {
    // A registered-but-broken harness surfaces as a Fail adapter row; still
    // zero hooks are actually wired, so the summary must warn.
    let rows = [row("codex", Status::Fail)];
    let r = hooks_row(&rows);
    assert_eq!(r.status, Status::Warn);
}

#[test]
fn hooks_row_counts_only_wired_pass_rows() {
    // One wired (Pass) harness + one unwired (Warn) harness → pass, but the
    // count reports only the wired one.
    let rows = [row("codex", Status::Pass), row("pi", Status::Warn)];
    let r = hooks_row(&rows);
    assert_eq!(r.status, Status::Pass);
    assert!(
        r.detail.contains("1 harness(es) wired"),
        "detail must count only wired rows: {}",
        r.detail
    );
}

// --- Task 5.23 Part 2: hook-dependency (jq/python3) probe -------------

#[test]
fn hook_deps_omitted_when_no_json_hook_adapter() {
    // No JSON-hook adapter wired → jq/python3 irrelevant → no row at all,
    // regardless of what happens to be on PATH.
    assert!(hook_deps_verdict(false, true, true).is_none());
    assert!(hook_deps_verdict(false, false, false).is_none());
}

#[test]
fn hook_deps_pass_when_both_present() {
    let (status, missing) = hook_deps_verdict(true, true, true).expect("wired → a row");
    assert_eq!(status, Status::Pass);
    assert!(missing.is_empty(), "nothing missing: {missing:?}");
}

#[test]
fn hook_deps_fail_names_missing_jq() {
    let (status, missing) = hook_deps_verdict(true, false, true).expect("wired → a row");
    assert_eq!(status, Status::Fail);
    assert_eq!(missing, vec!["jq"]);
}

#[test]
fn hook_deps_fail_names_missing_python3() {
    let (status, missing) = hook_deps_verdict(true, true, false).expect("wired → a row");
    assert_eq!(status, Status::Fail);
    assert_eq!(missing, vec!["python3"]);
}

#[test]
fn hook_deps_fail_names_both_when_both_missing() {
    let (status, missing) = hook_deps_verdict(true, false, false).expect("wired → a row");
    assert_eq!(status, Status::Fail);
    assert_eq!(missing, vec!["jq", "python3"]);
}

#[test]
fn hook_deps_result_fail_has_remediation_naming_missing() {
    let r = build_hook_deps_result((Status::Fail, vec!["jq"]));
    assert_eq!(r.name, "hook deps");
    assert_eq!(r.status, Status::Fail);
    assert!(
        r.detail.contains("jq"),
        "detail names the missing dep: {}",
        r.detail
    );
    let rem = r.remediation.expect("fail row must remediate");
    assert!(
        rem.contains("jq"),
        "remediation names the missing dep: {rem}"
    );
    assert!(
        rem.contains("fails open"),
        "remediation must explain the fail-open consequence: {rem}"
    );
}

#[test]
fn hook_deps_result_pass_has_no_remediation() {
    let r = build_hook_deps_result((Status::Pass, vec![]));
    assert_eq!(r.name, "hook deps");
    assert_eq!(r.status, Status::Pass);
    assert!(r.remediation.is_none());
}

#[test]
fn json_hook_adapter_wired_false_on_clean_env() {
    let tmp = tempdir().unwrap();
    let env = adapter_env(tmp.path());
    assert!(!json_hook_adapter_wired(&env));
}

#[test]
fn json_hook_adapter_wired_true_when_codex_installed() {
    let tmp = tempdir().unwrap();
    let env = adapter_env(tmp.path());
    let h = all_harnesses()
        .into_iter()
        .find(|h| h.name == "codex")
        .unwrap();
    install(&h, &env, Scope::Local).unwrap();
    assert!(json_hook_adapter_wired(&env));
}

#[test]
fn hook_deps_row_none_when_no_json_hook_adapter() {
    // A clean env has no wired JSON-hook adapter → no deps row, even though
    // jq/python3 may be present on the machine running the test.
    let tmp = tempdir().unwrap();
    let env = adapter_env(tmp.path());
    assert!(hook_deps_row(&env).is_none());
}

#[test]
fn hook_deps_row_present_when_json_hook_adapter_wired() {
    // Installing a JSON-hook adapter makes the row fire. The Pass/Fail
    // status depends on whether the machine has jq/python3, but a row is
    // ALWAYS produced once wired — that Some/None split is machine-agnostic.
    let tmp = tempdir().unwrap();
    let env = adapter_env(tmp.path());
    let h = all_harnesses()
        .into_iter()
        .find(|h| h.name == "codex")
        .unwrap();
    install(&h, &env, Scope::Local).unwrap();
    let r = hook_deps_row(&env).expect("wired adapter → a deps row");
    assert_eq!(r.name, "hook deps");
}
