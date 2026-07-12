use crate::config::Check;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Removes its temp file on drop — covers normal return, error, timeout, and
/// panic-unwind. `Drop` does **not** run across a hard process kill: a
/// `SIGTERM`/`SIGINT`/`SIGKILL` that ends ironlint mid-check (harnesses
/// routinely do this on their own hook timeout budget, not just on a manual
/// `kill -9`) skips this destructor entirely and leaves the file sitting in
/// the checked file's own directory — which for real source is nested (e.g.
/// `crates/foo/src/`), not the config root. Two `sweep_stale_tmpfiles`
/// backstops reclaim that leak once it's older than its age threshold:
/// `maybe_materialize_tmpfile` sweeps the tmpfile's own parent directory
/// immediately before writing a new tmpfile there, so a later check against
/// any file in that same directory reclaims the leak; the sweep at engine
/// `load()` additionally covers leaks sitting directly in the config root.
pub(crate) struct TmpFileGuard {
    pub(crate) path: PathBuf,
}

impl Drop for TmpFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// True iff any of the check's steps reference the `$IRONLINT_TMPFILE` token.
pub(crate) fn check_references_tmpfile(check: &Check) -> bool {
    check
        .effective_steps()
        .iter()
        .any(|s| s.run.contains("IRONLINT_TMPFILE"))
}

/// True iff any of the check's steps reference the `$IRONLINT_ARCH_LAYERS`
/// token.
pub(crate) fn check_references_arch_layers(check: &Check) -> bool {
    check
        .effective_steps()
        .iter()
        .any(|s| s.run.contains("IRONLINT_ARCH_LAYERS"))
}

/// The naming prefix ironlint gives every `$IRONLINT_TMPFILE` it
/// materializes (see `unique_tmp_name`).
pub(crate) const TMPFILE_PREFIX: &str = "ironlint-tmp-";

/// The naming prefix for the `$IRONLINT_ARCH_LAYERS` materialized file.
/// These live in the system temp directory, not the project tree.
pub(crate) const ARCH_LAYERS_PREFIX: &str = "ironlint-arch-";

/// Prefixes that `sweep_stale_tmpfiles` may reclaim. Both `$IRONLINT_TMPFILE`
/// and `$IRONLINT_ARCH_LAYERS` leaks can be left behind when ironlint is
/// SIGKILLed mid-check and `TmpFileGuard::drop` does not run.
pub(crate) const STALE_TMPFILE_PREFIXES: &[&str] = &[TMPFILE_PREFIX, ARCH_LAYERS_PREFIX];

/// A collision-resistant temp-file name with `prefix` and optional `ext`.
pub(crate) fn unique_name(prefix: &str, ext: Option<&str>) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    match ext {
        Some(e) => format!("{prefix}{pid}-{n}-{nanos}.{e}"),
        None => format!("{prefix}{pid}-{n}-{nanos}"),
    }
}

/// A collision-resistant temp-file name mirroring `ext` (no `rng` dependency).
pub(crate) fn unique_tmp_name(ext: Option<&str>) -> String {
    unique_name(TMPFILE_PREFIX, ext)
}

/// Default age threshold for `sweep_stale_tmpfiles`, used both at engine
/// `load()` (config-root sweep) and at tmpfile materialization time (sweep
/// of the tmpfile's own parent directory — see `maybe_materialize_tmpfile`).
/// A tmpfile older than this is assumed to be a leak from a killed prior run
/// rather than one still in flight: an in-progress check's own wall-clock
/// cap is `execution.timeout_secs` (default 30s, see `resolve_timeout`), so
/// an hour of headroom keeps a live tmpfile — even from a slow,
/// concurrently-running ironlint process — comfortably below the threshold
/// and never a sweep target. This is what makes it safe to run the sweep
/// unconditionally rather than coordinating with other in-flight processes:
/// age, not identity, is the only signal, and a generous threshold keeps
/// false positives effectively impossible. That safety margin holds only
/// while the configured per-check timeout (`execution.timeout_secs`) stays
/// well under this 1h threshold — a check configured with an hours-long
/// timeout would put its own still-live tmpfile at risk of being swept as a
/// leak.
pub(crate) const TMPFILE_SWEEP_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Best-effort reclaim of `$IRONLINT_TMPFILE` and `$IRONLINT_ARCH_LAYERS`
/// leaks in `root`'s immediate directory: removes only regular files whose
/// name starts with one of [`STALE_TMPFILE_PREFIXES`] and whose mtime is older
/// than `max_age`. Never touches anything else — a non-matching name, a
/// directory, or an unreadable/racing entry is simply skipped rather than
/// erroring, since this runs unconditionally on every engine load and a sweep
/// failure must never block a check.
///
/// Deliberately shallow (does not recurse into subdirectories): a tmpfile
/// for a nested checked file lands in that file's own directory (see
/// `maybe_materialize_tmpfile`), which this pass does not visit. `load()` is
/// a hot path — it can run once per agent write — so the sweep is bounded to
/// `root`'s own entries rather than paying an O(repo size) walk on every
/// invocation.
pub(crate) fn sweep_stale_tmpfiles(root: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if is_stale_tmpfile(&entry, now, max_age) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// True iff `entry` is a regular file matching one of
/// [`STALE_TMPFILE_PREFIXES`] whose mtime is older than `max_age` relative to
/// `now`. Extracted from `sweep_stale_tmpfiles` to keep both functions well
/// under the cognitive complexity cap.
fn is_stale_tmpfile(entry: &std::fs::DirEntry, now: SystemTime, max_age: Duration) -> bool {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        return false;
    };
    if !STALE_TMPFILE_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    let Ok(metadata) = entry.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    // `Err` means `modified` is in the future relative to `now` (clock skew
    // or a filesystem with coarse/odd mtime semantics) — treat as fresh, not
    // stale, rather than guessing.
    now.duration_since(modified)
        .map(|age| age > max_age)
        .unwrap_or(false)
}

/// Create the `$IRONLINT_TMPFILE` at `path` with `content`, using an
/// exclusive (`O_EXCL` / `create_new`) open at mode `0600` on unix.
///
/// Exclusive create makes a symlink race fail-closed: if `path` already
/// exists (attacker pre-creates it), the open errors instead of clobbering.
/// Mode `0600` keeps the briefly-written proposed content unreadable by
/// co-located processes (the old `std::fs::write` left it world-readable
/// ~`0o644`). On non-unix the mode bit is unavailable; fall back to a plain
/// exclusive create + write (the name is still collision-resistant).
pub(crate) fn materialize_tmpfile(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }
}
