# Hector H2 — `hector record-verdict` subcommand

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-14-subagent-semantic-eval.md` §H2](../specs/2026-05-14-subagent-semantic-eval.md)
**Severity:** 🔴 critical (scaffolding for the Claude Code adapter's interpreter skill; H3 depends on this + H1 shipping)
**Sequencing:** 0.2.x cohort, independent of H1 — both can ship in parallel.

---

**Goal:** Add `hector record-verdict --rule <id> --verdict <pass|violation> [--file <path>] [--dir <path>]` — a tiny CLI subcommand that appends one `LogEntry::SemanticVerdict` record to `.hector/log.jsonl`. Consumed by the Claude Code adapter's interpreter skill (H3) after it parses subagent output, so coverage reports (`hector coverage`, D2) reflect subagent-evaluated rules instead of treating them as dead. No new core surface; reuses the existing `telemetry::append` + `LogEntry::SemanticVerdict` shipped in D1.

**Architecture:** New file `crates/hector-cli/src/commands/record_verdict.rs` owns a single `pub fn run(rule: String, verdict: VerdictValue, file: Option<String>, dir: &Path) -> Result<i32>`. The clap-side enum `VerdictValue { Pass, Violation }` enforces the two-value constraint at parse time so the runtime body cannot see an invalid value. The function: resolves `dir` to `.hector/log.jsonl`, builds a `LogEntry::SemanticVerdict { ts: now_rfc3339(), rule, verdict, file }`, calls `telemetry::append`, returns `0` on success or `1` on disk failure. Never returns `2` — this command is not a gate.

**Tech Stack:** Rust, workspace-stable. No new runtime dependencies. clap (existing) for the subcommand, `chrono` (already in-tree) for the rfc3339 timestamp, `assert_cmd` + `tempfile` (existing dev-deps) for the CLI integration test, `serde_json` (workspace) for asserting the appended line shape.

---

## Decisions ratified up-front (per spec §H2)

| Decision | Choice | Reason / source |
|---|---|---|
| `--verdict` accepted values | clap `ValueEnum` with exactly two arms: `Pass`, `Violation`. Anything else is a parse error (exit 2 from clap, not from our code). | Spec §H2 step 1. The two-value constraint is enforced at the boundary, not inside the function body. |
| `--rule` cardinality | Single occurrence (not repeatable). Each subagent verdict is one rule. | Spec §H2 step 1. The skill makes one call per (rule, file) pair; batching would couple H2 to skill internals. |
| `--file` semantics | Optional. When omitted, the appended record has `file: null` matching `LogEntry::SemanticVerdict.file: Option<String>` semantics. The CLI does not coerce empty string → None; clap rejects `--file ""` as required-value-empty. | Spec §H2 step 1; mirrors how the D1 typed-telemetry shape already serializes `Option<String>`. |
| `--dir` default | `.` (cwd). Same convention as `init` / `doctor` / `explain`. | Locates `.hector/log.jsonl` for tests via a tempdir. |
| `.hector/` auto-create | Yes, `telemetry::append` already calls `std::fs::create_dir_all(parent)?` so a missing `.hector/` directory is created on first append. The first append in a fresh project does not require a prior `hector init` or `hector session start`. | Matches existing `telemetry::append` behaviour; see `crates/hector-core/src/telemetry.rs::append`. |
| Exit code on success | `0`. | Spec §H2 step 3. |
| Exit code on telemetry-write failure | `1`. Stderr carries the io::Error message. | Spec §H2 step 3. Distinct from clap parse failures (handled by clap, usually exit 2). |
| Hidden from `--help` | **No.** Visible. Documented as adapter-internal in `docs/record-verdict.md`. | Spec §H2 step 4 explicitly says no — future contributors will want to inspect it. |
| Auth model | None. No HMAC, no nonce. The trust model is unchanged: anyone who can run `hector record-verdict` can also append to `.hector/log.jsonl` directly. | Spec §H2 Notes. Auth would be theater since it lives on the same machine as the log. |
| `session_init` lazy stamp | Reuse the same lazy `SessionInit` write `hector session record` performs (`crates/hector-cli/src/commands/session.rs`). The first `record-verdict` in a session writes a `SessionInit` record before its `SemanticVerdict`. Operators expect the session log to start with `session_init`; no need for a separate `hector session start` precondition. | Mirrors `session record`'s D1 behaviour. The `read_all` lifter doesn't synthesize `SessionInit` from `SemanticVerdict` alone, so without the lazy stamp the legacy fallback path would emit a deprecation warning for fresh logs. |
| Trust gate | **Skipped.** This subcommand does not load `.hector.yml`. No trust check, no fingerprint verify. | The command writes telemetry; it does not consult rule definitions. Trust is only required for paths that *execute* rule logic. |

---

## File structure

```
crates/hector-cli/
├── src/
│   ├── cli.rs                          ← MODIFIED: add RecordVerdict variant
│   ├── main.rs                         ← MODIFIED: dispatch RecordVerdict to commands::record_verdict::run
│   └── commands/
│       ├── mod.rs                      ← MODIFIED: pub mod record_verdict
│       └── record_verdict.rs           ← NEW: run + clap ValueEnum for --verdict
└── tests/
    └── cli_e2e_record_verdict.rs       ← NEW: assert_cmd integration tests

