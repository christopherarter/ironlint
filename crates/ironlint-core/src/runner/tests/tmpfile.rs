use super::*;
use crate::engine::InternalReason;
use crate::runner::{materialize_tmpfile, sweep_stale_tmpfiles, CheckInput, IronLintEngine};
use crate::verdict::Status;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

// --- IRONLINT_TMPFILE materialization ---

#[test]
fn tmpfile_materialized_with_content_ext_and_cleaned() {
    let dir = TempDir::new().unwrap();
    // Check copies $IRONLINT_TMPFILE to a stable capture path, asserts the .rs ext, then passes.
    write_config(&dir,
        "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"case \\\"$IRONLINT_TMPFILE\\\" in *.rs) cat \\\"$IRONLINT_TMPFILE\\\" > \\\"$IRONLINT_ROOT/captured.txt\\\"; exit 0;; *) exit 2;; esac\"\n");
    let engine = load_with_event(&dir, "write");
    let path = dir.path().join("src").join("a.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "OLD").unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: path.clone(),
            content: "PROPOSED-NEW".to_string(),
        })
        .unwrap();
    assert_eq!(report.verdict.status, Status::Pass);
    // The captured bytes are the PROPOSED content (not the OLD on-disk bytes).
    assert_eq!(
        std::fs::read_to_string(dir.path().join("captured.txt")).unwrap(),
        "PROPOSED-NEW"
    );
    // The temp file is gone (cleanup), but its sibling source file remains.
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
}

#[test]
fn tmpfile_not_created_when_unreferenced() {
    let dir = TempDir::new().unwrap();
    write_config(
        &dir,
        "checks:\n  g:\n    files: \"**/*.rs\"\n    run: \"! grep -q TODO\"\n",
    );
    let engine = load_with_event(&dir, "write");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fine").unwrap();
    let _ = engine
        .check_with_explain(CheckInput::File {
            path: path.clone(),
            content: "fine".into(),
        })
        .unwrap();
    let any_tmp = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"));
    assert!(
        !any_tmp,
        "no temp file should exist for an unreferenced check"
    );
}

#[test]
fn tmpfile_unset_on_pre_commit() {
    let dir = TempDir::new().unwrap();
    // On pre-commit the var must be empty even though the check references it.
    write_config(&dir, "checks:\n  pc:\n    files: \"**/*.rs\"\n    on: [pre-commit]\n    run: \"test -z \\\"$IRONLINT_TMPFILE\\\"\"\n");
    let engine = load_with_event(&dir, "pre-commit");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "x").unwrap();
    let verdict = engine.check_set(&[path]).unwrap();
    assert_eq!(
        verdict.status,
        Status::Pass,
        "IRONLINT_TMPFILE must be unset on pre-commit"
    );
}

#[test]
#[cfg(unix)]
fn tmpfile_write_failure_is_internal_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    write_config(
        &dir,
        "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\"\"\n",
    );
    let engine = load_with_event(&dir, "write");
    let sub = dir.path().join("ro");
    std::fs::create_dir(&sub).unwrap();
    let path = sub.join("a.rs");
    std::fs::write(&path, "x").unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    let verdict = engine
        .check(CheckInput::File {
            path,
            content: "x".into(),
        })
        .unwrap();
    // restore perms so TempDir cleanup works
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(verdict.status, Status::InternalError);
}

#[test]
#[cfg(unix)]
fn tmpfile_is_created_exclusive_mode_0600() {
    use std::os::unix::fs::MetadataExt; // for .mode() — reading bits, not constructing

    // Direct call to materialize_tmpfile: the file's mode is unobservable
    // through the end-to-end path because TmpFileGuard::drop removes it
    // after the check runs (locked by the existing tmpfile-* tests). This
    // test locks only the new perms/exclusivity contract.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ironlint-tmp-probe.txt");
    let _ = std::fs::remove_file(&path);

    materialize_tmpfile(&path, "body").expect("materialize probe");

    let mode = std::fs::metadata(&path).expect("metadata").mode();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        mode & 0o777,
        0o600,
        "tmpfile must be mode 0600, got {:o}",
        mode & 0o777
    );

    // Exclusivity (create_new/O_EXCL): a second create at the same path
    // must fail rather than clobber — the symlink-race fail-closed.
    std::fs::write(&path, "attacker").unwrap(); // pre-create the name
    let second = materialize_tmpfile(&path, "body");
    let _ = std::fs::remove_file(&path);
    assert!(
        second.is_err(),
        "materialize_tmpfile must fail (not clobber) when the path already exists"
    );
}

