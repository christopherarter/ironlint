use hector_core::baseline::Baseline;

/// refresh() does NOT upgrade v2 file-level entries to v3: file-level entries
/// have no line to re-hash, so their body hash can only come from a fresh
/// `hector baseline record` call. "No upgrade" here is intended, not a bug.
#[test]
fn refresh_does_not_upgrade_v2_file_level_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("v2.json");
    std::fs::write(&path, r#"{"entries":{"[\"r\",\"f.rs\",null]":null}}"#).unwrap();
    let mut b = hector_core::baseline::Baseline::load(&path).expect("v2 grace load");
    let report = b.refresh(tmp.path()).expect("refresh ok");
    assert_eq!(report.refreshed, 0, "no line-bearing entry to refresh");
    assert_eq!(report.dropped, 0, "no entry should be dropped");
    let meta = b
        .entries
        .get("[\"r\",\"f.rs\",null]")
        .expect("entry preserved");
    assert!(
        meta.body_sha256.is_none(),
        "refresh leaves v2 file-level body_sha256 as None; use `hector baseline record` to upgrade"
    );
}

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