docs/
└── record-verdict.md                   ← NEW: adapter-internal docs

CHANGELOG.md                            ← MODIFIED: Unreleased entry
plans/README.md                         ← MODIFIED (final task only): mark archived
```

The command is ~30 lines of real code. The clap `ValueEnum` adds five more for the two-arm enum + derives. The lazy `SessionInit` stamp is one call to an existing helper. Cognitive complexity per function: ≤ 4.

---

## Risk / rollback

**Verdict-schema impact.** None. `Verdict` is not constructed by this command.

**Telemetry-schema impact.** None. `LogEntry::SemanticVerdict` already exists (shipped in D1). This command is a writer for the existing variant.

**Exit-code-contract impact.** New exit-code surface (`0` / `1`) on a new subcommand. The locked `check` contract is untouched. The adapter does not interpret `record-verdict`'s exit code as a verdict signal — it logs failures and moves on.

**Config-schema impact.** None. Does not read `.hector.yml`.

**Performance.** One file open + one `write_all` + one fsync (via `telemetry::append`). Under 5ms on any healthy filesystem.

**Rollback.** Pure addition. Removing the `RecordVerdict` variant, the dispatch arm, the command module, the test, and the docs entry restores the prior CLI surface. The on-disk `.hector/log.jsonl` records written via this command remain valid `SemanticVerdict` entries and continue to parse via `telemetry::read_all`.

**Coexistence with H1/H3.** Independent of H1 — `record-verdict` does not read or produce `DeferredVerdict`. Consumed by H3 (adapter skill) after the subagent returns.

---

## Phase 1 — Subcommand wiring (clap variant + dispatch skeleton)

Smallest piece. Adds the variant, the dispatch, and a stub function that returns exit 0 without writing anything. The next phase adds the actual append.

### Task 1.1: Failing test — subcommand exists and is recognised by clap

**Files:**
- Create: `crates/hector-cli/tests/cli_e2e_record_verdict.rs`

- [ ] **Step 1.1.1: Write the failing test**

```rust
//! H2 — end-to-end coverage that `hector record-verdict` appends a
//! `SemanticVerdict` line to `.hector/log.jsonl`.

use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

#[test]
fn record_verdict_subcommand_is_recognised() {
    // Phase 1: just confirm the subcommand exists. Real append is verified
    // in Phase 2's test (overrides this minimal check).
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule").arg("no-debug")
        .arg("--verdict").arg("pass")
        .arg("--dir").arg(tmp.path())
        .assert()
        .code(0);
}

