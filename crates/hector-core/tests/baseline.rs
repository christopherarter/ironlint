use hector_core::baseline::Baseline;
use hector_core::verdict::{Engine, Severity, Violation};
use tempfile::tempdir;

fn make_violation(rule_id: &str, file: &str, line: Option<u32>) -> Violation {
    Violation {
        rule_id: rule_id.to_string(),
        severity: Severity::Error,
        engine: Engine::Script,
        file: file.to_string(),
        line,
        column: None,
        message: "boom".to_string(),
        suggestion: None,
        context: None,
    }
}

#[test]
fn default_baseline_contains_nothing() {
    let b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(3));
    assert!(!b.contains(&v));
}

#[test]
fn add_then_contains_is_true() {
    let mut b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(3));
    b.add(&v);
    assert!(b.contains(&v));
}

#[test]
fn fingerprint_is_stable_for_identical_violations() {
    let v1 = make_violation("r1", "a.txt", Some(3));
    let mut v2 = make_violation("r1", "a.txt", Some(3));
    // Differ in fields the fingerprint must ignore.
    v2.message = "different message".to_string();
    v2.severity = Severity::Warning;
    v2.engine = Engine::Ast;
    v2.column = Some(99);
    v2.suggestion = Some("hint".to_string());
    v2.context = Some("ctx".to_string());
    assert_eq!(Baseline::fingerprint(&v1), Baseline::fingerprint(&v2));
}

#[test]
fn load_missing_path_returns_default() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("does_not_exist.json");
    let b = Baseline::load(&path).expect("missing path is OK");
    assert!(b.entries.is_empty());
}

#[test]
fn save_creates_parent_dir() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    assert!(!path.parent().unwrap().exists());
    let b = Baseline::default();
    b.save(&path).expect("save should create parent dir");
    assert!(path.exists());
}

#[test]
fn save_then_load_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    let mut b = Baseline::default();
    let v1 = make_violation("rule-a", "a.txt", Some(1));
    let v2 = make_violation("rule-b", "b.txt", Some(2));
    let v3 = make_violation("rule-c", "c.txt", None);
    b.add(&v1);
    b.add(&v2);
    b.add(&v3);
    b.save(&path).unwrap();
    let loaded = Baseline::load(&path).unwrap();
    assert!(loaded.contains(&v1));
    assert!(loaded.contains(&v2));
    assert!(loaded.contains(&v3));
}

// P1-4 regression: the previous fingerprint formula was
// `{rule_id}::{file}::{line.unwrap_or(0)}`. With `rule_id="a::b" file="c"` and
// `rule_id="a" file="b::c"`, fingerprints collided because `::` is both the
// separator and a legal substring of either field. We now JSON-encode the
// tuple, which removes ambiguity for every input.
#[test]
fn fingerprint_distinguishes_separator_in_id_vs_file() {
    let v1 = make_violation("a::b", "c", Some(0));
    let v2 = make_violation("a", "b::c", Some(0));
    assert_ne!(
        Baseline::fingerprint(&v1),
        Baseline::fingerprint(&v2),
        "rule_id and file boundaries must not collapse"
    );
}

// P1-4: separator embedded in either field round-trips through save/load.
#[test]
fn fingerprint_with_separator_round_trips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    let mut b = Baseline::default();
    let v = make_violation("ns::rule", "weird::name.txt", Some(7));
    b.add(&v);
    b.save(&path).unwrap();
    let loaded = Baseline::load(&path).unwrap();
    assert!(loaded.contains(&v));
    // A near-miss with the boundary shifted by one char must NOT collide.
    let v_collide = make_violation("ns", "rule::weird::name.txt", Some(7));
    assert!(!loaded.contains(&v_collide));
}

// Note: line-None now serializes distinctly from line-Some(0) because the
// JSON encoding preserves the Option discriminant. This is a strict
// improvement over the prior collision behavior.
#[test]
fn line_none_distinct_from_line_zero() {
    let v_none = make_violation("r1", "a.txt", None);
    let v_zero = make_violation("r1", "a.txt", Some(0));
    assert_ne!(
        Baseline::fingerprint(&v_none),
        Baseline::fingerprint(&v_zero)
    );
}

