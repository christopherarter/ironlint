# Hector Trust Store (Plan 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dormant in-YAML trust fingerprint with a real, out-of-repo allow-list (the direnv model): `hector check` refuses to run any gate until the config **and** its gate scripts have been blessed *on this machine* via `hector trust`.

**Architecture:** A new `hector-core::trust` module computes a sha256 over the config bytes plus every file under `.hector/gates/` (sorted by relative path), and reads/writes a blessed-hash store at `~/.config/hector/trust.json` keyed by the config's canonical absolute path. Enforcement lives in the **CLI `check` command** (`commands/check.rs`) — it calls `trust::ensure_trusted(config)` before building the engine and fails closed (exit 1) on mismatch/missing. Core `HectorEngine::load` stays pure, so core unit tests are unaffected; only CLI tests that invoke `check` bless first. `hector trust` writes the blessed hash; `hector init` auto-blesses what it scaffolds. Home-dir resolution is hand-rolled XDG (`$XDG_CONFIG_HOME` → `$HOME/.config`) — no new dependency, and `$XDG_CONFIG_HOME` doubles as the test redirect hook.

**Tech Stack:** Rust, `sha2` (already a dep), `serde`/`serde_json` (already deps), `chrono` (already a dep, for `blessed_at` timestamps). No new crates.

## Global Constraints

Copied verbatim from `specs/2026-06-15-hector-gates-redesign-design.md` §6 and `AGENTS.md`. Every task's requirements implicitly include this section.

- **Trust store path:** `~/.config/hector/trust.json`, XDG-respecting (`$XDG_CONFIG_HOME` overrides `$HOME/.config`). Keyed by the **canonical absolute path** of `.hector.yml`.
- **Trust hash:** sha256 over the config file bytes **plus the bytes of every file under `.hector/gates/`** (sorted by relative path). Any change to any gate script invalidates trust. Limitation (documented, not fixed): trust covers the config and `.hector/gates/` only; a `run` that sources scripts elsewhere, or an `extends:` parent config, is the author's responsibility.
- **Fail closed:** on mismatch or missing entry, refuse to run any gate, **outer exit code `1`** (config/load-error tier), message `config/gates not trusted — review and run \`hector trust\``.
- **No interactive prompt.** Blessing is always the explicit `hector trust` action (the hook has no human). `hector init` auto-blesses the config it just wrote.
- **Enforcement site:** CLI `check` only. Read-only commands (`validate`, `explain`, `show-resolved-config`, `doctor`) do **not** enforce — they never run gates. (Doctor *reporting* trust status is deferred to Plan 3.)
- **Outer exit codes (unchanged, adapters depend on them):** `0` pass, `1` config/load error (now includes untrusted), `2` block, `3` internal error.
- **No new heavy dependencies** — hand-roll XDG resolution.
- **Repo rules:** bug fixes start with a failing test (TDD: write failing test → verify red → minimal impl → verify green → commit). Every touched `crates/*/src/` file must reach ≥80% region coverage. Cognitive complexity ≤15 per function (clippy). `Cargo.lock` is gitignored — do not commit. Binary is `hector`. `cargo fmt` + `cargo clippy --all-targets -- -D warnings` clean before each commit.

## Out of scope (later plans)

- `hector verify` (dynamic proof) and the full `doctor` expansion incl. trust-status reporting — **Plan 3**.
- Adapter `--event`/ABI side — **Plan 4**.
- File-locking the trust store (`fs4`) for concurrent writers — atomic temp+rename is sufficient for 0.3; revisit only if it bites.

## File Structure