#[test]
fn record_verdict_rejects_invalid_verdict_value() {
    // clap-enforced. Anything other than `pass` or `violation` errors at
    // parse time. We do NOT use code 1 here — clap exits with its own code
    // (2 on most platforms) for parse errors. The body of `run` is never
    // entered.
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule").arg("no-debug")
        .arg("--verdict").arg("fail") // not in the enum
        .arg("--dir").arg(tmp.path())
        .assert()
        .failure();
}
```

- [ ] **Step 1.1.2: Run test to verify it fails**

Run: `cargo test -p hector-cli --test cli_e2e_record_verdict record_verdict_subcommand_is_recognised`
Expected: FAIL — `unrecognized subcommand 'record-verdict'`.

### Task 1.2: Add the clap variant and dispatch skeleton

**Files:**
- Modify: `crates/hector-cli/src/cli.rs` (add a `RecordVerdict` variant to the `Command` enum)
- Modify: `crates/hector-cli/src/commands/mod.rs` (add `pub mod record_verdict;`)
- Create: `crates/hector-cli/src/commands/record_verdict.rs` (stub `run`)
- Modify: `crates/hector-cli/src/main.rs` (dispatch the new variant)

- [ ] **Step 1.2.1: Add the `Command::RecordVerdict` variant**

In `cli.rs`, alongside the existing variants:

```rust
    /// Append one `semantic_verdict` record to `.hector/log.jsonl`.
    ///
    /// Adapter-internal: consumed by the Claude Code interpreter skill
    /// after a subagent evaluates a deferred semantic rule. See
    /// `docs/record-verdict.md` for the wire-format contract.
    RecordVerdict {
        /// Rule id this verdict is for (single occurrence — one verdict per call).
        #[arg(long = "rule")]
        rule: String,
        /// Verdict value: `pass` or `violation`. Other values rejected at parse time.
        #[arg(long = "verdict", value_enum)]
        verdict: crate::commands::record_verdict::VerdictValue,
        /// Optional file path the verdict pertains to. When omitted, the
        /// appended record has `file: null`.
        #[arg(long = "file")]
        file: Option<String>,
        /// Directory containing `.hector/log.jsonl`. Defaults to cwd.
        #[arg(long = "dir", default_value = ".")]
        dir: PathBuf,
    },
```

- [ ] **Step 1.2.2: Create the stub command module**

`crates/hector-cli/src/commands/record_verdict.rs`:

```rust
//! H2: `hector record-verdict` — append a single `SemanticVerdict`
//! record to `.hector/log.jsonl`. Consumed by the Claude Code
//! interpreter skill after a subagent evaluates a deferred semantic
//! rule.

use anyhow::Result;
use clap::ValueEnum;
use std::path::Path;

/// Two-arm enum enforcing `--verdict pass | violation` at clap-parse
/// time. Anything else is a parse error from clap — the runtime body
/// of [`run`] cannot see an invalid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum VerdictValue {
    Pass,
    Violation,
}

impl VerdictValue {
    fn as_wire_str(self) -> &'static str {
        // The on-disk wire format mirrors bully's `pass` / `violation`
        // (lowercase). The `LogEntry::SemanticVerdict.verdict: String`
        // field is intentionally stringly-typed at the telemetry layer
        // so future extensions don't require a schema bump.
        match self {
            VerdictValue::Pass => "pass",
            VerdictValue::Violation => "violation",
        }
    }
}

pub fn run(
    rule: String,
    verdict: VerdictValue,
    file: Option<String>,
    dir: &Path,
) -> Result<i32> {
    // Phase 1 stub — Phase 2 fills in the actual append.
    let _ = (rule, verdict, file, dir);
    Ok(0)
}
```

- [ ] **Step 1.2.3: Register the module**

`crates/hector-cli/src/commands/mod.rs`:

```rust
pub mod record_verdict;
```

- [ ] **Step 1.2.4: Dispatch in main.rs**

Find the existing `match cli.command` block in `main.rs` (`grep -n 'match cli.command' crates/hector-cli/src/main.rs`). Add the arm:

```rust
        Command::RecordVerdict { rule, verdict, file, dir } => {
            std::process::exit(commands::record_verdict::run(rule, verdict, file, &dir)?)
        }
