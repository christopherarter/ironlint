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

// Regression: fingerprints must not collide when `::` appears inside a field.
// With `rule_id="a::b" file="c"` versus `rule_id="a" file="b::c"`, a naive
// `{rule_id}::{file}::{line}` formula collides because `::` is both the
// separator and a legal substring; JSON-encoding the tuple removes the
// ambiguity for every input.
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

// A separator embedded in either field must round-trip through save/load.
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

// line-None serializes distinctly from line-Some(0): the JSON encoding
// preserves the Option discriminant, so the two must not collide.
#[test]
fn line_none_distinct_from_line_zero() {
    let v_none = make_violation("r1", "a.txt", None);
    let v_zero = make_violation("r1", "a.txt", Some(0));
    assert_ne!(
        Baseline::fingerprint(&v_none),
        Baseline::fingerprint(&v_zero)
    );
}

// Regression: `Baseline::save` writes to a sibling `.tmp` file, `sync_all`s,
// then atomically `rename`s onto the target, so a crash mid-write can never
// leave a half-written file. A pre-existing corrupt file at the target path
// must be cleanly replaced by a successful `save`.
#[test]
fn save_replaces_corrupt_target_atomically() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".hector").join("baseline.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Pre-existing corrupt baseline (simulates a torn write from a crash).
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

// The temp file must live in the same directory as the target so `rename`
// stays atomic on one filesystem. Verify it's a sibling, not elsewhere.
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

// --- line-content checksum ---------------------------------------------

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

    // Suppression is content-bound: an entry only suppresses when the content
    // under `v.line` matches its stored hash, not merely the (file, line) key.
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

/// Regression: a file-level violation (line: None) must resurface when the
/// underlying message content changes — suppression keys on the body, not
/// just the (rule_id, file) pair.
#[test]
fn file_level_baseline_resurfaces_when_message_changes() {
    use hector_core::baseline::Baseline;
    use hector_core::verdict::{Engine, Severity, Violation};

    let mut b = Baseline::default();
    let v_old = Violation {
        rule_id: "no-debug".to_string(),
        severity: Severity::Error,
        engine: Engine::Script,
        file: "src/main.rs".to_string(),
        line: None,
        column: None,
        message: "DEBUG_OLD: leftover trace".to_string(),
        suggestion: None,
        context: None,
    };
    b.add_with_content(&v_old, None);
    assert!(
        b.contains_with_content(&v_old, None),
        "same body must match"
    );

    let v_new = Violation {
        message: "DEBUG_NEW: completely different problem".to_string(),
        ..v_old.clone()
    };
    assert!(
        !b.contains_with_content(&v_new, None),
        "different body on same (rule_id, file) must NOT match"
    );
}

/// Timestamp-shaped substrings must not defeat body matching.
#[test]
fn file_level_baseline_ignores_timestamps_in_body() {
    use hector_core::baseline::Baseline;
    use hector_core::verdict::{Engine, Severity, Violation};

    let mut b = Baseline::default();
    let v_first = Violation {
        rule_id: "linter".to_string(),
        severity: Severity::Error,
        engine: Engine::Script,
        file: "x.py".to_string(),
        line: None,
        column: None,
        message: "scanned at 2026-05-24T12:00:00; found 3 issues: A, B, C".to_string(),
        suggestion: None,
        context: None,
    };
    b.add_with_content(&v_first, None);

    let v_later = Violation {
        message: "scanned at 2026-05-25T09:30:11; found 3 issues: A, B, C".to_string(),
        ..v_first.clone()
    };
    assert!(
        b.contains_with_content(&v_later, None),
        "same body modulo timestamp must still match"
    );
}

/// ANSI color escapes must not defeat body matching.
#[test]
fn file_level_baseline_ignores_ansi_in_body() {
    use hector_core::baseline::Baseline;
    use hector_core::verdict::{Engine, Severity, Violation};

    let mut b = Baseline::default();
    let v_with_color = Violation {
        rule_id: "r".to_string(),
        severity: Severity::Error,
        engine: Engine::Script,
        file: "f".to_string(),
        line: None,
        column: None,
        message: "\x1b[31merror:\x1b[0m bad thing".to_string(),
        suggestion: None,
        context: None,
    };
    b.add_with_content(&v_with_color, None);

    let v_plain = Violation {
        message: "error: bad thing".to_string(),
        ..v_with_color.clone()
    };
    assert!(
        b.contains_with_content(&v_plain, None),
        "stripping ANSI must yield equivalent body checksums"
    );
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

// --- long-tail refresh / contains arms ---------------------------------

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
    use hector_core::baseline::EntryMeta;
    let dir = tempdir().unwrap();
    let mut b = Baseline::default();
    b.entries
        .insert("not a valid fingerprint".to_string(), EntryMeta::default());

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
    // Stored checksum exists; passing None for content falls back to a
    // conservative match.
    assert!(b.contains_with_content(&v, None));
}

// `contains_with_content`: stored line_sha256 + violation has no line.
// The line checksum can't apply; body path handles it. This exercises
// the grace-period path where a v2 entry (no body_sha256) loads and
// matches anything for the file-level slot.
#[test]
fn contains_line_none_with_stored_checksum_still_matches() {
    use hector_core::baseline::EntryMeta;
    let mut b = Baseline::default();
    // Simulate a v2-era entry: line_sha256 set, body_sha256 absent.
    let v_none = make_violation("r1", "a.txt", None);
    b.entries.insert(
        Baseline::fingerprint(&v_none),
        EntryMeta {
            line_sha256: Some("deadbeef".to_string()),
            body_sha256: None,
        },
    );
    // Grace period: no body_sha256 → match anything.
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
    // No line_sha256 captured because `line_at(_, 0) == None`.
    // body_sha256 is also None because the violation has a line (Some(0)).
    let stored = b.entries.get(&Baseline::fingerprint(&v)).unwrap();
    assert!(
        stored.line_sha256.is_none(),
        "line == 0 must produce no line_sha256: {stored:?}"
    );
    assert!(
        stored.body_sha256.is_none(),
        "line == Some(0) must produce no body_sha256: {stored:?}"
    );
}

/// Body normalization must preserve multi-byte UTF-8 characters intact: a
/// byte-to-char cast would explode them into separate per-byte scalars,
/// corrupting the checksum.
#[test]
fn body_checksum_preserves_non_ascii_characters() {
    use hector_core::baseline::Baseline;
    // Two messages differing only in a non-ASCII character must hash
    // differently (proving the char wasn't corrupted into bytes).
    let with_euro = "cost was €5 at 2026-05-24T12:00:00";
    let with_pound = "cost was £5 at 2026-05-24T12:00:00";
    let h_euro = Baseline::body_checksum(with_euro);
    let h_pound = Baseline::body_checksum(with_pound);
    assert_ne!(
        h_euro, h_pound,
        "different non-ASCII chars must produce different hashes"
    );

    // The same message hashed twice must produce the same hash (the
    // function is deterministic). The timestamp inside must still
    // be stripped, so this also hashes equal to the same message at
    // a different timestamp.
    let with_euro_2 = "cost was €5 at 2026-05-25T09:30:11";
    let h_euro_2 = Baseline::body_checksum(with_euro_2);
    assert_eq!(
        h_euro, h_euro_2,
        "timestamps must still be stripped from non-ASCII strings"
    );

    // Pin the exact hash so we detect any regression in the normalization
    // pipeline. normalize_body for "cost was €5 at 2026-05-24T12:00:00":
    //   1. strip_ansi — no change
    //   2. strip_timestamps — removes "2026-05-24T12:00:00", leaving "cost was €5 at "
    //   3. lines().map(trim_end).join("\n") — trims trailing space -> "cost was €5 at"
    // sha256("cost was €5 at") = 09d918d8b04de137d6886c8d5cb5c3226c282d3d49bd5c3840a31b51198c9bfd
    //
    // A byte-cast bug would inflate €'s bytes (E2 82 AC) into three separate
    // Unicode scalars (U+00E2, U+0082, U+00AC) and yield a different hash;
    // pinning the exact value catches regressions in either direction.
    assert_eq!(
        h_euro, "09d918d8b04de137d6886c8d5cb5c3226c282d3d49bd5c3840a31b51198c9bfd",
        "body_checksum must hash the true UTF-8 encoding of €, not per-byte scalars"
    );
}
