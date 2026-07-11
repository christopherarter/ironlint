//! Bug 1: env-manifest overlay so multi-file patches can't sneak forbidden imports.
//!
//! When a single `apply_patch` adds two files atomically — e.g. adds
//! `src/data/db.ts` AND modifies `src/components/App.tsx` to import from it —
//! the hook checks `App.tsx` BEFORE `db.ts` is on disk. The arch subprocess
//! builds the dependency graph from disk, so the import resolves to `None`
//! and the edge is silently dropped — a false negative (under-block).
//!
//! The fix threads a manifest of ALL sibling proposed files through the
//! check ABI (`$IRONLINT_PROPOSED_MANIFEST`) so the arch subprocess can
//! merge them as VIRTUAL graph nodes before resolving the proposed file's
//! outgoing imports.

use ironlint_core::arch::config::ArchConfig;
use ironlint_core::arch::engine::{ArchEngine, ArchOutcome};
use std::fs;
use std::path::{Path, PathBuf};

fn forbidden_config() -> ArchConfig {
    serde_yaml::from_str(
        "layers:\n  - name: presentation\n    globs: [\"src/components/**\"]\n  - name: data\n    globs: [\"src/data/**\"]\nrules:\n  - from: presentation\n    may_import: []\n",
    )
    .unwrap()
}

/// Write a manifest file mapping each `path` to its content file. Returns the
/// manifest path. Tab-separated, one `ABSPATH\tCONTENTFILE` per line.
fn write_manifest(dir: &Path, entries: &[(PathBuf, &str)]) -> PathBuf {
    let manifest_path = dir.join("manifest.tsv");
    let mut lines = Vec::new();
    for (path, content) in entries {
        let content_file = dir.join(format!(
            "content-{}.txt",
            path.file_stem().unwrap().to_str().unwrap()
        ));
        fs::write(&content_file, content).unwrap();
        lines.push(format!("{}\t{}", path.display(), content_file.display()));
    }
    fs::write(&manifest_path, lines.join("\n") + "\n").unwrap();
    manifest_path
}

/// The capstone scenario: a single atomic patch adds `src/data/db.ts` AND
/// modifies `src/components/App.tsx` to `import { db } from '../data/db'`.
/// `db.ts` is NOT on disk yet (pre-patch state). Without the manifest, the
/// import resolves to `None` and the violation passes silently. With the
/// manifest, `db.ts` is merged as a virtual node and the violation is caught.
#[test]
fn check_write_catches_forbidden_import_via_manifest_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    // db.ts is intentionally NOT on disk — simulates the pre-patch state.
    let db_path = root.join("src/data/db.ts");
    let app_path = root.join("src/components/App.tsx");
    let manifest_path = write_manifest(root, &[(db_path.clone(), "export const db = 1;\n")]);

    let content = b"import { db } from '../data/db';\n";
    let outcome = ArchEngine::check_write(
        root,
        &forbidden_config(),
        &app_path,
        content,
        Some(&manifest_path),
    );

    match outcome {
        ArchOutcome::Block { violations } => {
            assert_eq!(violations.len(), 1, "should catch the forbidden import");
            assert_eq!(violations[0].rule_from, "presentation");
        }
        other => panic!("expected Block, got {other:?} — forbidden import sneaked through"),
    }
}

/// Without the manifest, the same scenario passes (false negative). This
/// pins the bug's existence and proves the fix is what closes it.
#[test]
fn check_write_false_negative_without_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    // db.ts is NOT on disk.
    let app_path = root.join("src/components/App.tsx");
    let content = b"import { db } from '../data/db';\n";

    let outcome = ArchEngine::check_write(root, &forbidden_config(), &app_path, content, None);

    assert!(
        matches!(outcome, ArchOutcome::Pass),
        "without manifest, the forbidden import should sneak through (false negative): {outcome:?}"
    );
}

/// A manifest entry for a file that IS already on disk: the proposed
/// (manifest) version OVERRIDES the disk version. This is the correct
/// behavior — the proposed content is what will be true after the patch.
#[test]
fn manifest_proposed_overrides_disk_version() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    // db.ts IS on disk, but with DIFFERENT content — the manifest version
    // should override it.
    let db_path = root.join("src/data/db.ts");
    fs::write(&db_path, "// old content\n").unwrap();
    let app_path = root.join("src/components/App.tsx");
    let manifest_path = write_manifest(root, &[(db_path.clone(), "export const db = 1;\n")]);

    let content = b"import { db } from '../data/db';\n";
    let outcome = ArchEngine::check_write(
        root,
        &forbidden_config(),
        &app_path,
        content,
        Some(&manifest_path),
    );

    assert!(
        matches!(outcome, ArchOutcome::Block { .. }),
        "manifest-proposed db.ts should override disk version and resolve the import: {outcome:?}"
    );
}

/// Manifest with a missing content file: best-effort skip, doesn't crash.
/// The entry is silently dropped — the target won't resolve, matching the
/// pre-fix behavior (status quo for that edge).
#[test]
fn manifest_with_missing_content_file_skips_entry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    let db_path = root.join("src/data/db.ts");
    let app_path = root.join("src/components/App.tsx");

    // Manifest references a content file that doesn't exist.
    let manifest_path = root.join("manifest.tsv");
    fs::write(
        &manifest_path,
        format!(
            "{}\t{}\n",
            db_path.display(),
            root.join("nonexistent-content.txt").display()
        ),
    )
    .unwrap();

    let content = b"import { db } from '../data/db';\n";
    let outcome = ArchEngine::check_write(
        root,
        &forbidden_config(),
        &app_path,
        content,
        Some(&manifest_path),
    );

    assert!(
        matches!(outcome, ArchOutcome::Pass),
        "missing content file → skip entry → import unresolved → Pass (best-effort): {outcome:?}"
    );
}

/// Manifest with a blank line: should be skipped without error.
#[test]
fn manifest_with_blank_lines_is_tolerant() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    let db_path = root.join("src/data/db.ts");
    let app_path = root.join("src/components/App.tsx");

    let content_file = root.join("db-content.txt");
    fs::write(&content_file, "export const db = 1;\n").unwrap();
    let manifest_path = root.join("manifest.tsv");
    fs::write(
        &manifest_path,
        format!("\n\n{}\t{}\n\n", db_path.display(), content_file.display()),
    )
    .unwrap();

    let content = b"import { db } from '../data/db';\n";
    let outcome = ArchEngine::check_write(
        root,
        &forbidden_config(),
        &app_path,
        content,
        Some(&manifest_path),
    );

    assert!(
        matches!(outcome, ArchOutcome::Block { .. }),
        "blank lines should be tolerated, db.ts merged: {outcome:?}"
    );
}

/// Empty manifest: no entries → behaves like None (no overlay).
#[test]
fn empty_manifest_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/data")).unwrap();
    // db.ts on disk — the import resolves without the manifest.
    fs::write(root.join("src/data/db.ts"), "export const db = 1;\n").unwrap();
    let app_path = root.join("src/components/App.tsx");
    let manifest_path = root.join("manifest.tsv");
    fs::write(&manifest_path, "").unwrap();

    let content = b"import { db } from '../data/db';\n";
    let outcome = ArchEngine::check_write(
        root,
        &forbidden_config(),
        &app_path,
        content,
        Some(&manifest_path),
    );

    assert!(
        matches!(outcome, ArchOutcome::Block { .. }),
        "empty manifest → no overlay → disk db.ts resolves → Block: {outcome:?}"
    );
}