```

- [ ] **Step 1.2.5: Run the phase-1 tests**

Run: `cargo test -p hector-cli --test cli_e2e_record_verdict`
Expected: PASS, both tests (the subcommand is recognised, invalid verdict is parse-rejected).

- [ ] **Step 1.2.6: Commit**

```bash
git add crates/hector-cli/src/cli.rs crates/hector-cli/src/main.rs crates/hector-cli/src/commands/mod.rs crates/hector-cli/src/commands/record_verdict.rs crates/hector-cli/tests/cli_e2e_record_verdict.rs
git commit -m "feat(cli): scaffold record-verdict subcommand (H2 phase 1)

Add the clap variant, two-arm VerdictValue enum, and stub run().
Phase 2 fills in the actual telemetry append."
```

---

## Phase 2 — Implementation: append one `SemanticVerdict` record

### Task 2.1: Failing test — appended record has the expected shape

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_record_verdict.rs` (replace the Phase 1 placeholder test with a real assertion)

- [ ] **Step 2.1.1: Add the real test**

Append (do not delete the Phase 1 tests):

```rust
#[test]
fn record_verdict_appends_one_semantic_verdict_line() {
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule").arg("no-debug")
        .arg("--verdict").arg("violation")
        .arg("--file").arg("src/foo.rs")
        .arg("--dir").arg(tmp.path())
        .assert()
        .code(0);

    let log_path = tmp.path().join(".hector/log.jsonl");
    assert!(log_path.exists(), ".hector/log.jsonl must be created");
    let content = fs::read_to_string(&log_path).unwrap();
    // The log MAY contain a leading `session_init` record (lazy stamp).
    // We assert there is exactly one `semantic_verdict` line and that its
    // fields match what we passed in.
    let semantic_lines: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("\"type\":\"semantic_verdict\""))
        .collect();
    assert_eq!(
        semantic_lines.len(),
        1,
        "expected exactly one semantic_verdict line, got: {content}"
    );
    let v: serde_json::Value = serde_json::from_str(semantic_lines[0]).unwrap();
    assert_eq!(v["type"], "semantic_verdict");
    assert_eq!(v["rule"], "no-debug");
    assert_eq!(v["verdict"], "violation");
    assert_eq!(v["file"], "src/foo.rs");
    assert!(
        v["ts"].as_str().unwrap().contains("T"),
        "ts must be rfc3339 (contains 'T'), got {:?}",
        v["ts"]
    );
}

#[test]
fn record_verdict_with_no_file_omits_field() {
    // `LogEntry::SemanticVerdict.file` is `Option<String>` serialized with
    // `skip_serializing_if = "Option::is_none"` — when omitted on the
    // command line, the on-disk line has no `file` key at all.
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule").arg("no-debug")
        .arg("--verdict").arg("pass")
        .arg("--dir").arg(tmp.path())
        .assert()
        .code(0);

    let content = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
    let line = content
        .lines()
        .find(|l| l.contains("\"type\":\"semantic_verdict\""))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert!(
        v.get("file").is_none(),
        "file key must be absent when --file is omitted; got {line}"
    );
}

#[test]
fn record_verdict_writes_session_init_lazily() {
    // The first record-verdict in a fresh project stamps a session_init
    // record before its semantic_verdict, so `hector coverage` (D2) and
    // the legacy-format lifter see a well-formed log.
    let tmp = tempdir().unwrap();
    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule").arg("r1")
        .arg("--verdict").arg("pass")
        .arg("--dir").arg(tmp.path())
        .assert()
        .code(0);

    let content = fs::read_to_string(tmp.path().join(".hector/log.jsonl")).unwrap();
    let first_line = content.lines().next().expect("at least one line");
    let v: serde_json::Value = serde_json::from_str(first_line).unwrap();
    assert_eq!(
        v["type"], "session_init",
        "first record in a fresh log must be session_init, got: {first_line}"
    );
}
```