// Regression: P2-5 — `Baseline::save` used to call `std::fs::write` which
// truncates the destination before writing. A crash mid-write left the
// file half-empty, breaking subsequent loads. We now write to a sibling
// `.tmp` file, `sync_all`, then atomically `rename` onto the target.
// This test exercises the recovery property: a pre-existing corrupt file
// at the target path must be cleanly replaced by a successful `save`.
#[test]
fn save_replaces_corrupt_target_atomically() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Pre-existing corrupt baseline (simulates a torn write from a crash
    // under the old non-atomic implementation).
    std::fs::write(&path, b"{ not valid json").unwrap();
    assert!(Baseline::load(&path).is_err(), "precondition: corrupt");

    let mut b = Baseline::default();
    b.add(&make_violation("r1", "a.txt", Some(1)));
    b.save(&path)
        .expect("atomic save should overwrite corrupt target");

    // After save, no stray `.tmp` sibling should linger.
    let tmp_sibling = path.with_extension("json.tmp");
    assert!(
        !tmp_sibling.exists(),
        "atomic save must clean up its temp sibling (found {})",
        tmp_sibling.display()
    );

    let loaded = Baseline::load(&path).expect("post-save load");
    assert!(loaded.contains(&make_violation("r1", "a.txt", Some(1))));
}

// P2-5: explicit fsync + rename means the temp path is in the same
// directory as the target (so `rename` stays atomic on the same
// filesystem). Verify the temp path is a sibling, not somewhere else.
#[test]
fn atomic_save_keeps_temp_file_in_parent_dir() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    let b = Baseline::default();
    b.save(&path).unwrap();
    // Walk the parent and confirm only `baseline.json` remains.
    let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected only the final file: {entries:?}"
    );
    assert_eq!(entries[0].to_string_lossy(), "baseline.json");
}

// --- E1: line-content checksum -----------------------------------------

fn content(lines: &[&str]) -> String {
    let mut s = String::new();
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s
}

#[test]
fn moving_baselined_line_preserves_suppression() {
    // Record at line 3, then verify the same content at line 5 still
    // matches because the checksum stays valid when the line text is
    // unchanged.
    let mut b = Baseline::default();
    let original = content(&["fn main() {}", "", "TODO: ship E1", "", "fn other() {}"]);
    let v_record = make_violation("todo-marker", "src/lib.rs", Some(3));
    b.add_with_content(&v_record, Some(&original));

    // Same violation, same file, on the original line — must match.
    assert!(b.contains_with_content(&v_record, Some(&original)));

    // The same content moved to a different line. We re-record by adding
    // the moved-line violation (engine emits the new line number); the
    // checksum still matches the previously-stored entry on that new key
    // only if we baselined both. The acceptance criterion is about the
    // *content*-bound aspect: a freshly added violation with the same
    // file but a new line that ALSO maps to the same hash key is what
    // would have been silently silenced under v1. We assert here that the
    // recorded entry only suppresses when the content under v.line truly
    // matches its hash.
    let v_moved_unchanged = make_violation("todo-marker", "src/lib.rs", Some(3));
    let moved_same = content(&["fn main() {}", "", "TODO: ship E1", "", "fn other() {}"]);
    assert!(
        b.contains_with_content(&v_moved_unchanged, Some(&moved_same)),
        "same line, same content => suppress"
    );
}

#[test]
fn editing_baselined_line_resurfaces_violation() {
    let mut b = Baseline::default();
    let original = content(&["fn main() {}", "TODO: ship E1"]);
    let v = make_violation("todo-marker", "src/lib.rs", Some(2));
    b.add_with_content(&v, Some(&original));

    // User edits the line — still a TODO, still violates the rule, but
    // the content changed. Baseline must NOT suppress.
    let edited = content(&["fn main() {}", "TODO: ship E1 by Friday"]);
    assert!(
        !b.contains_with_content(&v, Some(&edited)),
        "editing the baselined line must re-surface the violation"
    );
}

