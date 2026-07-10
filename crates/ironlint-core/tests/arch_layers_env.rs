use ironlint_core::runner::{CheckInput, CheckOptions, IronLintEngine};
use ironlint_core::verdict::Status;
use std::collections::HashSet;

#[test]
fn arch_layers_env_materialized_for_referencing_check() {
    let dir = tempfile::tempdir().unwrap();
    let config = r#"
architecture:
  layers:
    - name: data
      globs: ["src/data/**"]
  rules:
    - from: data
      may_import: []
checks:
  cap:
    files: "*"
    run: |
      echo "$IRONLINT_ARCH_LAYERS" > "$IRONLINT_ROOT/arch-path.txt"
      cat "$IRONLINT_ARCH_LAYERS" > "$IRONLINT_ROOT/arch-captured.yml"
"#;
    std::fs::write(dir.path().join(".ironlint.yml"), config).unwrap();

    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: HashSet::from(["cap".to_string()]),
            ..Default::default()
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap();

    let target = dir.path().join("x.txt");
    std::fs::write(&target, "body").unwrap();

    let report = engine
        .check_with_explain(CheckInput::File {
            path: target,
            content: "body".to_string(),
        })
        .unwrap();

    assert_eq!(
        report.verdict.status,
        Status::Pass,
        "check should pass; explain: {:?}",
        report.explain
    );

    let captured = std::fs::read_to_string(dir.path().join("arch-captured.yml")).unwrap();
    assert!(
        captured.contains("name: data"),
        "$IRONLINT_ARCH_LAYERS should point to a file containing the layers YAML; got:\n{captured}"
    );

    let path = std::fs::read_to_string(dir.path().join("arch-path.txt"))
        .unwrap()
        .trim()
        .to_string();
    assert!(
        !std::path::Path::new(&path).exists(),
        "arch-layers tempfile should be cleaned up after the check: {path}"
    );
}

#[test]
fn arch_layers_env_unset_when_no_yaml() {
    // A check that references the token without an architecture: block must
    // see $IRONLINT_ARCH_LAYERS unset.
    let dir = tempfile::tempdir().unwrap();
    let config = r#"
checks:
  ref-but-no-yaml:
    files: "*"
    run: "test -z \"$IRONLINT_ARCH_LAYERS\""
"#;
    std::fs::write(dir.path().join(".ironlint.yml"), config).unwrap();

    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: HashSet::from(["ref-but-no-yaml".to_string()]),
            ..Default::default()
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap();

    let target = dir.path().join("x.txt");
    std::fs::write(&target, "body").unwrap();

    let report = engine
        .check_with_explain(CheckInput::File {
            path: target,
            content: "body".to_string(),
        })
        .unwrap();

    assert_eq!(
        report.verdict.status,
        Status::Pass,
        "$IRONLINT_ARCH_LAYERS must be unset when there is no architecture block; explain: {:?}",
        report.explain
    );
}

#[test]
fn arch_layers_env_not_materialized_for_unreferenced_check() {
    // With an architecture block, a check that does not mention the token must
    // not cause an ironlint-arch-* tempfile to be created.
    let dir = tempfile::tempdir().unwrap();
    let config = r#"
architecture:
  layers:
    - name: data
      globs: ["src/data/**"]
  rules:
    - from: data
      may_import: []
checks:
  no-ref:
    files: "*"
    run: "exit 0"
"#;
    std::fs::write(dir.path().join(".ironlint.yml"), config).unwrap();

    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: HashSet::from(["no-ref".to_string()]),
            ..Default::default()
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap();

    let target = dir.path().join("x.txt");
    std::fs::write(&target, "body").unwrap();

    let report = engine
        .check_with_explain(CheckInput::File {
            path: target,
            content: "body".to_string(),
        })
        .unwrap();

    assert_eq!(
        report.verdict.status,
        Status::Pass,
        "unreferenced check should pass; explain: {:?}",
        report.explain
    );

    // Arch-layers files are never written into the project tree.
    let project_leaks: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("ironlint-arch-")
        })
        .map(|e| e.path())
        .collect();
    assert!(
        project_leaks.is_empty(),
        "no arch-layers tempfile should be created in the project tree: {project_leaks:?}"
    );
}

#[test]
fn arch_layers_env_materialized_at_pre_commit() {
    let dir = tempfile::tempdir().unwrap();
    let config = r#"
architecture:
  layers:
    - name: data
      globs: ["src/data/**"]
  rules:
    - from: data
      may_import: []
checks:
  cap:
    files: "*"
    on: [pre-commit]
    run: |
      echo "$IRONLINT_ARCH_LAYERS" > "$IRONLINT_ROOT/arch-path-pc.txt"
      cat "$IRONLINT_ARCH_LAYERS" > "$IRONLINT_ROOT/arch-captured-pc.yml"
"#;
    std::fs::write(dir.path().join(".ironlint.yml"), config).unwrap();

    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: HashSet::from(["cap".to_string()]),
            event: "pre-commit".to_string(),
            ..Default::default()
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap();

    let target = dir.path().join("x.txt");
    std::fs::write(&target, "body").unwrap();

    let report = engine.check_set(&[target]).unwrap();

    assert_eq!(
        report.status,
        Status::Pass,
        "pre-commit check should pass; blocks: {:?}, errors: {:?}",
        report.blocks,
        report.errors
    );

    let captured = std::fs::read_to_string(dir.path().join("arch-captured-pc.yml")).unwrap();
    assert!(
        captured.contains("name: data"),
        "$IRONLINT_ARCH_LAYERS should be materialized at pre-commit; got:\n{captured}"
    );

    let path = std::fs::read_to_string(dir.path().join("arch-path-pc.txt"))
        .unwrap()
        .trim()
        .to_string();
    assert!(
        !std::path::Path::new(&path).exists(),
        "arch-layers tempfile should be cleaned up after pre-commit: {path}"
    );
}
