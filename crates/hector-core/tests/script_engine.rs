use hector_core::config::{Capabilities, EngineKind, Rule, Severity, WritesPolicy};
use hector_core::engine::script::run_script_rule;
use tempfile::tempdir;

fn make_rule(script: &str) -> Rule {
    Rule {
        description: "test rule".into(),
        engine: EngineKind::Script,
        scope: vec!["*".into()],
        severity: Severity::Error,
        script: Some(script.into()),
        pattern: None,
        language: None,
        context: None,
        capabilities: Some(Capabilities {
            network: false,
            writes: WritesPolicy::None,
        }),
        fix_hint: None,
    }
}

#[test]
fn passing_script_produces_no_violation() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let rule = make_rule("grep -nE 'forbidden' {file} && exit 1 || exit 0");
    let res = run_script_rule("ok-rule", &rule, &file, "", dir.path()).expect("run");
    assert!(res.is_none(), "no violation expected");
}

#[test]
fn failing_script_produces_violation() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("bad.txt");
    std::fs::write(&file, "forbidden\n").unwrap();
    let rule = make_rule("grep -nE 'forbidden' {file} && exit 1 || exit 0");
    let res = run_script_rule("no-forbidden", &rule, &file, "", dir.path()).expect("run");
    let v = res.expect("violation expected");
    assert_eq!(v.rule_id, "no-forbidden");
    assert_eq!(v.severity, hector_core::verdict::Severity::Error);
    assert_eq!(v.engine, hector_core::verdict::Engine::Script);
    assert_eq!(v.file, file.display().to_string());
}

#[test]
fn script_engine_quotes_file_path_with_shell_metacharacters() {
    use hector_core::config::{EngineKind, Rule, Severity};
    use hector_core::engine::script::run_script_rule;
    let tmp = tempdir().unwrap();
    let cwd = tmp.path();
    // Filename that, if interpolated unquoted into the shell, would `touch PWNED`.
    let evil_name = "a; touch PWNED; b.txt";
    let evil = cwd.join(evil_name);
    std::fs::write(&evil, "hi").unwrap();
    let rule = Rule {
        description: "echo only".into(),
        engine: EngineKind::Script,
        scope: vec!["**/*".into()],
        severity: Severity::Warning,
        // The script is expected to receive the path as an env-backed shell
        // parameter, not as literal text spliced into the command.
        script: Some("ls -- {file} >/dev/null".into()),
        pattern: None,
        language: None,
        context: None,
        // Unrestricted capabilities so this test exercises the quoting defense
        // itself on every platform — not the Linux mount-namespace, which
        // would block `touch` regardless of the bug.
        capabilities: Some(Capabilities {
            network: true,
            writes: WritesPolicy::Unrestricted,
        }),
        fix_hint: None,
    };
    let _ = run_script_rule("evil", &rule, &evil, "", cwd);
    assert!(
        !cwd.join("PWNED").exists(),
        "shell injection succeeded — PWNED marker was created"
    );
}
