use super::super::{check_binary, exit_code, CheckResult, Report, Status};

#[test]
fn exit_code_is_zero_when_all_pass_or_warn() {
    let report = Report {
        ironlint_version: "0".into(),
        checks: vec![
            CheckResult {
                name: "a",
                status: Status::Pass,
                detail: "".into(),
                remediation: None,
            },
            CheckResult {
                name: "b",
                status: Status::Warn,
                detail: "".into(),
                remediation: None,
            },
        ],
    };
    assert_eq!(exit_code(&report), 0);
}

#[test]
fn exit_code_is_one_when_any_fail() {
    let report = Report {
        ironlint_version: "0".into(),
        checks: vec![
            CheckResult {
                name: "a",
                status: Status::Pass,
                detail: "".into(),
                remediation: None,
            },
            CheckResult {
                name: "b",
                status: Status::Fail,
                detail: "boom".into(),
                remediation: Some("fix it".into()),
            },
        ],
    };
    assert_eq!(exit_code(&report), 1);
}

#[test]
fn check_binary_reports_running_version() {
    let r = check_binary();
    assert_eq!(r.status, Status::Pass);
    assert!(r.detail.contains(env!("CARGO_PKG_VERSION")));
}