#[test]
fn tmpfile_refuses_to_write_outside_project_root() {
    // Config dir A; separate tempdir B simulates an out-of-project path.
    // resolve_input_path bypasses its containment guard when the target
    // file doesn't exist yet (pre-write). maybe_materialize_tmpfile must
    // catch this and refuse to write the tmpfile.
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    write_config(
        &dir_a,
        "checks:\n  chk:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\"\"\n",
    );
    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: std::iter::once("chk".to_string()).collect(),
            event: "write".to_string(),
            allow_external_paths: false,
            force: true,
        })
        .load(&dir_a.path().join(".ironlint.yml"))
        .unwrap();
    // Target does NOT exist — triggers the bypass branch in resolve_input_path.
    let evil = dir_b.path().join("evil.rs");
    let verdict = engine
        .check(CheckInput::File {
            path: evil,
            content: "x".into(),
        })
        .unwrap();
    assert_eq!(
        verdict.status,
        Status::InternalError,
        "should refuse to materialize tmpfile outside project root"
    );
    // No ironlint-tmp-* file should have been written in dir_b.
    let leaked: Vec<_> = std::fs::read_dir(dir_b.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"))
        .collect();
    assert!(
        leaked.is_empty(),
        "tmpfile leaked into external dir: {leaked:?}"
    );
}

#[test]
fn tmpfile_allows_outside_write_with_allow_external_paths() {
    // Same topology as above, but allow_external_paths: true. The tmpfile
    // should be written, the check should run and see the proposed content,
    // and cleanup should leave no ironlint-tmp-* in dir_b.
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    // Check copies $IRONLINT_TMPFILE content to a capture path inside $IRONLINT_ROOT.
    write_config(
        &dir_a,
        "checks:\n  chk:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\" > \\\"$IRONLINT_ROOT/captured.txt\\\"\"\n",
    );
    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: std::iter::once("chk".to_string()).collect(),
            event: "write".to_string(),
            allow_external_paths: true,
            force: true,
        })
        .load(&dir_a.path().join(".ironlint.yml"))
        .unwrap();
    let evil = dir_b.path().join("evil.rs");
    let verdict = engine
        .check(CheckInput::File {
            path: evil,
            content: "proposed".into(),
        })
        .unwrap();
    assert_eq!(
        verdict.status,
        Status::Pass,
        "allow_external_paths=true should permit the tmpfile write"
    );
    // The check captured the proposed content via $IRONLINT_TMPFILE.
    let captured = std::fs::read_to_string(dir_a.path().join("captured.txt")).unwrap();
    assert_eq!(captured, "proposed");
    // The tmpfile was cleaned up — no ironlint-tmp-* leftover in dir_b.
    let leaked: Vec<_> = std::fs::read_dir(dir_b.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ironlint-tmp-"))
        .collect();
    assert!(
        leaked.is_empty(),
        "tmpfile leaked after cleanup: {leaked:?}"
    );
}

#[test]
fn force_runs_out_of_scope_named_check() {
    let dir = TempDir::new().unwrap();
    write_config(
        &dir,
        "checks:\n  only-src:\n    files: \"src/**/*.rs\"\n    run: \"! grep -q BAD\"\n",
    );
    // File path is OUTSIDE the src/**/*.rs glob.
    let path = dir.path().join("fixtures").join("x.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "BAD").unwrap();
    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: std::iter::once("only-src".to_string()).collect(),
            event: "write".to_string(),
            allow_external_paths: false,
            force: true,
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path,
            content: "BAD".into(),
        })
        .unwrap();
    // Without force it would be skipped out_of_scope → Pass. With force it fires → Block.
    assert_eq!(report.verdict.status, Status::Block);
}

