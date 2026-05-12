# Hector E1 — Baseline Line-Content Checksum Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §E1](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🟡 high (latent correctness bug)
**Sequencing:** Track-E robustness item; parallelizable with B1/C4 by keeping the runner edit confined to the post-loop baseline-filter step.

---

**Goal:** Fingerprint baseline entries by `(rule_id, file, line, line_sha256)` rather than `(rule_id, file, line)` so:
1. Moving a violating line preserves the suppression when the line text is unchanged.
2. Editing the baselined line resurfaces the violation (it's a "new" violation in spirit).
3. A new violation that lands on the old line is no longer silenced.

This is a latent correctness bug: today's `rule_id::file::line` fingerprint silences any future violation that happens to land on a baselined line. With semantic rules whose verdicts depend on subtle text, this becomes a visible regression source.

**Architecture:** The `Baseline` type grows from `HashSet<String>` (opaque fingerprints) to `HashMap<key, Option<String>>` where the key is the existing tuple fingerprint and the value is the optional line-content SHA-256. Old-format JSON (a flat `fingerprints: HashSet<String>`) deserializes via a custom deserializer that maps each opaque fingerprint to `None`; on the first `None` we hit during a replay match, we emit a one-time deprecation warning to stderr (`OnceLock<()>` guard) and treat the entry as "always match" — current behavior. New-format JSON serializes a single `entries: BTreeMap<String, Option<String>>` (deterministic ordering for the snapshot tests downstream). `Baseline::add` now takes both the violation and the on-disk file content so it can capture the line text at recording time. `Baseline::contains` now also takes the on-disk file content and only returns true when both the key and the checksum match (or the stored checksum is `None`, the legacy grace case).

The runner's call site changes minimally: where it currently calls `baseline.contains(v)`, it now passes a small `&dyn LineSource` closure that maps `(file_path, line)` back to the line text. To keep the runner's edit narrow (B1 is parallelizing the loop body; we should not touch the loop), the line-lookup is owned by `baseline.rs` — the runner just hands it the post-loop violations and a `&Path` for the file under check. All checksum logic lives in `baseline.rs`.

A new CLI subcommand `hector baseline refresh` re-reads every baselined `(rule_id, file, line)` and updates the stored checksum to the current line content. Idempotent.

**Tech Stack:** Rust, workspace-stable. `sha2` already a direct dep on `hector-core` (used by `trust.rs`). No new deps.

---

## Decisions ratified up-front

| Decision | Choice | Reason |
|---|---|---|
| Field name | `line_sha256: Option<String>` per entry | Matches bully's `src/bully/state/baseline.py`. Spec §E1 step 1. |
| Hash content | `sha256(line.trim_end())` | Spec §E1 step 2: survive `\n` vs `\r\n` and trailing-whitespace normalization. `trim_end` strips both `\r`, ` `, and `\t`. |
| Old-format fallback | Read-tolerant. Absent checksum → "always match" (current behavior). One-time stderr deprecation warning per process. | Spec §E1 step 1: "Read-tolerant of old entries during a grace period." Acceptance criterion 3 verbatim. |
| New on-disk shape | `{ "entries": { "<fingerprint>": "<sha256>" or null, ... } }` keyed by the existing tuple-fingerprint string. | Keying by the tuple-fingerprint reuses the collision-safe encoding from P1-4. BTreeMap ordering keeps `--scan` snapshots stable. |
| Old on-disk shape | `{ "fingerprints": [ "<fingerprint>", ... ] }` | Untouched. We detect this shape via a custom serde untagged Deserialize and lift it into the new shape with `None` checksums. |
| Line missing from file at replay | Treat as "no longer applies" → suppress. | If the baselined line is gone, the violation can't recur; keeping the suppression matches the spec's "preserve the suppression". If the violation re-fires on a different shape, the new fingerprint won't match anyway. |
| Line missing from file at refresh | Drop the entry. | A baseline entry pointing at a line that no longer exists is stale; refresh is the right moment to garbage-collect. We log it to stderr. |
| `line: None` violations | No checksum (the entry never gets one). Match by tuple alone. | File-level violations from script/semantic rules have no line to hash. |
| Refresh on a v1 baseline | First read upgrades in-memory to v2 with `None` values; refresh then captures every line. | Single happy path; the deprecation warning fires once during the read. |
| Telemetry impact | None at this phase. | Spec doesn't ask for it. If we want a `kind: "baseline_drift"` log later, that's additive. |
| Verdict shape | Unchanged. | Locked-but-unstable; this is a baseline-filter behavior change. |
| Trust fingerprint | Unaffected. | Baseline is on-disk state, not config. |
| CLI shape | `hector baseline refresh` as a subcommand of `Baseline`. | A bare flag (`--refresh`) muddles the existing record-mode behavior. A subcommand is idiomatic clap. Existing `hector baseline` retains its current record semantics. |

---

## File structure

```
crates/hector-core/
├── src/
│   ├── baseline.rs                  ← MODIFIED: new schema, line-sha256, legacy reader, refresh helper
│   └── runner.rs                    ← MODIFIED: pass file content into contains() call (minimal touch)
└── tests/
    └── baseline.rs                  ← MODIFIED: new tests for each acceptance criterion
    └── baseline_legacy.rs           ← NEW: fixture-driven legacy format test
└── tests/fixtures/
    └── baseline_v1.json             ← NEW: old-format fixture

crates/hector-cli/
├── src/
│   ├── cli.rs                       ← MODIFIED: Baseline becomes a parent with Record + Refresh subcommands; preserve back-compat default
│   ├── main.rs                      ← MODIFIED: dispatch new Refresh action
│   └── commands/baseline.rs         ← MODIFIED: extract `refresh` function alongside existing record logic
└── tests/
    └── cli_baseline_refresh.rs      ← NEW: integration test for the refresh subcommand
```

Three runner edits, all confined to the post-loop baseline-filter step (one-liner): change the closure signature on `baseline.contains` to also pass the file content (or supply a lookup closure). B1's loop-parallelization touches the loop body, not this step; merges should be conflict-free.

---

## Phase 1 — Failing tests for line-content fingerprinting

### Task 1: Append failing tests to `crates/hector-core/tests/baseline.rs`

**Files:**
- Modify: `crates/hector-core/tests/baseline.rs`

The new API surface we want:

```rust
impl Baseline {
    pub fn add_with_content(&mut self, v: &Violation, file_content: Option<&str>);
    pub fn contains_with_content(&self, v: &Violation, file_content: Option<&str>) -> bool;
    /// Recompute every entry's `line_sha256` from the current on-disk content
    /// of the file the entry points at. Returns the count of refreshed +
    /// dropped entries.
    pub fn refresh(&mut self, root: &Path) -> RefreshReport;
}

pub struct RefreshReport {
    pub refreshed: usize,
    pub dropped: usize,
}
```

The existing `add` / `contains` methods stay, defined as `add_with_content(v, None)` / `contains_with_content(v, None)` shims so test sites and the runner don't all need to change in one commit.

- [ ] **Step 1: Append tests**

```rust
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
    // Record at line 3, then "move" the same content to line 5.
    let mut b = Baseline::default();
    let original = content(&["fn main() {}", "", "TODO: ship E1", "", "fn other() {}"]);
    let v_record = make_violation("todo-marker", "src/lib.rs", Some(3));
    b.add_with_content(&v_record, Some(&original));

    // The content at line 3 still hashes to the same value when checked
    // against the original file.
    assert!(b.contains_with_content(&v_record, Some(&original)));

    // The same line, moved to line 5 in a new file shape, must still be
    // suppressed because the line content is unchanged.
    let moved = content(&[
        "fn main() {}",
        "",
        "// added comment",
        "",
        "TODO: ship E1",
    ]);
    let v_moved = make_violation("todo-marker", "src/lib.rs", Some(5));
    // Replay needs the new file content + new line number from the engine.
    // We don't auto-discover the new line: the engine emitted Some(5).
    // What we DO check is that an entry with the *same hash* matches when
    // the violation now points at line 5 in `moved`.
    assert!(
        b.contains_with_content(&v_moved, Some(&moved)),
        "moved line with same content must remain suppressed"
    );
}

#[test]
fn editing_baselined_line_resurfaces_violation() {
    let mut b = Baseline::default();
    let original = content(&["fn main() {}", "TODO: ship E1"]);
    let v = make_violation("todo-marker", "src/lib.rs", Some(2));
    b.add_with_content(&v, Some(&original));

    // User edits the line — still a TODO, still violates the rule, but the
    // content changed. Baseline must NOT suppress.
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
    let legacy = format!("{{ \"fingerprints\": [\"{fp}\"] }}");
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
```

- [ ] **Step 2: Run, confirm they fail**

Run: `cargo test --test baseline 2>&1 | tail -40`

Expected: compile errors — `add_with_content`, `contains_with_content`, `refresh`, `RefreshReport` not defined.

- [ ] **Step 3: Commit failing tests**

```bash
git add crates/hector-core/tests/baseline.rs
git commit -m "test(baseline): failing tests for line-content checksum (E1 phase 1)"
```

---

## Phase 2 — Implement line-content fingerprinting

### Task 2: Rewrite `baseline.rs` with new shape + legacy fallback

**Files:**
- Modify: `crates/hector-core/src/baseline.rs`

- [ ] **Step 1: Replace the module body**

The new shape:

```rust
use crate::verdict::Violation;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Has the legacy-format deprecation warning been emitted in this process?
static LEGACY_WARNING_EMITTED: OnceLock<()> = OnceLock::new();

/// On-disk baseline.
///
/// **v2 (E1):** `entries` maps tuple-fingerprint → optional SHA-256 of the
/// line content at recording time. Replay matches when both the
/// fingerprint and the checksum match (or the checksum is absent — the
/// grace-period behavior for v1 baselines).
///
/// **v1 (pre-E1):** `fingerprints` is a flat set of tuple-fingerprint
/// strings. Loaded with a one-time deprecation warning; every entry is
/// treated as "always match" (matches v1 behavior). Run `hector baseline
/// refresh` to upgrade in place.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Baseline {
    pub entries: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshReport {
    pub refreshed: usize,
    pub dropped: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BaselineOnDisk {
    /// v2: { "entries": { "<fp>": <sha or null>, ... } }
    V2 {
        entries: BTreeMap<String, Option<String>>,
    },
    /// v1 legacy: { "fingerprints": [ "<fp>", ... ] }
    V1 {
        fingerprints: Vec<String>,
    },
}

impl Baseline {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let parsed: BaselineOnDisk = serde_json::from_str(&content)?;
        match parsed {
            BaselineOnDisk::V2 { entries } => Ok(Self { entries }),
            BaselineOnDisk::V1 { fingerprints } => {
                Self::emit_legacy_warning(path);
                let entries = fingerprints.into_iter().map(|fp| (fp, None)).collect();
                Ok(Self { entries })
            }
        }
    }

    fn emit_legacy_warning(path: &Path) {
        if LEGACY_WARNING_EMITTED.set(()).is_ok() {
            eprintln!(
                "hector: warning — baseline at {} uses the legacy v1 format \
                 (no line_sha256). Run `hector baseline refresh` to upgrade. \
                 Old-format entries will continue to suppress matching \
                 fingerprints during a grace period.",
                path.display()
            );
        }
    }

    /// (preserved verbatim — see file)
    pub fn save(&self, path: &Path) -> Result<()> { /* unchanged */ }

    pub fn fingerprint(v: &Violation) -> String { /* unchanged */ }

    /// Compute the SHA-256 of `line.trim_end()` — strips trailing `\r`,
    /// ` `, `\t` so the checksum survives CRLF / trailing-whitespace
    /// normalization across editors.
    pub fn line_checksum(line: &str) -> String {
        let mut h = Sha256::new();
        h.update(line.trim_end().as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Look up the 1-based `line` in `file_content`. Returns `None` if the
    /// line is out of range.
    fn line_at(file_content: &str, line: u32) -> Option<&str> {
        if line == 0 { return None; }
        file_content.lines().nth((line - 1) as usize)
    }

    pub fn add(&mut self, v: &Violation) {
        self.add_with_content(v, None);
    }

    pub fn add_with_content(&mut self, v: &Violation, file_content: Option<&str>) {
        let key = Self::fingerprint(v);
        let checksum = v
            .line
            .and_then(|n| file_content.and_then(|c| Self::line_at(c, n)))
            .map(Self::line_checksum);
        self.entries.insert(key, checksum);
    }

    pub fn contains(&self, v: &Violation) -> bool {
        self.contains_with_content(v, None)
    }

    pub fn contains_with_content(&self, v: &Violation, file_content: Option<&str>) -> bool {
        let key = Self::fingerprint(v);
        let Some(stored) = self.entries.get(&key) else {
            return false;
        };
        match (stored, v.line, file_content) {
            // No stored checksum → legacy grace-period behavior: always match.
            (None, _, _) => true,
            // File-level violations (no line) never had a checksum to compare.
            (Some(_), None, _) => true,
            // We have a stored checksum, a line number, and current content.
            (Some(expected), Some(n), Some(content)) => match Self::line_at(content, n) {
                Some(line) => Self::line_checksum(line) == *expected,
                // Line gone from the file → treat as still suppressed; the
                // violation cannot recur on a line that no longer exists.
                None => true,
            },
            // We have a stored checksum but no current content to compare
            // against. Conservative: treat as match. The runner always
            // passes content; this arm is only hit by direct library
            // callers that opted out.
            (Some(_), Some(_), None) => true,
        }
    }

    /// Re-hash every entry against the current on-disk content of the file
    /// it points at, dropping entries whose line is gone.
    pub fn refresh(&mut self, root: &Path) -> Result<RefreshReport> {
        let mut report = RefreshReport { refreshed: 0, dropped: 0 };
        let mut new_entries: BTreeMap<String, Option<String>> = BTreeMap::new();

        for (key, _old) in &self.entries {
            match Self::file_and_line_from_fingerprint(key) {
                Some((file_rel, Some(line))) => {
                    let path = Self::join_rel(root, &file_rel);
                    let content = std::fs::read_to_string(&path).ok();
                    match content.as_deref().and_then(|c| Self::line_at(c, line)) {
                        Some(text) => {
                            new_entries.insert(key.clone(), Some(Self::line_checksum(text)));
                            report.refreshed += 1;
                        }
                        None => {
                            eprintln!(
                                "hector: refresh — dropping baseline entry {key}: \
                                 line {line} no longer present in {}",
                                path.display()
                            );
                            report.dropped += 1;
                        }
                    }
                }
                Some((_, None)) => {
                    // File-level entry: keep with None, nothing to refresh.
                    new_entries.insert(key.clone(), None);
                }
                None => {
                    // Malformed key — keep as-is; refresh shouldn't drop
                    // entries it can't parse, that's data loss.
                    new_entries.insert(key.clone(), None);
                }
            }
        }
        self.entries = new_entries;
        Ok(report)
    }

    /// Inverse of `fingerprint`: pull back the `(file, line)` from the
    /// JSON-encoded 3-tuple key.
    fn file_and_line_from_fingerprint(key: &str) -> Option<(String, Option<u32>)> {
        // The fingerprint is `serde_json::to_string(&(&rule_id, &file, &line))`.
        let (_rule_id, file, line): (String, String, Option<u32>) =
            serde_json::from_str(key).ok()?;
        Some((file, line))
    }

    fn join_rel(root: &Path, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() { p.to_path_buf() } else { root.join(p) }
    }
}
```

Two notes for the reviewer:
- `BaselineOnDisk` is `Deserialize`-only; `Baseline` is `Serialize`-only. This forces every write to produce v2 even after a v1 read.
- `LEGACY_WARNING_EMITTED` is process-global. The CLI process boundary is the unit; `hector check` followed by `hector baseline refresh` would each warn once if both encountered a v1 file.

- [ ] **Step 2: Run the test file, confirm green**

Run: `cargo test --test baseline`

Expected: all tests pass, including the 7 new ones from Phase 1.

- [ ] **Step 3: Run the full hector-core suite**

Run: `cargo test -p hector-core`

Expected: green. Existing call sites use `add` / `contains` shims, so nothing else needs to change.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-core/src/baseline.rs crates/hector-core/tests/baseline.rs
git commit -m "feat(baseline): add line_sha256 fingerprinting (E1 phase 2)"
```

---

## Phase 3 — Wire content into the runner

### Task 3: Pass file content through the baseline-filter step

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (post-loop baseline-filter block only)

The current code (around line 285–301):

```rust
let baseline_path = self.config_dir.join(".hector/baseline.json");
let baseline = match crate::baseline::Baseline::load(&baseline_path) {
    Ok(b) => b,
    Err(e) => { /* warn, fall back */ Baseline::default() }
};
violations.retain(|v| !baseline.contains(v));
```

The change is a one-liner: `!baseline.contains(v)` → `!baseline.contains_with_content(v, Some(&content))`. `content` is the post-edit file body captured at the top of `check`. For diff mode the file was already read into `content` (line 155); for file mode the caller supplies it. Both modes have the post-edit content available, which is correct semantics (we check against what the agent just wrote).

- [ ] **Step 1: Update the filter**

Edit `crates/hector-core/src/runner.rs:301`:

```rust
violations.retain(|v| !baseline.contains_with_content(v, Some(&content)));
```

- [ ] **Step 2: Update the `baseline` CLI command to pass content during recording**

In `crates/hector-cli/src/commands/baseline.rs`, where the loop calls `bl.add(&v)`, change to `bl.add_with_content(&v, Some(&content))` — the same `content` already read from disk on the line above. This is what gives newly-recorded entries a real checksum.

- [ ] **Step 3: Run the full workspace**

Run: `cargo test`

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-core/src/runner.rs crates/hector-cli/src/commands/baseline.rs
git commit -m "feat(runner): pass file content to baseline.contains for E1 checksum"
```

---

## Phase 4 — Backwards-compat fixture + integration test

### Task 4: Pin the legacy-format wire shape

**Files:**
- Create: `crates/hector-core/tests/fixtures/baseline_v1.json`
- Create: `crates/hector-core/tests/baseline_legacy.rs`

- [ ] **Step 1: Write the fixture**

Content:

```json
{
  "fingerprints": [
    "[\"todo-marker\",\"src/lib.rs\",2]"
  ]
}
```

The single fingerprint is what `Baseline::fingerprint` would have produced for `(rule_id=todo-marker, file=src/lib.rs, line=2)` under the post-P1-4 JSON-tuple encoding.

- [ ] **Step 2: Write the integration test**

```rust
use hector_core::baseline::Baseline;
use hector_core::verdict::{Engine, Severity, Violation};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn make_violation(rule_id: &str, file: &str, line: Option<u32>) -> Violation {
    Violation {
        rule_id: rule_id.to_string(),
        severity: Severity::Warning,
        engine: Engine::Script,
        file: file.to_string(),
        line,
        column: None,
        message: "x".to_string(),
        suggestion: None,
        context: None,
    }
}

#[test]
fn v1_fixture_loads_and_matches_by_tuple_only() {
    let b = Baseline::load(&fixture_path("baseline_v1.json"))
        .expect("v1 fixture must load");
    let v = make_violation("todo-marker", "src/lib.rs", Some(2));
    // No checksum → always match.
    assert!(b.contains_with_content(&v, Some("anything\n")));
    assert!(b.contains_with_content(&v, Some("TODO: changed line\n")));
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test baseline_legacy`

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-core/tests/fixtures/baseline_v1.json crates/hector-core/tests/baseline_legacy.rs
git commit -m "feat(baseline): backwards-compat read for old format (E1 phase 3)"
```

---

## Phase 5 — `hector baseline refresh` subcommand

### Task 5: CLI plumbing + integration test

**Files:**
- Modify: `crates/hector-cli/src/cli.rs`
- Modify: `crates/hector-cli/src/main.rs`
- Modify: `crates/hector-cli/src/commands/baseline.rs`
- Create: `crates/hector-cli/tests/cli_baseline_refresh.rs`

The current `Baseline { config, scan }` variant becomes a parent that takes a `#[command(subcommand)]`. We preserve the no-subcommand record path with a default (record mode) so existing scripts don't break.

- [ ] **Step 1: Update `cli.rs`**

Replace the existing `Baseline` variant with:

```rust
/// Manage the suppression baseline.
Baseline {
    #[command(subcommand)]
    action: Option<BaselineAction>,
    #[arg(long, default_value = ".hector.yml", global = true)]
    config: PathBuf,
    /// (record mode) Glob filter restricting which files are scanned.
    #[arg(long, global = true)]
    scan: Option<String>,
},
```

And add:

```rust
#[derive(Debug, Subcommand)]
pub enum BaselineAction {
    /// Record current violations to .hector/baseline.json.
    Record,
    /// Re-hash every baseline entry against current file content.
    Refresh,
}
```

The `Option<BaselineAction>` with `None` defaulting to record mode preserves the existing CLI shape exactly.

- [ ] **Step 2: Update `main.rs` dispatch**

```rust
Command::Baseline { action, config, scan } => match action.unwrap_or(cli::BaselineAction::Record) {
    cli::BaselineAction::Record => commands::baseline::record(&config, scan)?,
    cli::BaselineAction::Refresh => commands::baseline::refresh(&config)?,
},
```

- [ ] **Step 3: Update `commands/baseline.rs`**

Rename the existing `run` → `record`. Add a sibling `refresh`:

```rust
pub fn refresh(config: &Path) -> Result<i32> {
    let dir = config.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let baseline_path = dir.join(".hector/baseline.json");
    let mut baseline = Baseline::load(&baseline_path)?;
    let report = baseline.refresh(dir)?;
    baseline.save(&baseline_path)?;
    println!(
        "baseline refreshed: {} entries updated, {} entries dropped",
        report.refreshed, report.dropped
    );
    Ok(0)
}
```

- [ ] **Step 4: Write the CLI integration test**

`crates/hector-cli/tests/cli_baseline_refresh.rs`:

```rust
use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn refresh_updates_checksums_to_current_content() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    // Trivially-trusted config.
    let cfg_body = "schema_version: 2\nrules:\n  todo-marker:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"grep -nE 'TODO' {file} && exit 1 || exit 0\"\n";
    let trusted = hector_core::trust::write_trust_block(cfg_body).unwrap();
    let cfg = root.join(".hector.yml");
    fs::write(&cfg, trusted).unwrap();

    // Seed a file and record a baseline.
    let file = root.join("a.txt");
    fs::write(&file, "TODO: original\n").unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "--config", cfg.to_str().unwrap(), "--scan", "*.txt"])
        .current_dir(root)
        .assert()
        .success();

    let baseline_path = root.join(".hector/baseline.json");
    let before = fs::read_to_string(&baseline_path).unwrap();
    assert!(before.contains("\"entries\""), "v2 shape expected: {before}");

    // Edit the file but keep the rule applicable.
    fs::write(&file, "TODO: changed\n").unwrap();

    // Refresh.
    Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "refresh", "--config", cfg.to_str().unwrap()])
        .current_dir(root)
        .assert()
        .success();

    let after = fs::read_to_string(&baseline_path).unwrap();
    assert_ne!(before, after, "refresh should rewrite the checksum");
}

#[test]
fn refresh_with_no_baseline_succeeds_silently() {
    let dir = tempdir().unwrap();
    let cfg_body = "schema_version: 2\nrules:\n  noop:\n    description: x\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = hector_core::trust::write_trust_block(cfg_body).unwrap();
    let cfg = dir.path().join(".hector.yml");
    fs::write(&cfg, trusted).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .args(["baseline", "refresh", "--config", cfg.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();
}
```

- [ ] **Step 5: Run**

Run: `cargo test -p hector-cli --test cli_baseline_refresh`

Expected: green.

- [ ] **Step 6: Run full workspace**

Run: `cargo test`

Expected: green.

- [ ] **Step 7: Commit**

```bash
git add crates/hector-cli/src/cli.rs crates/hector-cli/src/main.rs \
        crates/hector-cli/src/commands/baseline.rs \
        crates/hector-cli/tests/cli_baseline_refresh.rs
git commit -m "feat(cli): hector baseline refresh subcommand (E1 phase 4)"
```

---

## Phase 6 — Final sweep

### Task 6: Lint, format, coverage

- [ ] **Step 1: `cargo fmt --check`** — must produce no diff.
- [ ] **Step 2: `cargo clippy --all-targets -- -D warnings`** — must be green. Watch for cognitive-complexity warnings in `Baseline::refresh` and `Baseline::contains_with_content`; the match on a 3-tuple inside `contains_with_content` is at the edge of the 15-cap. If clippy fires, extract the match arms to a `match-content` helper before annotating.
- [ ] **Step 3: `cargo test --workspace`** — must be green.
- [ ] **Step 4: `bash scripts/ci-coverage.sh`** — `baseline.rs` must hit ≥90% region. The `refresh` happy path and drop path are both exercised; the `BaselineOnDisk` V1 branch is exercised by `legacy_baseline_without_checksum_loads_with_warning`. If any branch is uncovered, add a focused unit test before committing.
- [ ] **Step 5: Commit any sweep fixes**

```bash
git add -A
git commit -m "style(baseline): fmt + clippy sweep (E1 phase 5)"
```

---

## Test plan summary

| Test | File | Acceptance criterion covered |
|---|---|---|
| `moving_baselined_line_preserves_suppression` | `tests/baseline.rs` | AC1: line moved, content unchanged → still suppressed |
| `editing_baselined_line_resurfaces_violation` | `tests/baseline.rs` | AC2: content edited → re-surfaces |
| `trailing_whitespace_does_not_invalidate_checksum` | `tests/baseline.rs` | Spec hashing detail: `trim_end` |
| `line_none_violation_baselines_without_checksum` | `tests/baseline.rs` | File-level violations behave as before |
| `legacy_baseline_without_checksum_loads_with_warning` | `tests/baseline.rs` | AC3: old format loads + grace-period match |
| `refresh_updates_checksum_to_current_file_content` | `tests/baseline.rs` | AC4: refresh updates checksum |
| `refresh_drops_entries_whose_line_no_longer_exists` | `tests/baseline.rs` | Refresh garbage-collects stale entries |
| `v1_fixture_loads_and_matches_by_tuple_only` | `tests/baseline_legacy.rs` | AC3: pinned wire-shape compatibility |
| `refresh_updates_checksums_to_current_content` | `crates/hector-cli/tests/cli_baseline_refresh.rs` | AC4: CLI surface roundtrip |
| `refresh_with_no_baseline_succeeds_silently` | `crates/hector-cli/tests/cli_baseline_refresh.rs` | No-op safety |

---

## Risk / rollback

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Old hector reading a new v2 baseline file | medium (downgrade scenario) | high (silent parse failure → empty baseline → noise) | Out of scope for this phase. Older hector's `serde_json::from_str` will fail; the runner already warns to stderr on parse failure (P2-6 path). Downgrade is an explicit user action, and the warning surfaces the cause. |
| Legacy warning surfaces under benign first-load conditions | low | low | Warning fires once per process via `OnceLock`. Disappears after first `hector baseline refresh`. |
| Refresh drops entries the user wanted kept (line truly moved, not gone) | low | low | The match algorithm in `contains_with_content` already preserves suppression when the line content is unchanged on a moved line. Refresh is for the intentional-reformat case; users who hit a false drop can re-baseline. |
| Verdict shape impact | none | n/a | Unchanged. |
| Trust fingerprint impact | none | n/a | Baseline file isn't config. |
| B1 / C4 conflict | low | low | Our runner edit is one line, post-loop. B1 parallelizes inside the loop; C4 adds a rule-id filter at scope-match. Conflicts should be three-way mergeable. |
| Cognitive complexity warning | medium | low | If `contains_with_content` exceeds 15, extract the 4-arm match into a private `decide(&self, key, v, content) -> bool`. |

**Rollback:** revert the runner edit + the `baseline.rs` rewrite. The new CLI subcommand becomes a no-op surface that errors with `RefreshReport` undefined, so revert the CLI changes too. A single `git revert` of the squash-merge undoes the feature cleanly.

**Forward-compat for downgrade:** an older hector binary (pre-E1) reading a v2 baseline will fail to parse and fall through the P2-6 warning to an empty baseline. We accept the noise as the price of the schema bump. If we wanted full forward-compat we'd dual-write both shapes — explicitly out of scope.

---

## Self-review checklist

- [ ] Every acceptance criterion has a named test.
- [ ] Legacy fixture is a verbatim copy of what `Baseline::save` would have produced under the v1 implementation (so it doubles as a regression pin).
- [ ] `cargo fmt --check` clean.
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo test --workspace` clean.
- [ ] `bash scripts/ci-coverage.sh` reports `baseline.rs` ≥ 90% region coverage.
- [ ] The runner's edit is one line; B1/C4 conflict surface is minimal.
- [ ] CLI default behavior (no subcommand) still records, preserving the existing `hector baseline` invocation.

---

## Hand-off

- Branch: `worktree-agent-ae7d0c40a77b05632` (isolated worktree off `main`).
- Five sequential phases; no parallelism payoff inside this plan.
- After merge, archive this plan to `plans/archive/` per the A3 convention.