- [ ] **Step 2.1.2: Run tests to verify they fail**

Run: `cargo test -p hector-cli --test cli_e2e_record_verdict`
Expected: the three new tests FAIL (the stub `run()` returns `Ok(0)` without writing).

### Task 2.2: Implement the append

**Files:**
- Modify: `crates/hector-cli/src/commands/record_verdict.rs` (replace the stub `run` body)

- [ ] **Step 2.2.1: Implement `run`**

```rust
pub fn run(
    rule: String,
    verdict: VerdictValue,
    file: Option<String>,
    dir: &Path,
) -> Result<i32> {
    let log_path = dir.join(".hector/log.jsonl");

    // Lazy session_init: if the log is empty/absent, write a session_init
    // record first so the log starts with the canonical first-record type.
    // Idempotent in the same sense as `hector session record` — we check
    // the file presence; we do not parse existing records to avoid an O(n)
    // read for every append.
    if !log_path.exists() || std::fs::metadata(&log_path).map(|m| m.len() == 0).unwrap_or(true) {
        let init = hector_core::telemetry::LogEntry::SessionInit {
            ts: chrono::Utc::now().to_rfc3339(),
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: hector_core::telemetry::SCHEMA_VERSION,
        };
        if let Err(e) = hector_core::telemetry::append(&log_path, &init) {
            eprintln!("ERROR: failed to write session_init: {e:#}");
            return Ok(1);
        }
    }

    let entry = hector_core::telemetry::LogEntry::SemanticVerdict {
        ts: chrono::Utc::now().to_rfc3339(),
        rule,
        verdict: verdict.as_wire_str().to_string(),
        file,
    };

    if let Err(e) = hector_core::telemetry::append(&log_path, &entry) {
        eprintln!("ERROR: failed to append semantic_verdict: {e:#}");
        return Ok(1);
    }

    Ok(0)
}
```

Verify the exact `SessionInit` variant field names against `crates/hector-core/src/telemetry.rs` — the spec uses `hector_version` and `schema_version` per D1's wire format. If the variant differs (e.g. takes more fields), match the actual definition; the spec confirmed shape was `session_init` carrying `ts` / `hector_version` / `schema_version`. Adjust the `telemetry::SCHEMA_VERSION` constant reference to match the actual public name (could be `SCHEMA_VERSION` or `TELEMETRY_SCHEMA_VERSION` — `grep -n 'pub const' crates/hector-core/src/telemetry.rs`).

- [ ] **Step 2.2.2: Run tests to verify they pass**

Run: `cargo test -p hector-cli --test cli_e2e_record_verdict`
Expected: PASS, all five tests (two from Phase 1, three from this phase).

- [ ] **Step 2.2.3: Run the workspace tests for regressions**

Run: `cargo test --workspace`
Expected: previous totals, all green.

- [ ] **Step 2.2.4: Commit**

```bash
git add crates/hector-cli/src/commands/record_verdict.rs crates/hector-cli/tests/cli_e2e_record_verdict.rs
git commit -m "feat(cli): record-verdict appends SemanticVerdict (H2 phase 2)

Append one LogEntry::SemanticVerdict per invocation to
.hector/log.jsonl, lazily writing session_init on the first call
to a fresh log. Reuses telemetry::append for atomic writes; no
new core surface."
```

---

## Phase 3 — Error path: telemetry-write failure exits 1