#[test]
fn force_does_not_bypass_disable_directive() {
    let dir = TempDir::new().unwrap();
    write_config(
        &dir,
        "checks:\n  only-src:\n    files: \"src/**/*.rs\"\n    run: \"! grep -q BAD\"\n",
    );
    let path = dir.path().join("x.rs");
    std::fs::write(&path, "BAD").unwrap();
    let engine = IronLintEngine::builder()
        .with_options(CheckOptions {
            checks: std::iter::once("only-src".to_string()).collect(),
            event: "write".to_string(),
            allow_external_paths: false,
            force: true,
        })
        .load(&dir.path().join(".ironlint.yml"))
        .unwrap();
    // Inline disable suppresses the check even under --force.
    let content = "BAD\n// ironlint-disable: only-src\n".to_string();
    let report = engine
        .check_with_explain(CheckInput::File { path, content })
        .unwrap();
    assert_eq!(report.verdict.status, Status::Pass);
}

// --- Task 2.5: stale $IRONLINT_TMPFILE sweep ---

/// Backdate `path`'s mtime `secs_ago` seconds into the past, to simulate
/// a tmpfile leaked by a run that was killed well before "now".
fn backdate_mtime(path: &Path, secs_ago: u64) {
    let old = SystemTime::now() - Duration::from_secs(secs_ago);
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
}

#[test]
fn sweep_stale_tmpfiles_removes_only_old_matching_files() {
    let dir = TempDir::new().unwrap();

    // Stale: matches the tmpfile naming prefix, mtime well past the
    // threshold — this is the leaked-file case the sweep exists for.
    let stale = dir.path().join("ironlint-tmp-1111-0-1.rs");
    std::fs::write(&stale, "leaked").unwrap();
    backdate_mtime(&stale, 2 * 60 * 60); // 2h ago

    // Fresh: matches the naming prefix but is recent — could be a
    // concurrently-running ironlint process's still-live tmpfile. Must
    // survive the sweep.
    let fresh = dir.path().join("ironlint-tmp-2222-0-2.rs");
    std::fs::write(&fresh, "still live").unwrap();

    // Unrelated: old, but the name doesn't match the tmpfile prefix.
    // Must never be touched, regardless of age.
    let unrelated = dir.path().join("real_file.rs");
    std::fs::write(&unrelated, "keep me").unwrap();
    backdate_mtime(&unrelated, 2 * 60 * 60);

    sweep_stale_tmpfiles(dir.path(), Duration::from_secs(60 * 60));

    assert!(!stale.exists(), "stale ironlint-tmp-* file must be swept");
    assert!(
        fresh.exists(),
        "fresh ironlint-tmp-* file must survive (may be a concurrent live run)"
    );
    assert!(
        unrelated.exists(),
        "non-tmpfile-pattern files must never be touched, regardless of age"
    );
}

#[test]
fn sweep_stale_tmpfiles_ignores_directories_matching_the_prefix() {
    let dir = TempDir::new().unwrap();
    let weird_dir = dir.path().join("ironlint-tmp-a-dir");
    std::fs::create_dir(&weird_dir).unwrap();

    // Even with an effectively-zero threshold (everything old enough to
    // count as stale), a directory is never a sweep candidate.
    sweep_stale_tmpfiles(dir.path(), Duration::from_secs(0));

    assert!(
        weird_dir.exists(),
        "a directory matching the tmpfile prefix must never be removed"
    );
}

#[test]
fn sweep_stale_tmpfiles_tolerates_missing_root() {
    // Best-effort: a root that doesn't exist (or vanished) must not panic.
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("does-not-exist");
    sweep_stale_tmpfiles(&missing, Duration::from_secs(60 * 60));
}