- **`crates/hector-core/src/trust.rs`** — *replaced wholesale*. Old API (`canonicalize_for_fingerprint`, `fingerprint`, `verify`, `write_trust_block`, the anchor/alias scanner, `yaml_to_json`, `sort_json_keys`) is deleted. New API: `compute_hash`, the `TrustStore`/`TrustEntry` types, `read_store`/`write_store`, `ensure_trusted_in`/`bless_in` (testable cores), and the thin `trust_store_path`/`ensure_trusted`/`bless` wrappers.
- **`crates/hector-core/tests/trust_canonical_json.rs`** — *deleted* (pins the removed legacy API; would break compilation).
- **`crates/hector-cli/src/commands/trust.rs`** — stub → real bless via `trust::bless`.
- **`crates/hector-cli/src/commands/check.rs`** — add the `ensure_trusted` gate at the top of `run()`.
- **`crates/hector-cli/src/commands/init/mod.rs`** — auto-bless after writing the config.
- **`crates/hector-cli/src/cli.rs`** — fix the stale `Trust` help string.
- **`crates/hector-cli/tests/common/mod.rs`** — *new* shared `bless()` helper for e2e tests.
- **`crates/hector-cli/tests/cli_e2e_trust.rs`** — *new* enforcement e2e tests.
- Existing CLI test files invoking `check` (`cli_check_external_paths.rs`, `cli_check_single_load.rs`, `cli_e2e_check_quiet_stderr.rs`, `cli_e2e_gates.rs`, `cli_runner_telemetry_failure.rs`, `cli_version.rs`) — bless first + thread `XDG_CONFIG_HOME`.
- Docs: `AGENTS.md`, `plans/README.md`, `CHANGELOG.md` (if present), memory.

---

### Task 1: Replace `trust.rs` with `compute_hash` over config + gates dir

**Files:**
- Modify (replace wholesale): `crates/hector-core/src/trust.rs`
- Delete: `crates/hector-core/tests/trust_canonical_json.rs`
- Test: inline `#[cfg(test)] mod tests` in `trust.rs`

**Interfaces:**
- Produces: `pub fn compute_hash(config_path: &Path) -> anyhow::Result<String>` — returns `"sha256:<hex>"`. Used by Tasks 3.

- [ ] **Step 1: Confirm the old API is dead before deleting it**

Run: `grep -rn "canonicalize_for_fingerprint\|trust::fingerprint\|trust::verify\|write_trust_block" crates/ --include='*.rs'`
Expected: the only hits are in `crates/hector-core/src/trust.rs` itself and `crates/hector-core/tests/trust_canonical_json.rs`. If any *other* file references them, stop and report (the plan assumed trust.rs is unwired). The CLI `trust` stub does **not** call these (verify with the same grep).

- [ ] **Step 2: Delete the dormant legacy test**

```bash
git rm crates/hector-core/tests/trust_canonical_json.rs
```

- [ ] **Step 3: Write the failing tests for `compute_hash`**

