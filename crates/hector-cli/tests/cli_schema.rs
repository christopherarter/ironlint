use assert_cmd::Command;

fn schema_stdout() -> String {
    let out = Command::cargo_bin("hector")
        .unwrap()
        .arg("schema")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).expect("schema output is utf8")
}

#[test]
fn schema_prints_the_authoring_guide() {
    let s = schema_stdout();
    assert!(
        s.contains("$HECTOR_FILE"),
        "guide must mention $HECTOR_FILE:\n{s}"
    );
    assert!(
        s.contains("nonzero"),
        "guide must mention the nonzero-blocks contract:\n{s}"
    );
    assert!(
        s.contains("$HECTOR_TMPFILE"),
        "guide must mention $HECTOR_TMPFILE:\n{s}"
    );
}

#[test]
fn schema_output_has_no_yaml_frontmatter() {
    let s = schema_stdout();
    assert!(
        !s.starts_with("---"),
        "frontmatter must be stripped from `hector schema` output, got:\n{s}"
    );
}
