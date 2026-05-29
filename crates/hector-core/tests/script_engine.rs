use hector_core::config::{Capabilities, EngineKind, OutputMode, Rule, Severity, WritesPolicy};
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
        output: OutputMode::default(),
    }
}

#[test]
fn passing_script_produces_no_violation() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ok.txt");
    std::fs::write(&file, "clean\n").unwrap();
    let rule = make_rule("grep -nE 'forbidden' {file} && exit 1 || exit 0");
    let res = run_script_rule("ok-rule", &rule, &file, "", None, dir.path()).expect("run");
    assert!(res.is_empty(), "no violation expected, got {res:?}");
}

#[test]
fn failing_script_produces_violation() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("bad.txt");
    std::fs::write(&file, "forbidden\n").unwrap();
    let rule = make_rule("grep -nE 'forbidden' {file} && exit 1 || exit 0");
    let res = run_script_rule("no-forbidden", &rule, &file, "", None, dir.path()).expect("run");
    assert_eq!(res.len(), 1, "expected exactly one violation, got {res:?}");
    let v = &res[0];
    assert_eq!(v.rule_id, "no-forbidden");
    assert_eq!(v.severity, hector_core::verdict::Severity::Error);
    assert_eq!(v.engine, hector_core::verdict::Engine::Script);
    assert_eq!(v.file, file.display().to_string());
}

#[test]
fn script_engine_quotes_file_path_with_shell_metacharacters() {
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
        output: OutputMode::default(),
    };
    let _ = run_script_rule("evil", &rule, &evil, "", None, cwd);
    assert!(
        !cwd.join("PWNED").exists(),
        "shell injection succeeded — PWNED marker was created"
    );
}
