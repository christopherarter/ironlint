use hector_core::baseline::Baseline;

#[test]
fn loads_v3_fixture() {
    let path = std::path::Path::new("tests/fixtures/baseline_v3.json");
    let b = Baseline::load(path).expect("v3 fixture loads");
    assert_eq!(b.entries.len(), 2);
    let file_level = b
        .entries
        .get("[\"no-debug\",\"src/main.rs\",null]")
        .expect("file-level key present");
    assert!(
        file_level.body_sha256.is_some(),
        "file-level entry has body_sha256"
    );
    assert!(
        file_level.line_sha256.is_none(),
        "file-level entry has no line_sha256"
    );

    let line_level = b
        .entries
        .get("[\"no-todo\",\"src/lib.rs\",42]")
        .expect("line-level key present");
    assert!(
        line_level.line_sha256.is_some(),
        "line-level entry has line_sha256"
    );
    assert!(
        line_level.body_sha256.is_none(),
        "line-level entry has no body_sha256"
    );
}

#[test]
fn v2_to_v3_grace_period() {
    // A v2 file with no body_sha256 must load and treat file-level entries
    // as "match on key only" — the grace-period behavior.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("v2.json");
    std::fs::write(&path, r#"{"entries":{"[\"r\",\"f\",null]":null}}"#).unwrap();
    let b = Baseline::load(&path).expect("v2 grace load");
    let meta = b.entries.get("[\"r\",\"f\",null]").expect("entry present");
    assert!(meta.body_sha256.is_none(), "v2 entry has no body_sha256");
    assert!(meta.line_sha256.is_none(), "v2 entry has no line_sha256");
}