### Task 3.1: Failing test — write failure surfaces as exit 1

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_record_verdict.rs` (append a test)

- [ ] **Step 3.1.1: Add the test**

```rust
#[cfg(unix)]
#[test]
fn record_verdict_returns_1_on_telemetry_write_failure() {
    // Point --dir at a read-only directory. telemetry::append's
    // create_dir_all + open(append) chain will fail, and run() must
    // return 1 with a stderr message.
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    let readonly = tmp.path().join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    let mut perms = std::fs::metadata(&readonly).unwrap().permissions();
    perms.set_mode(0o500); // r-x for owner; no write
    std::fs::set_permissions(&readonly, perms).unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .arg("record-verdict")
        .arg("--rule").arg("r1")
        .arg("--verdict").arg("pass")
        .arg("--dir").arg(&readonly)
        .assert()
        .code(1)
        .stderr(predicates::str::contains("ERROR:"));

    // Cleanup: restore write so the tempdir teardown succeeds.
    let mut perms = std::fs::metadata(&readonly).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&readonly, perms).unwrap();
}
```

- [ ] **Step 3.1.2: Run the test**

Run: `cargo test -p hector-cli --test cli_e2e_record_verdict record_verdict_returns_1_on_telemetry_write_failure`
Expected: PASS — the Phase 2 implementation already returns `Ok(1)` on `append` failure. If it FAILs (e.g. the directory is writable as root, or macOS's APFS ignores the perms), tighten the test setup (e.g. use a file path instead of a directory: `--dir /dev/null/x` — `/dev/null/anything` is guaranteed to fail `create_dir_all`).

- [ ] **Step 3.1.3: Commit**

```bash
git add crates/hector-cli/tests/cli_e2e_record_verdict.rs
git commit -m "test(cli): record-verdict returns 1 on telemetry write failure (H2 phase 3)"
```

---

## Phase 4 — Docs

### Task 4.1: Write `docs/record-verdict.md`

**Files:**
- Create: `docs/record-verdict.md`

- [ ] **Step 4.1.1: Document the contract**

```markdown
# `hector record-verdict`

Adapter-internal subcommand. Appends one `semantic_verdict` record to
`.hector/log.jsonl` so subagent-evaluated rules show up in coverage
reports (`hector coverage`, D2) instead of looking dead.

Consumed by the Claude Code adapter's interpreter skill after it parses
a subagent's pass/violation answer for a deferred semantic or session
rule.

## Synopsis

\`\`\`
hector record-verdict --rule <id> --verdict <pass|violation> [--file <path>] [--dir <path>]
\`\`\`

| Flag | Required | Default | Notes |
|---|---|---|---|
| `--rule` | yes | — | Rule id this verdict is for. Single occurrence. |
| `--verdict` | yes | — | Exactly `pass` or `violation`. Other values rejected at clap-parse time. |
| `--file` | no | omitted | File path the verdict pertains to. When absent, the on-disk record has no `file` field. |
| `--dir` | no | `.` | Directory containing `.hector/log.jsonl`. Created if it doesn't exist. |

## Wire format

Appends one line of the form:

\`\`\`json
{"type":"semantic_verdict","ts":"2026-05-14T12:34:56.789Z","rule":"no-debug","verdict":"violation","file":"src/foo.rs"}
\`\`\`

(`file` omitted when `--file` is not passed.)

The first invocation against a fresh `.hector/log.jsonl` stamps a
`session_init` record before the `semantic_verdict`. See
[`docs/telemetry.md`](./telemetry.md) for the full wire-format reference.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Record appended successfully. |
| `1` | Telemetry write failure (disk full, permissions, parent directory unwritable). Stderr carries the io::Error. |
| (`2`) | clap parse error — invalid `--verdict` value, missing `--rule`, etc. Not returned by our code. |

`record-verdict` is **not** a gate. It never returns `2` from our code
and the adapter does not treat a non-zero exit as a verdict signal — it
logs the failure and moves on.

## Trust model

None. No HMAC, no nonce, no signing. An attacker who can run
`hector record-verdict` can also write to `.hector/log.jsonl` directly.
The subcommand is convenience, not security. See `docs/security.md`
for the project's overall trust model.
```