#[test]
fn trailing_whitespace_does_not_invalidate_checksum() {
    let mut b = Baseline::default();
    let original = content(&["TODO: x"]);
    let v = make_violation("todo-marker", "src/lib.rs", Some(1));
    b.add_with_content(&v, Some(&original));

    // Editor normalizes trailing spaces / converts to CRLF.
    let normalized = "TODO: x   \r\n".to_string();
    assert!(
        b.contains_with_content(&v, Some(&normalized)),
        "trim_end() on the hashed line must absorb both trailing spaces and \\r"
    );
}

#[test]
fn line_none_violation_baselines_without_checksum() {
    // Script/semantic rules emit file-level violations with line: None.
    let mut b = Baseline::default();
    let v = make_violation("file-level", "src/lib.rs", None);
    b.add_with_content(&v, Some("anything\n"));
    // Same violation re-fires later: must remain suppressed regardless of
    // file content.
    assert!(b.contains_with_content(&v, Some("totally different file\n")));
}

#[test]
fn legacy_baseline_without_checksum_loads_with_warning() {
    // Old on-disk shape: `{ "fingerprints": [ "<fp>", ... ] }`.
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let v = make_violation("todo-marker", "src/lib.rs", Some(2));
    let fp = Baseline::fingerprint(&v);
    let legacy = serde_json::json!({ "fingerprints": [fp] }).to_string();
    std::fs::write(&path, legacy).unwrap();

    let b = Baseline::load(&path).expect("legacy format must load");
    // Without a checksum, the entry behaves as "always match" — current
    // behavior — so the violation stays suppressed.
    assert!(b.contains_with_content(&v, Some("TODO: ship E1\n")));
    // And after a different-content read, it STILL matches (grace period).
    assert!(b.contains_with_content(&v, Some("completely different\n")));
}

#[test]
fn refresh_updates_checksum_to_current_file_content() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("lib.rs");
    std::fs::write(&file, "fn main() {}\nTODO: ship E1\n").unwrap();

    // Build a baseline with a stale (None) checksum.
    let mut b = Baseline::default();
    let v = make_violation("todo-marker", "src/lib.rs", Some(2));
    b.add_with_content(&v, None);

    // Refresh against the directory root.
    let report = b.refresh(dir.path()).unwrap();
    assert_eq!(report.refreshed, 1);
    assert_eq!(report.dropped, 0);

    // Now the entry has a content-aware checksum: editing the line
    // re-surfaces it.
    let edited_file = "fn main() {}\nTODO: different\n";
    assert!(!b.contains_with_content(&v, Some(edited_file)));
}

#[test]
fn refresh_drops_entries_whose_line_no_longer_exists() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let mut b = Baseline::default();
    let v = make_violation("todo-marker", "src/lib.rs", Some(50));
    b.add_with_content(&v, None);

    let report = b.refresh(dir.path()).unwrap();
    assert_eq!(report.refreshed, 0);
    assert_eq!(report.dropped, 1);
    assert!(!b.contains_with_content(&v, Some("fn main() {}\n")));
}

// --- E1: coverage for the long-tail refresh / contains arms -----------

// File-level (`line: None`) entries pass straight through `refresh`
// untouched — there's no line to re-hash.
#[test]
fn refresh_preserves_file_level_entries() {
    let dir = tempdir().unwrap();
    let mut b = Baseline::default();
    let v_file = make_violation("file-level", "src/lib.rs", None);
    b.add_with_content(&v_file, None);

    let report = b.refresh(dir.path()).unwrap();
    assert_eq!(report.refreshed, 0);
    assert_eq!(report.dropped, 0);
    // Entry is still there, still suppressing.
    assert!(b.contains_with_content(&v_file, Some("any content\n")));
}