#[test]
fn maybe_materialize_tmpfile_sweeps_stale_leaks_in_its_own_nested_dir() {
    // $IRONLINT_TMPFILE is materialized as a SIBLING of the checked file
    // (its own directory), which for real source is nested — e.g.
    // crates/foo/src/ — not the config root. The load-time
    // sweep_stale_tmpfiles call only sweeps config_dir's immediate
    // entries, so it never reaches a leak sitting here. This drives the
    // real, end-to-end reclaim path (through maybe_materialize_tmpfile,
    // via a full check dispatch) and proves the nested leak is gone.
    let dir = TempDir::new().unwrap();
    write_config(
        &dir,
        "checks:\n  cap:\n    files: \"**/*.rs\"\n    run: \"cat \\\"$IRONLINT_TMPFILE\\\" > /dev/null\"\n",
    );
    let nested = dir.path().join("crates").join("foo").join("src");
    std::fs::create_dir_all(&nested).unwrap();

    // Stale: leaked ironlint-tmp-* file sitting in the checked file's
    // own (nested) directory, from a run killed well before "now" — the
    // exact leak the root-only load-time sweep misses.
    let stale = nested.join("ironlint-tmp-9999-0-1.rs");
    std::fs::write(&stale, "leaked").unwrap();
    backdate_mtime(&stale, 2 * 60 * 60); // 2h ago

    // Fresh: matches the naming prefix but is recent — could be a
    // concurrently-running ironlint process's still-live tmpfile in the
    // same directory. Must survive the sweep.
    let fresh = nested.join("ironlint-tmp-8888-0-2.rs");
    std::fs::write(&fresh, "still live").unwrap();

    // Unrelated: old, but the name doesn't match the tmpfile prefix.
    // Must never be touched, regardless of age.
    let unrelated = nested.join("lib.rs");
    std::fs::write(&unrelated, "keep me").unwrap();
    backdate_mtime(&unrelated, 2 * 60 * 60);

    let engine = load_with_event(&dir, "write");
    let path = nested.join("a.rs");
    std::fs::write(&path, "OLD").unwrap();
    let report = engine
        .check_with_explain(CheckInput::File {
            path: path.clone(),
            content: "PROPOSED".to_string(),
        })
        .unwrap();
    assert_eq!(report.verdict.status, Status::Pass);

    assert!(
        !stale.exists(),
        "stale ironlint-tmp-* file in the checked file's nested dir must be swept at materialization time"
    );
    assert!(
        fresh.exists(),
        "fresh ironlint-tmp-* file in the nested dir must survive (may be a concurrent live run)"
    );
    assert!(
        unrelated.exists(),
        "non-tmpfile-pattern files in the nested dir must never be touched, regardless of age"
    );
}

#[test]
fn detail_for_truncates_multibyte_run_at_char_boundary() {
    // A run command > MAX_RUN_LEN (80 bytes) whose byte-80 position lands
    // inside a multibyte UTF-8 codepoint. The naive `&run[..80]` byte
    // slice panics here; the truncation must step back to the nearest
    // char boundary so the detail string is valid UTF-8 (and the
    // InternalError path doesn't panic instead of returning a verdict).
    // 78 ASCII bytes, then a 4-byte emoji (🚀) straddling bytes 78..82,
    // so byte 80 falls mid-codepoint.
    let run = "#".repeat(78) + "🚀" + "tail-here";
    assert!(
        run.len() > 80,
        "fixture must exceed the 80-byte truncation limit; got {}",
        run.len()
    );
    let detail =
        IronLintEngine::detail_for(&InternalReason::NotFound, &run, Duration::from_secs(30));
    // Must not panic (the slice would have), must end in the ellipsis,
    // and the prefix must be valid UTF-8 ending on a char boundary.
    assert!(
        detail.ends_with('…'),
        "truncated detail must end in ellipsis; got: {detail:?}"
    );
    let body = detail.strip_suffix('…').unwrap();
    let truncated = body.strip_prefix("not_found running: ").unwrap();
    // Truncated portion must be ≤80 bytes AND valid UTF-8 (char-aligned).
    assert!(
        truncated.len() <= 80,
        "truncated run must be ≤80 bytes; got {} ({truncated:?})",
        truncated.len()
    );
    assert!(
        truncated.chars().all(|_| true),
        "truncated run must be valid UTF-8 (char-boundary-aligned)"
    );
    // The emoji must NOT appear at the cut — it straddled the boundary.
    assert!(!truncated.contains('🚀'),);
}