Replace the entire body of `crates/hector-core/src/trust.rs` test module with these (and leave the impl empty for now so they fail to compile/assert):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn hash_is_deterministic_and_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let a = compute_hash(&cfg).unwrap();
        let b = compute_hash(&cfg).unwrap();
        assert_eq!(a, b, "same inputs must hash identically");
        assert!(a.starts_with("sha256:"), "hash must be sha256-prefixed: {a}");
    }

    #[test]
    fn editing_config_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        let before = compute_hash(&cfg).unwrap();
        write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"false\"\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(before, after, "a config edit must invalidate the hash");
    }

    #[test]
    fn editing_a_gate_script_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \".hector/gates/g.sh\"\n");
        let script = dir.path().join(".hector/gates/g.sh");
        write(&script, "#!/bin/sh\nexit 0\n");
        let before = compute_hash(&cfg).unwrap();
        write(&script, "#!/bin/sh\nexit 2\n");
        let after = compute_hash(&cfg).unwrap();
        assert_ne!(before, after, "a gate-script edit must invalidate the hash");
    }

    #[test]
    fn hash_is_independent_of_filesystem_enumeration_order() {
        // Two gate files; the hash must fold them in sorted-relative-path order,
        // not in whatever order the OS yields. Assert by recomputing — stable.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(&cfg, "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        write(&dir.path().join(".hector/gates/a.sh"), "a\n");
        write(&dir.path().join(".hector/gates/b.sh"), "b\n");
        let first = compute_hash(&cfg).unwrap();
        let second = compute_hash(&cfg).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn missing_gates_dir_hashes_only_the_config() {
        // No .hector/gates/ at all — must succeed (not error), hashing config alone.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".hector.yml");
        write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n");
        assert!(compute_hash(&cfg).unwrap().starts_with("sha256:"));
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p hector-core --lib trust::`
Expected: FAIL — `compute_hash` not found (compile error).

- [ ] **Step 5: Write the `compute_hash` implementation**

Replace the **non-test** portion of `crates/hector-core/src/trust.rs` with:

```rust
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Feed one labeled blob into the hasher with length prefixes on both the
/// label and the content, so no two distinct (label, bytes) pairs can collide
/// by concatenation.
fn hash_entry(hasher: &mut Sha256, label: &str, bytes: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Recursively collect `(relative-path, bytes)` for every file under `dir`,
/// with `/`-separated relative paths for cross-platform determinism.
fn collect_gate_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    collect_into(dir, dir, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect_into(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_into(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

/// Compute the trust hash of a config: sha256 over the config file bytes plus
/// every file under `<config-dir>/.hector/gates/` (sorted by relative path).
/// Returns `"sha256:<hex>"`.
pub fn compute_hash(config_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let cfg_bytes = std::fs::read(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    hash_entry(&mut hasher, "config", &cfg_bytes);

    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let gates_dir = config_dir.join(".hector").join("gates");
    if gates_dir.is_dir() {
        for (rel, bytes) in collect_gate_files(&gates_dir)? {
            hash_entry(&mut hasher, &format!("gates/{rel}"), &bytes);
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}
```

- [ ] **Step 6: Run the tests to verify they pass + lint**

Run: `cargo test -p hector-core --lib trust:: && cargo clippy -p hector-core --all-targets -- -D warnings && cargo fmt`
Expected: all 5 tests PASS, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/hector-core/src/trust.rs
git rm --cached crates/hector-core/tests/trust_canonical_json.rs 2>/dev/null || true
git add -A crates/hector-core/tests/
git commit -m "feat(trust)!: hash config + .hector/gates/ for the trust store; drop in-YAML fingerprint"
```

---

### Task 2: Trust store — XDG path resolution + read/write/upsert

**Files:**
- Modify: `crates/hector-core/src/trust.rs`
- Test: inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from Task 1 (independent).
- Produces:
  - `pub fn trust_store_path() -> anyhow::Result<std::path::PathBuf>`
  - `pub struct TrustStore { pub version: u32, pub entries: std::collections::BTreeMap<String, TrustEntry> }`
  - `pub struct TrustEntry { pub hash: String, pub blessed_at: String }`
  - `pub fn read_store(path: &Path) -> anyhow::Result<TrustStore>` (missing file → default empty)
  - `pub fn write_store(path: &Path, store: &TrustStore) -> anyhow::Result<()>` (atomic temp+rename)
  - `const TRUST_STORE_VERSION: u32 = 1;`

- [ ] **Step 1: Write the failing tests**

Append to the `trust.rs` test module:

```rust
    #[test]
    fn store_path_joins_under_config_home() {
        let p = store_path_in(Path::new("/home/u/.config"));
        assert_eq!(p, Path::new("/home/u/.config/hector/trust.json"));
    }

    #[test]
    fn read_missing_store_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = read_store(&dir.path().join("trust.json")).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/trust.json"); // parent must be created
        let mut store = TrustStore::default();
        store.version = TRUST_STORE_VERSION;
        store.entries.insert(
            "/abs/.hector.yml".to_string(),
            TrustEntry { hash: "sha256:abc".into(), blessed_at: "2026-06-24T00:00:00Z".into() },
        );
        write_store(&path, &store).unwrap();
        let back = read_store(&path).unwrap();
        assert_eq!(back.entries["/abs/.hector.yml"].hash, "sha256:abc");
        assert_eq!(back.version, TRUST_STORE_VERSION);
    }

    #[test]
    fn xdg_config_home_overrides_home() {
        // config_home() prefers XDG_CONFIG_HOME. Test the pure resolver with an
        // explicit value rather than mutating process env.
        assert_eq!(
            config_home_from(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            config_home_from(None, Some("/h".into())),
            Some(PathBuf::from("/h/.config"))
        );
        assert_eq!(config_home_from(None, None), None);
    }
```

Add `use std::path::PathBuf;` to the test module imports if not already present (it is referenced above).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-core --lib trust::`
Expected: FAIL — `store_path_in`, `read_store`, `write_store`, `TrustStore`, `config_home_from` undefined.

- [ ] **Step 3: Implement the store**

Add to the non-test portion of `trust.rs` (extend the existing `use` lines):

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const TRUST_STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub entries: BTreeMap<String, TrustEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    pub hash: String,
    pub blessed_at: String,
}

/// `$XDG_CONFIG_HOME` (if set and non-empty) else `$HOME/.config`. Pure
/// resolver split out from the env read so it is testable without mutating
/// process env.
fn config_home_from(xdg: Option<String>, home: Option<String>) -> Option<PathBuf> {
    if let Some(x) = xdg {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    home.map(|h| PathBuf::from(h).join(".config"))
}

fn config_home() -> Option<PathBuf> {
    config_home_from(
        std::env::var("XDG_CONFIG_HOME").ok(),
        std::env::var("HOME").ok(),
    )
}

fn store_path_in(config_home: &Path) -> PathBuf {
    config_home.join("hector").join("trust.json")
}

/// Absolute path to the out-of-repo trust store.
pub fn trust_store_path() -> Result<PathBuf> {
    let home = config_home()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve config home (set $XDG_CONFIG_HOME or $HOME)"))?;
    Ok(store_path_in(&home))
}

/// Read the store; a missing file yields an empty store (never an error).
pub fn read_store(path: &Path) -> Result<TrustStore> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write the store atomically: serialize to a sibling temp file, then rename.
pub fn write_store(path: &Path, store: &TrustStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(store)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}
```

- [ ] **Step 4: Run to verify pass + lint**

Run: `cargo test -p hector-core --lib trust:: && cargo clippy -p hector-core --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/trust.rs
git commit -m "feat(trust): out-of-repo trust store (XDG path, atomic read/write)"
```

---

### Task 3: `ensure_trusted` + `bless` (enforcement core + thin wrappers)

**Files:**
- Modify: `crates/hector-core/src/trust.rs`
- Test: inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `compute_hash` (Task 1), `read_store`/`write_store`/`TrustStore`/`TrustEntry`/`trust_store_path`/`TRUST_STORE_VERSION` (Task 2).
- Produces:
  - `pub fn ensure_trusted(config_path: &Path) -> anyhow::Result<()>` — Ok if blessed & current; Err with the fail-closed message otherwise. Used by Task 5.
  - `pub fn bless(config_path: &Path) -> anyhow::Result<()>` — parse-validate, hash, upsert, write. Used by Tasks 4 & 6.
  - Testable cores: `ensure_trusted_in(config_path, store_path)`, `bless_in(config_path, store_path, now)`.

- [ ] **Step 1: Write the failing tests**

Append to the `trust.rs` test module:

```rust
    fn cfg_with_gate(dir: &Path) -> PathBuf {
        let cfg = dir.join(".hector.yml");
        write(&cfg, "gates:\n  g:\n    files: \"*\"\n    run: \".hector/gates/g.sh\"\n");
        write(&dir.join(".hector/gates/g.sh"), "#!/bin/sh\nexit 0\n");
        cfg
    }

    #[test]
    fn bless_then_ensure_succeeds() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = cfg_with_gate(proj.path());
        bless_in(&cfg, &store_path, "2026-06-24T00:00:00Z").unwrap();
        assert!(ensure_trusted_in(&cfg, &store_path).is_ok());
    }

    #[test]
    fn never_blessed_is_not_trusted() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let cfg = cfg_with_gate(proj.path());
        let err = ensure_trusted_in(&cfg, &store.path().join("trust.json"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not trusted"), "message must say not trusted: {err}");
        assert!(err.contains("hector trust"), "message must point at `hector trust`: {err}");
    }

    #[test]
    fn editing_a_gate_after_bless_revokes_trust() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = cfg_with_gate(proj.path());
        bless_in(&cfg, &store_path, "t").unwrap();
        // Tamper with the gate script.
        write(&proj.path().join(".hector/gates/g.sh"), "#!/bin/sh\nexit 2\n");
        assert!(ensure_trusted_in(&cfg, &store_path).is_err());
    }

    #[test]
    fn editing_config_after_bless_revokes_trust() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let store_path = store.path().join("trust.json");
        let cfg = cfg_with_gate(proj.path());
        bless_in(&cfg, &store_path, "t").unwrap();
        write(&cfg, "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
        assert!(ensure_trusted_in(&cfg, &store_path).is_err());
    }

    #[test]
    fn bless_rejects_unparseable_config() {
        let proj = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let cfg = proj.path().join(".hector.yml");
        write(&cfg, "schema_version: 2\nrules: {}\n"); // legacy → parser rejects
        assert!(bless_in(&cfg, &store.path().join("trust.json"), "t").is_err());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-core --lib trust::`
Expected: FAIL — `ensure_trusted_in` / `bless_in` undefined.

- [ ] **Step 3: Implement**

Add to the non-test portion of `trust.rs`:

```rust
/// Canonical absolute path used as the store key for `config_path`.
fn canonical_key(config_path: &Path) -> Result<String> {
    let canon = config_path
        .canonicalize()
        .with_context(|| format!("resolving {}", config_path.display()))?;
    Ok(canon.to_string_lossy().to_string())
}

/// Verify `config_path` (and its gate scripts) match a blessed entry in the
/// store at `store_path`. Fails closed with a fixed, actionable message.
pub fn ensure_trusted_in(config_path: &Path, store_path: &Path) -> Result<()> {
    let key = canonical_key(config_path)?;
    let expected = compute_hash(config_path)?;
    let store = read_store(store_path)?;
    match store.entries.get(&key) {
        Some(entry) if entry.hash == expected => Ok(()),
        _ => anyhow::bail!("config/gates not trusted — review and run `hector trust`"),
    }
}

/// Recompute the hash of `config_path` and write it to the store as blessed.
/// Parse-validates the config first so a broken config is never blessed.
pub fn bless_in(config_path: &Path, store_path: &Path, now: &str) -> Result<()> {
    crate::config::parse_file(config_path).context("refusing to trust a config that does not parse")?;
    let key = canonical_key(config_path)?;
    let hash = compute_hash(config_path)?;
    let mut store = read_store(store_path)?;
    store.version = TRUST_STORE_VERSION;
    store
        .entries
        .insert(key, TrustEntry { hash, blessed_at: now.to_string() });
    write_store(store_path, &store)
}

/// Thin wrapper: enforce trust against the real out-of-repo store.
pub fn ensure_trusted(config_path: &Path) -> Result<()> {
    ensure_trusted_in(config_path, &trust_store_path()?)
}

/// Thin wrapper: bless against the real out-of-repo store, stamping `blessed_at`
/// with the current UTC time.
pub fn bless(config_path: &Path) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    bless_in(config_path, &trust_store_path()?, &now)
}
```

- [ ] **Step 4: Run to verify pass + lint + full core suite**

Run: `cargo test -p hector-core && cargo clippy -p hector-core --all-targets -- -D warnings && cargo fmt`
Expected: all PASS (the core suite is unchanged elsewhere — enforcement is not in `load`).

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/trust.rs
git commit -m "feat(trust): ensure_trusted + bless against the out-of-repo store"
```

---

### Task 4: `hector trust` blesses the config (CLI)

**Files:**
- Modify: `crates/hector-cli/src/commands/trust.rs`
- Modify: `crates/hector-cli/src/cli.rs:54-58` (the `Trust` doc comment)
- Test: `crates/hector-cli/tests/cli_e2e_trust.rs` (new — the `trust`-command portion)

**Interfaces:**
- Consumes: `hector_core::trust::bless` (Task 3).

- [ ] **Step 1: Write the failing e2e test**

Create `crates/hector-cli/tests/cli_e2e_trust.rs`:

```rust
use assert_cmd::Command;
use std::fs;

/// `hector trust` writes a blessed entry into the XDG-redirected store.
#[test]
fn trust_writes_a_store_entry() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .success();

    let store = xdg.path().join("hector/trust.json");
    assert!(store.exists(), "trust must create the store file");
    let body = fs::read_to_string(&store).unwrap();
    assert!(body.contains("sha256:"), "store must hold a hash: {body}");
}

/// Blessing a config that does not parse fails (exit 1), writes nothing.
#[test]
fn trust_rejects_unparseable_config() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(&cfg, "schema_version: 2\nrules: {}\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(&cfg)
        .assert()
        .failure()
        .code(1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-cli --test cli_e2e_trust`
Expected: FAIL — the stub exits 0 and writes nothing (`store.exists()` false; the unparseable case exits 0 not 1).

- [ ] **Step 3: Implement the command**

Replace `crates/hector-cli/src/commands/trust.rs` with:

```rust
use anyhow::Result;
use std::path::Path;

pub fn run(config: &Path) -> Result<i32> {
    hector_core::trust::bless(config)?;
    println!("trusted: {}", config.display());
    Ok(0)
}
```

(The `?` propagates a parse/hash failure as `Err`; `main` already maps a command `Err` to exit 1 — verify this in `main.rs` and match the existing convention. If `main` does **not** map `Err`→1 for commands, instead `eprintln!` the error and `return Ok(1)` here.)

- [ ] **Step 4: Fix the stale help string**

In `crates/hector-cli/src/cli.rs`, replace the `Trust` doc comment:

```rust
    /// Bless this config + its `.hector/gates/` scripts in the out-of-repo trust store.
    Trust {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
    },
```

- [ ] **Step 5: Run to verify pass + lint**

Run: `cargo test -p hector-cli --test cli_e2e_trust && cargo clippy -p hector-cli --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

- [ ] **Step 6: Commit**

```bash
git add crates/hector-cli/src/commands/trust.rs crates/hector-cli/src/cli.rs crates/hector-cli/tests/cli_e2e_trust.rs
git commit -m "feat(cli): hector trust blesses config + gates in the trust store"
```

---

### Task 5: Enforce trust in `hector check` + bless existing check tests

**Files:**
- Modify: `crates/hector-cli/src/commands/check.rs` (top of `run()`)
- Create: `crates/hector-cli/tests/common/mod.rs` (shared `bless` helper)
- Modify: `crates/hector-cli/tests/cli_e2e_trust.rs` (add enforcement cases)
- Modify (bless + thread `XDG_CONFIG_HOME`): `cli_check_external_paths.rs`, `cli_check_single_load.rs`, `cli_e2e_check_quiet_stderr.rs`, `cli_e2e_gates.rs`, `cli_runner_telemetry_failure.rs`, `cli_version.rs`

**Interfaces:**
- Consumes: `hector_core::trust::ensure_trusted` (Task 3).

- [ ] **Step 1: Write the failing enforcement e2e tests**

Append to `crates/hector-cli/tests/cli_e2e_trust.rs`:

```rust
/// An unblessed config makes `check` fail closed with exit 1.
#[test]
fn unblessed_config_check_exits_1() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"exit 0\"\n").unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"])
        .arg(&cfg)
        .arg("--file")
        .arg(&target)
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("not trusted"));
}

/// After `trust`, the same `check` runs normally (passes here → exit 0).
#[test]
fn blessed_config_check_runs() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \"exit 0\"\n").unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("hector").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"]).arg(&cfg)
        .assert().success();

    Command::cargo_bin("hector").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"]).arg(&cfg)
        .arg("--file").arg(&target)
        .assert().success();
}

/// Editing a gate script after blessing revokes trust → check exits 1.
#[test]
fn editing_gate_after_bless_blocks_check() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let cfg = proj.path().join(".hector.yml");
    fs::write(&cfg, "gates:\n  g:\n    files: \"*.rs\"\n    run: \".hector/gates/g.sh\"\n").unwrap();
    let gates = proj.path().join(".hector/gates");
    fs::create_dir_all(&gates).unwrap();
    fs::write(gates.join("g.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    let target = proj.path().join("a.rs");
    fs::write(&target, "x\n").unwrap();

    Command::cargo_bin("hector").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"]).arg(&cfg)
        .assert().success();

    fs::write(gates.join("g.sh"), "#!/bin/sh\nexit 2\n").unwrap(); // tamper

    Command::cargo_bin("hector").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"]).arg(&cfg)
        .arg("--file").arg(&target)
        .assert().failure().code(1)
        .stderr(predicates::str::contains("not trusted"));
}
```

- [ ] **Step 2: Run to verify the first new test fails**

Run: `cargo test -p hector-cli --test cli_e2e_trust unblessed_config_check_exits_1`
Expected: FAIL — `check` currently ignores trust, so an unblessed config still runs (exit 0, no "not trusted").

- [ ] **Step 3: Add the enforcement gate to `check.rs`**

In `crates/hector-cli/src/commands/check.rs`, at the very top of `run()` (before building `CheckOptions`):

```rust
    if let Err(e) = hector_core::trust::ensure_trusted(config) {
        eprintln!("ERROR: {e:#}");
        return Ok(1);
    }
```

- [ ] **Step 4: Verify the new enforcement tests pass**

Run: `cargo test -p hector-cli --test cli_e2e_trust`
Expected: all PASS.

- [ ] **Step 5: Create the shared bless helper**

Create `crates/hector-cli/tests/common/mod.rs`:

```rust
//! Shared helpers for CLI integration tests.
use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

/// Bless `config` in a fresh, isolated trust store and return the `TempDir`
/// that backs it. Keep the returned guard alive for the test, and set
/// `XDG_CONFIG_HOME` to `guard.path()` on every `hector` invocation that runs
/// `check`, so they all read the same blessed store.
#[must_use]
pub fn blessed_store(config: &Path) -> TempDir {
    let xdg = tempfile::tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["trust", "--config"])
        .arg(config)
        .assert()
        .success();
    xdg
}
```

- [ ] **Step 6: Convert each existing check test (representative example)**

For **every** file in the "Modify" list, add `mod common;` at the top, bless the config before the first `check`, and add `.env("XDG_CONFIG_HOME", xdg.path())` to **every** `check` command. Worked example — `cli_check_single_load.rs` before:

```rust
    let mut cmd = Command::cargo_bin("hector").unwrap();
    cmd.env("HECTOR_DEBUG_LOAD_COUNT", "1")
        .args(["check", "--config"]).arg(&cfg)
        .arg("--file").arg(&target);
    cmd.assert().success();
```

after:

```rust
mod common; // top of file

    let xdg = common::blessed_store(&cfg);
    let mut cmd = Command::cargo_bin("hector").unwrap();
    cmd.env("XDG_CONFIG_HOME", xdg.path())
        .env("HECTOR_DEBUG_LOAD_COUNT", "1")
        .args(["check", "--config"]).arg(&cfg)
        .arg("--file").arg(&target);
    cmd.assert().success();
```

Apply the same shape to each `check` invocation in: `cli_check_external_paths.rs` (2 sites), `cli_check_single_load.rs` (1 check site after the trust setup), `cli_e2e_check_quiet_stderr.rs` (the check sites), `cli_e2e_gates.rs` (every `check`), `cli_runner_telemetry_failure.rs` (the check sites), `cli_version.rs` (its single `check`). Notes:
- The config path passed to `blessed_store` must be the **same** path later passed to `--config` (so the canonical-key matches). When a test edits the config after blessing to assert a *different* behavior, that test is now also exercising trust revocation — re-bless after the edit if the test's intent is to check post-edit gate behavior, not trust.
- `cli_e2e_check_quiet_stderr.rs` asserts stderr is empty on a passing check. `blessed_store` runs in a *separate* process, so it does not pollute the check's stderr — but double-check the assertion targets only the `check` command's output.
- A test that intentionally passes a **bad/missing config** to assert a parse/load error (exit 1) does **not** need blessing — trust returns exit 1 too, but assert on the existing error text; if a test distinguishes "not trusted" from a parse error, bless it so the parse path is reached.

- [ ] **Step 7: Run the full CLI suite + lint**

Run: `cargo test -p hector-cli && cargo clippy -p hector-cli --all-targets -- -D warnings && cargo fmt`
Expected: ALL pass. If any check test still fails with exit 1 / "not trusted", it is missing either the bless call or the `XDG_CONFIG_HOME` env on that command.

- [ ] **Step 8: Commit**

```bash
git add crates/hector-cli/src/commands/check.rs crates/hector-cli/tests/
git commit -m "feat(cli)!: hector check fails closed on untrusted config/gates"
```

---

### Task 6: `hector init` auto-blesses the scaffolded config

**Files:**
- Modify: `crates/hector-cli/src/commands/init/mod.rs:30-36` (`run`)
- Test: `crates/hector-cli/tests/cli_init.rs` (add one case)

**Interfaces:**
- Consumes: `hector_core::trust::bless` (Task 3).

- [ ] **Step 1: Write the failing test**

Add to `crates/hector-cli/tests/cli_init.rs`:

```rust
/// `init` auto-blesses, so a `check` against the scaffolded config runs
/// without a separate `hector trust` step (it is not rejected as untrusted).
#[test]
fn init_auto_blesses_so_check_is_trusted() {
    let proj = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();

    Command::cargo_bin("hector").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["init", "--dir"]).arg(proj.path())
        .assert().success();

    let cfg = proj.path().join(".hector.yml");
    let target = proj.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();

    // Should NOT be rejected as untrusted. Some scaffolded gate may or may not
    // block on this file, but the verdict must not be the trust exit-1.
    let out = Command::cargo_bin("hector").unwrap()
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["check", "--config"]).arg(&cfg)
        .arg("--file").arg(&target)
        .assert();
    let code = out.get_output().status.code().unwrap();
    assert_ne!(code, 1, "init-blessed config must not be rejected as untrusted");
}
```

(If `cli_init.rs` does not already `use assert_cmd::Command;` / `tempfile`, add the imports.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p hector-cli --test cli_init init_auto_blesses_so_check_is_trusted`
Expected: FAIL — check exits 1 (config not blessed).

- [ ] **Step 3: Auto-bless in `init::run`**

In `crates/hector-cli/src/commands/init/mod.rs`, after `std::fs::write(&cfg_path, body)?;`:

```rust
    std::fs::write(&cfg_path, body)?;
    hector_core::trust::bless(&cfg_path)
        .map_err(|e| anyhow!("scaffolded {} but could not trust it: {e:#}", cfg_path.display()))?;
    println!("scaffolded and trusted: {}", cfg_path.display());
    println!(
        "review the config, then run: hector check --file <path> --config {}",
        cfg_path.display()
    );
    Ok(0)
```

Remove the now-duplicated `println!("scaffolded: ...")` line above it.

- [ ] **Step 4: Run to verify pass + full init suite + lint**

Run: `cargo test -p hector-cli --test cli_init && cargo clippy -p hector-cli --all-targets -- -D warnings && cargo fmt`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-cli/src/commands/init/mod.rs crates/hector-cli/tests/cli_init.rs
git commit -m "feat(cli): hector init auto-blesses the scaffolded config"
```

---

### Task 7: Docs — record that trust is wired

**Files:**
- Modify: `AGENTS.md` (trust is no longer "unwired"/stub/dormant)
- Modify: `plans/README.md` (move this plan toward archive on completion; update Future/Active)
- Modify: `CHANGELOG.md` (if present)

**Interfaces:** none (docs only).

- [ ] **Step 1: Update `AGENTS.md`**

Edit these claims to reflect the wired trust store:
- "What this is" line: `trust` is no longer a no-op stub — it blesses the out-of-repo store; `check` fails closed (exit 1) on untrusted config/gates.
- "Not yet built" paragraph: remove Plan 2 from the deferred list (it's now built); keep Plans 3 (`verify` + full `doctor`) and 4 (adapters).
- `trust` module bullet: drop "**Kept but unwired**"; describe the new store (`~/.config/hector/trust.json`, hash over config + `.hector/gates/`, keyed by canonical abs path).
- Exit-code contract `1` line: add "or untrusted config/gates".
- Conventions bullet about `trust.rs` and `tests/trust_canonical_json.rs` being dormant: remove it (the test is deleted; trust.rs is live). Note the new enforcement site is the CLI `check` command, not `HectorEngine::load`.

- [ ] **Step 2: Update `plans/README.md`**

In the `Future` section, strike the `G1 trust+rules split CI lint` stop-gap if the new trust model supersedes it (it does — note "resolved by the 2026-06-24 trust store"). Add a one-line Archive entry pointer for this plan (the file move happens in the finishing step).

- [ ] **Step 3: Update `CHANGELOG.md` if present**

Run: `test -f CHANGELOG.md && echo present || echo absent`
If present, add an `Unreleased` bullet: "Trust: out-of-repo allow-list at `~/.config/hector/trust.json`; `hector check` fails closed until `hector trust` blesses the config + `.hector/gates/`; `hector init` auto-blesses."

- [ ] **Step 4: Commit**

```bash
git add AGENTS.md plans/README.md CHANGELOG.md 2>/dev/null
git commit -m "docs: trust store is wired (Plan 2)"
```

---

## Final verification (after all tasks)

- [ ] `cargo test` — full workspace green.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.
- [ ] `cargo fmt --check` — clean.
- [ ] `bash scripts/ci-coverage.sh` if runnable locally (else CI) — `trust.rs`, `check.rs`, `init/mod.rs`, `commands/trust.rs` ≥80% region.
- [ ] Manual smoke: in a scratch dir, `hector init` → `hector check --file x` passes trust; edit a gate script → `hector check` exits 1 "not trusted"; `hector trust` → passes again.
- [ ] Whole-branch code review (opus) before finishing.