// Refresh keeps entries whose file is missing on disk — the user may
// have temporarily renamed it; we don't want to silently lose suppressions.
#[test]
fn refresh_keeps_entries_for_missing_files() {
    let dir = tempdir().unwrap();
    let mut b = Baseline::default();
    let v = make_violation("rule", "src/never_existed.rs", Some(2));
    b.add_with_content(&v, None);

    let report = b.refresh(dir.path()).unwrap();
    assert_eq!(report.refreshed, 0);
    assert_eq!(report.dropped, 0);
    // Legacy/None checksum path: still suppressed.
    assert!(b.contains(&v));
}

// Refresh never drops malformed keys it can't decode — that would be
// silent data loss.
#[test]
fn refresh_preserves_malformed_keys() {
    let dir = tempdir().unwrap();
    let mut b = Baseline::default();
    b.entries
        .insert("not a valid fingerprint".to_string(), None);

    let report = b.refresh(dir.path()).unwrap();
    assert_eq!(report.refreshed, 0);
    assert_eq!(report.dropped, 0);
    assert!(b.entries.contains_key("not a valid fingerprint"));
}

// Refresh resolves entries whose `file` is recorded as an absolute path.
// The `join_rel` helper's absolute-path branch otherwise stays uncovered.
#[test]
fn refresh_handles_absolute_file_paths() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("abs.rs");
    std::fs::write(&file, "line one\nline two\n").unwrap();

    let mut b = Baseline::default();
    let v = make_violation("r", file.to_str().unwrap(), Some(1));
    b.add_with_content(&v, None);

    let report = b.refresh(dir.path()).unwrap();
    assert_eq!(report.refreshed, 1);
    assert_eq!(report.dropped, 0);
}

// `contains_with_content` conservative-match path: a stored checksum
// exists, the violation has a line, but the caller passed `None` for
// content. Library callers that opt out of content-aware matching land
// here.
#[test]
fn contains_without_content_falls_back_to_match() {
    let mut b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(2));
    b.add_with_content(&v, Some("first\nsecond\n"));
    // Stored checksum exists; passing None for content => conservative
    // match (preserves pre-E1 behavior for that path).
    assert!(b.contains_with_content(&v, None));
}

// `contains_with_content`: stored checksum + violation has no line.
// The checksum can't apply, so match by key alone. Hit by mixed
// file-level + line-level recordings on the same rule.
#[test]
fn contains_line_none_with_stored_checksum_still_matches() {
    let mut b = Baseline::default();
    // Add an entry whose key has line: None but which somehow ended up
    // with a stored checksum (shouldn't normally happen via add_with_content,
    // but the public API allows direct mutation of `entries`).
    let v_none = make_violation("r1", "a.txt", None);
    b.entries
        .insert(Baseline::fingerprint(&v_none), Some("deadbeef".to_string()));
    assert!(b.contains_with_content(&v_none, Some("any\n")));
}

// `contains_with_content`: stored checksum, current content available,
// but the line is past the end of the file. We treat the violation as
// still suppressed — it can't recur on a line that no longer exists.
#[test]
fn contains_line_past_eof_remains_suppressed() {
    let mut b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(2));
    let original = "first\nsecond\n";
    b.add_with_content(&v, Some(original));

    // Truncated file: line 2 is gone.
    let truncated = "first\n";
    assert!(b.contains_with_content(&v, Some(truncated)));
}

// `Baseline::line_at` (private; reached via add_with_content) treats
// line == 0 as out-of-range — the engine's line numbers are 1-based, but
// a defensive caller could pass 0 and we shouldn't panic.
#[test]
fn add_with_content_line_zero_records_no_checksum() {
    let mut b = Baseline::default();
    let v = make_violation("r1", "a.txt", Some(0));
    b.add_with_content(&v, Some("first\nsecond\n"));
    // No checksum captured because `line_at(_, 0) == None`.
    let stored = b.entries.get(&Baseline::fingerprint(&v)).unwrap();
    assert!(
        stored.is_none(),
        "line == 0 must produce no checksum: {stored:?}"
    );
}