- [ ] **Step 4.1.2: Commit**

```bash
git add docs/record-verdict.md
git commit -m "docs(h2): document hector record-verdict adapter-internal contract"
```

---

## Phase 5 — CHANGELOG + plan archive

### Task 5.1: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md` (under `## Unreleased`)

- [ ] **Step 5.1.1: Add the entry**

Insert alongside the H1 entry (or after, if H1 lands first):

```markdown
### Subagent semantic-eval — `hector record-verdict` (H2)

- New CLI subcommand `hector record-verdict --rule <id> --verdict <pass|violation> [--file <path>] [--dir <path>]`. Appends one `LogEntry::SemanticVerdict` record to `.hector/log.jsonl` so subagent-evaluated rules show up in coverage reports. Consumed by the Claude Code adapter's interpreter skill (H3, separate plan).
- `--verdict` is a clap `ValueEnum`; invalid values are rejected at parse time.
- First invocation against a fresh log lazily stamps a `session_init` record so the log starts with the canonical first-record type.
- Exit codes: `0` success, `1` telemetry write failure. Never `2` — `record-verdict` is not a gate.
- Wire format and trust model documented in [`docs/record-verdict.md`](docs/record-verdict.md).
- **Library-additive only.** No new core surface; reuses `hector_core::telemetry::{append, LogEntry::SemanticVerdict}` shipped in D1.
```

- [ ] **Step 5.1.2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): record H2 hector record-verdict subcommand"
```

### Task 5.2: Archive the plan

**Files:**
- Move: `plans/2026-05-14-hector-h2-record-verdict.md` → `plans/archive/`
- Modify: `plans/README.md` (move H2 from the Future section to Archive)

- [ ] **Step 5.2.1: Move the plan**

```bash
git mv plans/2026-05-14-hector-h2-record-verdict.md plans/archive/
```

- [ ] **Step 5.2.2: Update plans/README.md**

In the Future section, edit the H1-H4 bullet to reflect H2 shipped. In the Archive section, add:

```markdown
- [`2026-05-14-hector-h2-record-verdict`](archive/2026-05-14-hector-h2-record-verdict.md) — `hector record-verdict` subcommand appends one `SemanticVerdict` record to `.hector/log.jsonl`; consumed by the Claude Code interpreter skill (H3) to keep coverage reports accurate under subagent-mediated semantic eval.
```

- [ ] **Step 5.2.3: Commit**

```bash
git add plans/
git commit -m "docs(plans): archive H2 record-verdict plan"
```

---

## Acceptance criteria check (against spec §H2)

| Spec criterion | Covered by |
|---|---|
| `hector record-verdict --rule r1 --verdict pass --file foo.rs` appends one valid `semantic_verdict` line | `cli_e2e_record_verdict::record_verdict_appends_one_semantic_verdict_line` (Phase 2) |
| Invalid `--verdict` values error at clap parse time with exit `1` (spec says 1; clap typically exits 2 for parse errors — verify behaviour and document the discrepancy if it surfaces; the spec's "exit 1" likely refers to "non-zero", which clap's 2 satisfies) | `cli_e2e_record_verdict::record_verdict_rejects_invalid_verdict_value` (Phase 1) |
| `--file` omission produces a record with `file: null` (matches `Option<String>` skip-serializing-if-none semantics) | `cli_e2e_record_verdict::record_verdict_with_no_file_omits_field` (Phase 2) |
| Running in a directory with no `.hector/` initializes it (mirrors `commands/check.rs`) | Covered implicitly by every Phase-2 test using a fresh `tempdir()`. Add an explicit assertion in Phase 2's `record_verdict_appends_one_semantic_verdict_line` test (`assert!(log_path.exists())`) — already present in the test above. |

If the clap parse-error exit code turns out to be 2 rather than 1, update the spec acceptance criterion to match observed behaviour and document the rationale (clap is consistent across the rest of the CLI surface; a custom exit override would be inconsistent).
