# Hector Gates Redesign — Plan 1: Core Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hector's `engine`/`script`/`ast` rule model with the minimal `gates:` format so that `hector check` runs a gate command per matching file and proxies exit code 2 as a Block.

**Architecture:** A gate is `files` (globs) + `run` (a shell string). Hector matches touched files to gates, runs each `run` once per matching file with the ABI materialized as environment (`HECTOR_FILE`, `HECTOR_ROOT`, `HECTOR_EVENT`) plus proposed content on stdin, and reads only the exit code: `2` → Block, `126/127/≥128/timeout` → InternalError, everything else → Pass. On Block the gate's stdout+stderr is the message. No string templating, no per-rule engines, no severity.

**Tech Stack:** Rust (Cargo workspace, crates `hector-core` + `hector-cli`), `serde`/`serde_yaml`/`serde_json`, `wait-timeout` (new) for process timeouts, `insta` for snapshots, `assert_cmd` for CLI e2e.

**Spec:** `specs/2026-06-15-hector-gates-redesign-design.md`. This plan implements §2–§5, §9–§11 (core engine + `check`). Trust (§6), verify/doctor (§7), and adapters (§4 adapter side) are Plans 2–4.

**Out of scope for Plan 1 (deliberately):**
- The direnv trust store (§6) — Plan 2. Plan 1 *removes* the `trust::verify` call from `load` and leaves `trust.rs` unwired.
- `hector verify` / `doctor` upgrades (§7) — Plan 3.
- Adapter `--event` plumbing (§4) — Plan 4. Plan 1 adds the `--event` flag with a `manual` default so the contract exists.

---

## File Structure

**hector-core (`crates/hector-core/src/`):**
- `config/types.rs` — rewritten: `Config { extends, execution, gates }`, `Gate { files, run }`, `ExecutionConfig { timeout_secs, max_workers }`. Deletes `Rule`, `EngineKind`, `Severity`, `Capabilities`, `WritesPolicy`, `OutputMode`.
- `config/parser.rs` — rewritten: parse new `Config`, reject legacy shapes with a curated error. Deletes `SUPPORTED_SCHEMAS`, `is_legacy`, `peek_schema_version`.
- `config/mod.rs` — updated re-exports.
- `engine/gate.rs` — **new**: `run_gate`, `GateEnv`, `GateOutcome`, `InternalReason`.
- `engine/mod.rs` — collapsed: re-exports `gate::*`. Deletes `RuleEngine`, `RuleContext`.
- `engine/ast.rs`, `engine/output.rs`, `engine/capability.rs`, `engine/script.rs` — **deleted**.
- `verdict.rs` — rewritten to the §9 shape. `SCHEMA_VERSION = 4`.
- `runner.rs` — rewritten core: `HectorEngine { config, root, options }`, `check`, `gate_matches_path`, `check_with_explain`. Deletes engine-trait dispatch.
- `telemetry.rs` — retargeted: `PerGateRecord`, `LogEntry::Check.gates`, `SCHEMA_VERSION = 3`, legacy reader removed.
- `disable.rs` — directive vocabulary becomes gate ids (mostly comment/doc changes; logic is id-agnostic).
- `baseline.rs` — **deleted**.
- `lib.rs` — drop `pub mod baseline;`.

**hector-cli (`crates/hector-cli/src/`):**
- `cli.rs` — `Check` gets `--event` and `--gate` (renamed from `--rule`); `Migrate` and `Baseline` variants removed.
- `commands/check.rs` — rewritten: per-file gate run, `--event`, emit blocks, exit-code mapping. Diff-slice machinery removed.
- `commands/migrate.rs`, `commands/baseline.rs` — **deleted**.
- `commands/mod.rs` — drop the deleted modules.
- `commands/validate.rs`, `commands/explain.rs`, `commands/show_resolved_config.rs`, `commands/guide.rs` — minimal edits to compile against new types.

**Tests:** `crates/hector-core/src/**` unit tests inline; `crates/hector-cli/tests/cli_e2e_gates.rs` new; fixtures under `tests/fixtures/gates/`.

---

## Task 1: New config types

**Files:**
- Modify: `crates/hector-core/src/config/types.rs` (full rewrite)

- [ ] **Step 1: Write the failing tests**

Replace the entire contents of `crates/hector-core/src/config/types.rs` test module with these (keep at top of file for now; implementation follows in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_gate_with_files_list() {
        let cfg: Config = serde_yaml::from_str(
            "gates:\n  biome:\n    files: [\"**/*.ts\"]\n    run: \"biome check\"\n",
        )
        .unwrap();
        let g = cfg.gates.get("biome").unwrap();
        assert_eq!(g.files, vec!["**/*.ts".to_string()]);
        assert_eq!(g.run, "biome check");
    }

    #[test]
    fn files_accepts_a_bare_string() {
        let cfg: Config = serde_yaml::from_str(
            "gates:\n  g:\n    files: \"**/*.rs\"\n    run: \"true\"\n",
        )
        .unwrap();
        assert_eq!(cfg.gates["g"].files, vec!["**/*.rs".to_string()]);
    }

    #[test]
    fn execution_timeout_defaults_to_30() {
        let cfg: Config =
            serde_yaml::from_str("gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        assert_eq!(cfg.execution.timeout_secs, 30);
    }

    #[test]
    fn execution_timeout_is_overridable() {
        let cfg: Config = serde_yaml::from_str(
            "execution:\n  timeout_secs: 5\ngates:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
        )
        .unwrap();
        assert_eq!(cfg.execution.timeout_secs, 5);
    }

    #[test]
    fn extends_defaults_to_empty() {
        let cfg: Config =
            serde_yaml::from_str("gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        assert!(cfg.extends.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hector-core config::types`
Expected: FAIL — `Config`/`Gate` no longer have the old shape (compile errors referencing removed fields).

- [ ] **Step 3: Write the implementation**

Replace everything *above* the test module in `crates/hector-core/src/config/types.rs` with:

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub execution: ExecutionConfig,
    pub gates: BTreeMap<String, Gate>,
}

/// Optional execution-tuning block.
///
/// `timeout_secs` bounds each gate's wall-clock; a gate that exceeds it is
/// killed and reported as InternalError (never a silent pass). The
/// `HECTOR_TIMEOUT` env var overrides this at run time. `max_workers` tunes
/// the rayon pool that dispatches gates in parallel; `0` clamps to 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub max_workers: usize,
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout_secs(),
            max_workers: 0,
        }
    }
}

/// A single gate: match `files`, run `run`, read its exit code.
///
/// `run` is handed to the shell verbatim — no `{file}`/`{path}` templating.
/// The path under check arrives as `$HECTOR_FILE`; proposed content arrives
/// on stdin. `run` may be an inline command or a path to a script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gate {
    #[serde(deserialize_with = "files_one_or_many")]
    pub files: Vec<String>,
    pub run: String,
}

/// Accept either a single glob string or a list of globs for `files`.
/// Mirrors the old `scope` deserializer (bully parity).
fn files_one_or_many<'de, D>(de: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_yaml::Value::deserialize(de)?;
    match v {
        serde_yaml::Value::String(s) => Ok(vec![s]),
        serde_yaml::Value::Sequence(seq) => seq
            .into_iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| D::Error::custom("files entry must be string"))
            })
            .collect(),
        _ => Err(D::Error::custom("files must be string or list of strings")),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p hector-core config::types`
Expected: PASS (5 tests). Note: the crate as a whole will NOT compile yet — `parser.rs`, `runner.rs`, etc. still reference the old types. That's fixed in later tasks. To run just these tests before the crate compiles, temporarily comment out the offending modules is NOT needed — `cargo test` of one module still requires the crate to build. **Therefore: do Tasks 1–10 as one compile unit; commit at Step 5 of each task only after `cargo build -p hector-core` succeeds at that point in the sequence.** If a task's tests can't run because downstream modules are mid-migration, that is expected — the task is "done" when its code is written per spec, and the phase's final task (Task 11) is where the whole thing goes green.

> **Sequencing note for the executor:** this is a coordinated rewrite. Treat Tasks 1–10 as a single landing. Write each task's code, and run the *targeted* test as soon as the crate compiles (typically from Task 5 onward for core, Task 9 onward for CLI). Commit per task once `cargo build` of the affected crate passes.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/config/types.rs
git commit -m "feat(config)!: gates config types (Config/Gate/ExecutionConfig)"
```

---

## Task 2: Parser — parse new config, reject legacy

**Files:**
- Modify: `crates/hector-core/src/config/parser.rs` (full rewrite)
- Modify: `crates/hector-core/src/config/mod.rs` (re-exports)

- [ ] **Step 1: Write the failing tests**

Replace the test module in `crates/hector-core/src/config/parser.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_gates_config() {
        let cfg = parse_str("gates:\n  g:\n    files: \"*.rs\"\n    run: \"true\"\n").unwrap();
        assert!(cfg.gates.contains_key("g"));
    }

    #[test]
    fn rejects_legacy_schema_version() {
        let err = parse_str("schema_version: 2\nrules: {}\n").unwrap_err().to_string();
        assert!(err.contains("gates"), "error should point at the gates format: {err}");
    }

    #[test]
    fn rejects_legacy_rules_block() {
        let err = parse_str("rules:\n  r:\n    engine: script\n").unwrap_err().to_string();
        assert!(err.contains("gates"), "error should point at the gates format: {err}");
    }

    #[test]
    fn missing_gates_key_is_an_error() {
        assert!(parse_str("extends: []\n").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo build -p hector-core 2>&1 | head` — expect compile errors referencing removed `SUPPORTED_SCHEMAS`/`EngineKind`.

- [ ] **Step 3: Write the implementation**

Replace the non-test contents of `crates/hector-core/src/config/parser.rs` with:

```rust
use super::types::Config;
use anyhow::{anyhow, Context, Result};

/// Parse a `.hector.yml` (gates format).
///
/// Legacy v1/v2 configs (`schema_version:`, `rules:`, `engine:`) are rejected
/// with a curated message rather than serde's generic failure — hector 0.3
/// dropped the engine model. There is no migration path (no install base).
pub fn parse_str(input: &str) -> Result<Config> {
    if let Some(key) = legacy_marker(input) {
        return Err(anyhow!(
            "this looks like a pre-0.3 config (found `{key}:`). The 0.3 format uses a \
             top-level `gates:` map of `{{ files, run }}` entries — rewrite it. \
             See specs/2026-06-15-hector-gates-redesign-design.md"
        ));
    }
    let cfg: Config = serde_yaml::from_str(input).context("parsing hector config")?;
    Ok(cfg)
}

/// Return the first top-level legacy marker key present, if any.
fn legacy_marker(input: &str) -> Option<&'static str> {
    let value: serde_yaml::Value = serde_yaml::from_str(input).ok()?;
    let map = value.as_mapping()?;
    for key in ["schema_version", "rules", "trust"] {
        if map.contains_key(serde_yaml::Value::String(key.into())) {
            return Some(match key {
                "schema_version" => "schema_version",
                "rules" => "rules",
                _ => "trust",
            });
        }
    }
    None
}

pub fn parse_file(path: &std::path::Path) -> Result<Config> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_str(&content)
}
```

Then update `crates/hector-core/src/config/mod.rs` re-exports:

```rust
pub use parser::{parse_file, parse_str};
```

(Remove `is_legacy`, `peek_schema_version`, `SUPPORTED_SCHEMAS` from the `pub use`.)

- [ ] **Step 4: Run tests** — deferred to Task 11 (crate not yet compiling). Code is written to spec.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/config/parser.rs crates/hector-core/src/config/mod.rs
git commit -m "feat(config)!: parse gates format, reject legacy configs"
```

---

## Task 3: Gate engine — `run_gate`

**Files:**
- Modify: `crates/hector-core/Cargo.toml` (add `wait-timeout`)
- Create: `crates/hector-core/src/engine/gate.rs`

- [ ] **Step 1: Add the dependency**

In `crates/hector-core/Cargo.toml` under `[dependencies]`, add:

```toml
wait-timeout = "0.2"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/hector-core/src/engine/gate.rs` with the test module first (implementation in Step 4):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn env_for(file: &std::path::Path, root: &std::path::Path) -> GateEnv<'_> {
        GateEnv { file, root, event: "manual" }
    }

    fn t() -> Duration {
        Duration::from_secs(10)
    }

    #[test]
    fn exit_zero_is_pass() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("true", &env_for(&f, dir.path()), None, t());
        assert!(matches!(out, GateOutcome::Pass));
    }

    #[test]
    fn exit_one_is_pass() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 1", &env_for(&f, dir.path()), None, t());
        assert!(matches!(out, GateOutcome::Pass), "exit 1 must be Pass (opt-in blocking)");
    }

    #[test]
    fn exit_two_is_block_with_message() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("echo problem >&2; exit 2", &env_for(&f, dir.path()), None, t());
        match out {
            GateOutcome::Block { message } => assert_eq!(message, "problem"),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn block_with_no_output_is_empty_message() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("exit 2", &env_for(&f, dir.path()), None, t());
        match out {
            GateOutcome::Block { message } => assert_eq!(message, ""),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn command_not_found_is_internal() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("definitely-not-a-real-binary-xyz", &env_for(&f, dir.path()), None, t());
        assert!(matches!(out, GateOutcome::Internal(InternalReason::NotFound)));
    }

    #[test]
    fn timeout_is_internal() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        let out = run_gate("sleep 5", &env_for(&f, dir.path()), None, Duration::from_millis(200));
        assert!(matches!(out, GateOutcome::Internal(InternalReason::Timeout)));
    }

    #[test]
    fn hector_file_env_is_exported() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("target.txt");
        // gate blocks iff $HECTOR_FILE ends with target.txt
        let out = run_gate(
            "case \"$HECTOR_FILE\" in *target.txt) exit 2;; *) exit 0;; esac",
            &env_for(&f, dir.path()),
            None,
            t(),
        );
        assert!(matches!(out, GateOutcome::Block { .. }));
    }

    #[test]
    fn proposed_content_arrives_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        // block iff stdin contains "FORBIDDEN"
        let out = run_gate(
            "grep -q FORBIDDEN && exit 2 || exit 0",
            &env_for(&f, dir.path()),
            Some(b"line\nFORBIDDEN\n"),
            t(),
        );
        assert!(matches!(out, GateOutcome::Block { .. }));
    }
}
```

Add `tempfile` to `[dev-dependencies]` in `crates/hector-core/Cargo.toml` if not already present (`tempfile = "3"`).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p hector-core engine::gate 2>&1 | head`
Expected: FAIL — `run_gate`/`GateOutcome` undefined.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/hector-core/src/engine/gate.rs` (above the test module):

```rust
//! The one execution model: run a gate command, read its exit code.
//!
//! Contract (see spec §3): exit `2` → Block; `126`/`127`/`≥128`/timeout →
//! InternalError; everything else → Pass. On Block the combined trimmed
//! stdout+stderr is the message.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// The ABI handed to a gate, materialized as process environment + cwd.
pub struct GateEnv<'a> {
    /// Absolute path to the file under check (`$HECTOR_FILE`).
    pub file: &'a Path,
    /// Project root; also the gate's cwd (`$HECTOR_ROOT`).
    pub root: &'a Path,
    /// Trigger: `edit` | `write` | `pre-commit` | `manual` (`$HECTOR_EVENT`).
    pub event: &'a str,
}

#[derive(Debug)]
pub enum InternalReason {
    NotFound,
    NotExecutable,
    Timeout,
    Signal(i32),
    Spawn(String),
}

impl InternalReason {
    /// Stable string for telemetry / verdict `errors[].reason`.
    pub fn as_str(&self) -> String {
        match self {
            InternalReason::NotFound => "not_found".into(),
            InternalReason::NotExecutable => "not_executable".into(),
            InternalReason::Timeout => "timeout".into(),
            InternalReason::Signal(n) => format!("signal:{n}"),
            InternalReason::Spawn(e) => format!("spawn:{e}"),
        }
    }
}

#[derive(Debug)]
pub enum GateOutcome {
    Pass,
    Block { message: String },
    Internal(InternalReason),
}

/// Run one gate against one file. Never panics; spawn failures and timeouts
/// map to `Internal`.
pub fn run_gate(
    run: &str,
    env: &GateEnv,
    content: Option<&[u8]>,
    timeout: Duration,
) -> GateOutcome {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(run)
        .current_dir(env.root)
        .env("HECTOR_FILE", env.file)
        .env("HECTOR_ROOT", env.root)
        .env("HECTOR_EVENT", env.event)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return GateOutcome::Internal(InternalReason::Spawn(e.to_string())),
    };

    // Feed proposed content on stdin from a detached thread; a gate that
    // ignores stdin closes the pipe and our write fails with EPIPE, which we
    // intentionally ignore. Without content, close stdin immediately.
    match content {
        Some(bytes) => {
            if let Some(mut stdin) = child.stdin.take() {
                let owned = bytes.to_vec();
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&owned);
                });
            }
        }
        None => drop(child.stdin.take()),
    }

    // Drain stdout/stderr on threads to avoid pipe-buffer deadlock for chatty
    // gates whose output exceeds the pipe capacity before they exit.
    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_handle = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_handle = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });

    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return GateOutcome::Internal(InternalReason::Timeout);
        }
        Err(e) => return GateOutcome::Internal(InternalReason::Spawn(e.to_string())),
    };

    let stdout = String::from_utf8_lossy(&out_handle.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_handle.join().unwrap_or_default()).into_owned();

    classify(&status, &stdout, &stderr)
}

fn classify(status: &std::process::ExitStatus, stdout: &str, stderr: &str) -> GateOutcome {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return GateOutcome::Internal(InternalReason::Signal(sig));
        }
    }
    match status.code() {
        Some(2) => {
            let message = [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            GateOutcome::Block { message }
        }
        Some(126) => GateOutcome::Internal(InternalReason::NotExecutable),
        Some(127) => GateOutcome::Internal(InternalReason::NotFound),
        Some(c) if c >= 128 => GateOutcome::Internal(InternalReason::Signal(c - 128)),
        _ => GateOutcome::Pass,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p hector-core engine::gate`
Expected: PASS (8 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/hector-core/Cargo.toml crates/hector-core/src/engine/gate.rs
git commit -m "feat(engine): gate runner with exit-code contract and timeout"
```

---

## Task 4: Collapse the engine module, delete old engines

**Files:**
- Modify: `crates/hector-core/src/engine/mod.rs`
- Delete: `crates/hector-core/src/engine/{ast,output,capability,script}.rs`
- Modify: `crates/hector-core/Cargo.toml` (drop `ast-grep-core` / related)

- [ ] **Step 1: Replace `engine/mod.rs`**

Replace the entire file with:

```rust
//! Engine module: the single gate-execution model.

pub mod gate;

pub use gate::{run_gate, GateEnv, GateOutcome, InternalReason};
```

- [ ] **Step 2: Delete the old engine files**

```bash
git rm crates/hector-core/src/engine/ast.rs \
       crates/hector-core/src/engine/output.rs \
       crates/hector-core/src/engine/capability.rs \
       crates/hector-core/src/engine/script.rs
```

- [ ] **Step 3: Drop unused dependencies**

In `crates/hector-core/Cargo.toml`, remove any dependency only used by the deleted engines (`ast-grep-core`, `ast-grep-language`, and any AST-only transitive crates). Leave `sha2`/`serde*`/`globset`/`rayon`/`fs4` — still used.

Verify nothing else references them:

```bash
grep -rn "ast_grep\|ast-grep\|capability\|engine::output\|engine::script\|engine::ast" crates/hector-core/src crates/hector-cli/src
```

Expected after later tasks: no hits. Hits remaining now (runner.rs, etc.) are fixed in Task 6.

- [ ] **Step 4: Commit**

```bash
git add -A crates/hector-core/src/engine crates/hector-core/Cargo.toml
git commit -m "refactor(engine)!: collapse to single gate model, delete ast/script/capability/output"
```

---

## Task 5: Verdict — gates shape

**Files:**
- Modify: `crates/hector-core/src/verdict.rs` (full rewrite)

- [ ] **Step 1: Write the failing tests**

Replace the test module (or add one) in `crates/hector-core/src/verdict.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_pass() {
        let v = Verdict::from_outcomes(vec![], vec![], vec![], 0);
        assert_eq!(v.status, Status::Pass);
    }

    #[test]
    fn any_block_is_block() {
        let v = Verdict::from_outcomes(
            vec![Block { gate: "g".into(), file: "f".into(), message: "m".into() }],
            vec![],
            vec![],
            0,
        );
        assert_eq!(v.status, Status::Block);
    }

    #[test]
    fn block_wins_over_internal_error() {
        let v = Verdict::from_outcomes(
            vec![Block { gate: "g".into(), file: "f".into(), message: "m".into() }],
            vec![GateError { gate: "h".into(), file: "f".into(), reason: "timeout".into() }],
            vec![],
            0,
        );
        assert_eq!(
            v.status,
            Status::Block,
            "a confirmed block must not be downgraded to fail-open by an unrelated crash"
        );
    }

    #[test]
    fn errors_only_is_internal_error() {
        let v = Verdict::from_outcomes(
            vec![],
            vec![GateError { gate: "h".into(), file: "f".into(), reason: "not_found".into() }],
            vec![],
            0,
        );
        assert_eq!(v.status, Status::InternalError);
    }

    #[test]
    fn schema_version_is_4() {
        assert_eq!(SCHEMA_VERSION, 4);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo build -p hector-core 2>&1 | head` — expect errors (old `Violation`/`Severity`/`Engine` referenced elsewhere; `from_outcomes` undefined).

- [ ] **Step 3: Write the implementation**

Replace the non-test contents of `crates/hector-core/src/verdict.rs` with:

```rust
use serde::{Deserialize, Serialize};

/// Verdict JSON schema version. Bumped to 4 for the gates redesign:
/// `Violation`/`Severity`/`Engine` removed; `blocks`/`errors` added.
pub const SCHEMA_VERSION: u32 = 4;

/// Floor schema version all current verdicts satisfy.
pub const MIN_REQUIRED_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub schema_version: u32,
    pub hector_version: String,
    pub status: Status,
    pub blocks: Vec<Block>,
    pub errors: Vec<GateError>,
    /// Gate ids that ran and passed (for `--explain` / telemetry).
    pub passed: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Status {
    Pass,
    Block,
    #[serde(rename = "internal_error")]
    InternalError,
}

/// A gate that exited 2 on a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub gate: String,
    pub file: String,
    /// Verbatim trimmed stdout+stderr from the gate.
    pub message: String,
}

/// A gate that crashed (not found / not executable / timeout / signal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateError {
    pub gate: String,
    pub file: String,
    /// Stable reason string from `InternalReason::as_str`.
    pub reason: String,
}

impl Verdict {
    pub fn pass() -> Self {
        Self::from_outcomes(vec![], vec![], vec![], 0)
    }

    /// Build a verdict from collected outcomes.
    ///
    /// Status precedence: **Block wins over InternalError** — a confirmed
    /// policy violation (exit 2) must stop the edit even if an unrelated gate
    /// crashed. Only when there are no blocks does a crash escalate to
    /// InternalError (exit 3, adapter fail-open).
    pub fn from_outcomes(
        blocks: Vec<Block>,
        errors: Vec<GateError>,
        passed: Vec<String>,
        elapsed_ms: u64,
    ) -> Self {
        let status = if !blocks.is_empty() {
            Status::Block
        } else if !errors.is_empty() {
            Status::InternalError
        } else {
            Status::Pass
        };
        Self {
            schema_version: SCHEMA_VERSION,
            hector_version: env!("CARGO_PKG_VERSION").to_string(),
            status,
            blocks,
            errors,
            passed,
            elapsed_ms,
        }
    }
}
```

- [ ] **Step 4: Run tests** — deferred to Task 11 (crate still mid-migration; runner references old API until Task 6).

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/verdict.rs
git commit -m "feat(verdict)!: gates shape (blocks/errors), schema 4, block-wins precedence"
```

---

## Task 6: Runner — gate dispatch core

**Files:**
- Modify: `crates/hector-core/src/runner.rs` (core rewrite)

Before writing, read the current `runner.rs` in full to preserve the parts being kept: `config/scope.rs` matching usage, `extends` resolution call (`config::parse_file_with_extends`), path canonicalization (`canonicalize_through_parent`, `relativize`, `resolve_input_path`, `allow_external_paths`), and the `CheckInput` enum. Those stay; the *engine dispatch* is what changes.

- [ ] **Step 1: Write the failing tests**

Add to the `runner.rs` test module:

```rust
#[cfg(test)]
mod gate_dispatch_tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn matching_gate_that_exits_2_blocks() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
        );
        let target = write(dir.path(), "a.rs", "// nothing\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File { path: target, content: "// TODO fix\n".into() })
            .unwrap();
        assert_eq!(v.status, crate::verdict::Status::Block);
        assert_eq!(v.blocks.len(), 1);
        assert_eq!(v.blocks[0].gate, "no-todo");
    }

    #[test]
    fn non_matching_file_passes_with_no_gates_run() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "gates:\n  ts-only:\n    files: \"**/*.ts\"\n    run: \"exit 2\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File { path: target, content: "x\n".into() })
            .unwrap();
        assert_eq!(v.status, crate::verdict::Status::Pass);
        assert!(v.passed.is_empty());
    }

    #[test]
    fn broken_gate_is_internal_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".hector.yml",
            "gates:\n  oops:\n    files: \"**/*.rs\"\n    run: \"definitely-not-real-xyz\"\n",
        );
        let target = write(dir.path(), "a.rs", "x\n");
        let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
        let v = engine
            .check(CheckInput::File { path: target, content: "x\n".into() })
            .unwrap();
        assert_eq!(v.status, crate::verdict::Status::InternalError);
        assert_eq!(v.errors[0].reason, "not_found");
    }
}
```

- [ ] **Step 2: Run** `cargo build -p hector-core 2>&1 | head` — expect errors from the old dispatch path.

- [ ] **Step 3: Rewrite the dispatch core**

Replace the engine-dispatch portions of `runner.rs`:

1. Delete `engine_kind_to_verdict_engine` and `run_engine` (the `EngineKind` match) and any `use crate::engine::{RuleContext, RuleEngine, ...}` / `use crate::config::EngineKind`.

2. Change the `HectorEngine` struct to hold the parsed config + root + options (keep whatever fields the kept path-canonicalization code needs):

```rust
pub struct HectorEngine {
    config: crate::config::Config,
    /// Directory containing the config file; gate cwd + HECTOR_ROOT.
    root: std::path::PathBuf,
    options: CheckOptions,
    /// Validated `--gate` filter; empty = all gates.
    gate_filter: std::collections::HashSet<String>,
    timeout: std::time::Duration,
}
```

3. In `load` (and the builder's `load`), after `let config = crate::config::parse_file_with_extends(config_path)?;`:
   - **Remove** the `crate::trust::verify(...)` call entirely (trust returns in Plan 2).
   - Compute `root` = parent dir of the canonicalized config path.
   - Compute `timeout` from `HECTOR_TIMEOUT` env (parse u64 secs) else `config.execution.timeout_secs`, as `Duration::from_secs`.

```rust
fn resolve_timeout(config: &crate::config::Config) -> std::time::Duration {
    let secs = std::env::var("HECTOR_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(config.execution.timeout_secs);
    std::time::Duration::from_secs(secs.max(1))
}
```

4. Rename `rule_matches_path` → `gate_matches_path` and have it read `self.config.gates[id].files` (the scope-matching call in `config/scope.rs` is unchanged — it takes a list of glob strings). Rename `config_rule_ids` → `gate_ids` returning `self.config.gates.keys()`.

5. Replace `set_rule_filter` → `set_gate_filter`.

6. Rewrite `check`:

```rust
pub fn check(&self, input: CheckInput) -> Result<Verdict> {
    use crate::engine::{run_gate, GateEnv, GateOutcome};
    use crate::verdict::{Block, GateError, Verdict};

    let (path, content) = match &input {
        CheckInput::File { path, content } => (path.clone(), content.clone()),
        CheckInput::Diff { file, .. } => {
            // Gates don't consume diffs; read the post-image from disk.
            let content = std::fs::read_to_string(file).unwrap_or_default();
            (file.clone(), content)
        }
    };

    let abs = self.resolve_input_path(&path)?;
    let event = self.options.event.clone();

    let start = std::time::Instant::now();
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();

    for (id, gate) in &self.config.gates {
        if !self.gate_filter.is_empty() && !self.gate_filter.contains(id) {
            continue;
        }
        if !self.gate_matches_path(id, &path) {
            continue;
        }
        // hector-disable: <gate-id> directive in the proposed content.
        if crate::disable::is_disabled(&content, id) {
            continue;
        }
        let env = GateEnv { file: &abs, root: &self.root, event: &event };
        match run_gate(&gate.run, &env, Some(content.as_bytes()), self.timeout) {
            GateOutcome::Pass => passed.push(id.clone()),
            GateOutcome::Block { message } => blocks.push(Block {
                gate: id.clone(),
                file: path.display().to_string(),
                message,
            }),
            GateOutcome::Internal(reason) => errors.push(GateError {
                gate: id.clone(),
                file: path.display().to_string(),
                reason: reason.as_str(),
            }),
        }
    }

    Ok(Verdict::from_outcomes(
        blocks,
        errors,
        passed,
        start.elapsed().as_millis() as u64,
    ))
}
```

> Parallelism note: the old runner used rayon over rules. For Plan 1, run gates **sequentially** (the loop above). Re-introducing the rayon pool over gates is a follow-up; per-file there are rarely many gates, and sequential keeps this rewrite reviewable. Track as a Future bullet.

7. Add `event: String` to `CheckOptions` (default `"manual"`), and thread it through the builder.

- [ ] **Step 4: Run the targeted tests**

Run: `cargo test -p hector-core gate_dispatch_tests` (after Tasks 7–8 land the rest of the references, the whole crate compiles; if compile still blocked by telemetry/disable, finish Task 8 then run).
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/runner.rs
git commit -m "feat(runner)!: gate dispatch (match files -> run_gate -> verdict), drop trust gate"
```

---

## Task 7: Runner — explain + gate filter

**Files:**
- Modify: `crates/hector-core/src/runner.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn explain_reports_per_gate_outcome() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".hector.yml"),
        "gates:\n  blocker:\n    files: \"**/*.rs\"\n    run: \"exit 2\"\n  passer:\n    files: \"**/*.rs\"\n    run: \"exit 0\"\n",
    )
    .unwrap();
    let target = dir.path().join("a.rs");
    std::fs::write(&target, "x\n").unwrap();
    let engine = HectorEngine::load(&dir.path().join(".hector.yml")).unwrap();
    let report = engine
        .check_with_explain(CheckInput::File { path: target, content: "x\n".into() })
        .unwrap();
    let outcomes: std::collections::HashMap<_, _> = report
        .explain
        .iter()
        .map(|r| (r.gate_id.clone(), matches!(r.outcome, ExplainOutcome::Fire)))
        .collect();
    assert_eq!(outcomes["blocker"], true);
    assert_eq!(outcomes["passer"], false);
}
```

- [ ] **Step 2: Run** `cargo test -p hector-core explain_reports_per_gate_outcome` → FAIL (field renamed `rule_id`→`gate_id`, struct shapes).

- [ ] **Step 3: Implement**

Rename in `runner.rs`:
- `RuleExplain { rule_id, engine, outcome }` → `GateExplain { gate_id: String, outcome: ExplainOutcome }` (drop the `engine` field — there is one engine now).
- `ExplainOutcome` keeps `Fire | Pass | Skipped { reason }`.
- `CheckReport { verdict, explain: Vec<GateExplain> }`.
- Implement `check_with_explain` by running the same loop as `check` but recording a `GateExplain` per gate (Fire on Block, Pass on Pass, and `Skipped { reason }` for filtered/non-matching/disabled gates), then building the verdict from the collected blocks/errors/passed.

> Refactor to avoid duplication: extract the per-gate loop into a private helper that returns `(Vec<Block>, Vec<GateError>, Vec<String>, Vec<GateExplain>)`, and have both `check` and `check_with_explain` call it (the former discards `explain`). Keep cognitive complexity per function ≤ 15 (project rule) — the helper should delegate per-gate classification to a small `classify_gate` fn.

- [ ] **Step 4: Run** `cargo test -p hector-core explain_reports_per_gate_outcome` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/runner.rs
git commit -m "feat(runner): per-gate explain + gate filter"
```

---

## Task 8: Telemetry + disable retarget

**Files:**
- Modify: `crates/hector-core/src/telemetry.rs`
- Modify: `crates/hector-core/src/disable.rs`
- Modify: `crates/hector-core/src/runner.rs` (emit telemetry from `check`)

- [ ] **Step 1: Write the failing tests**

In `telemetry.rs` tests:

```rust
#[test]
fn round_trips_a_gate_check_entry() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.jsonl");
    let entry = LogEntry::Check {
        ts: "2026-06-15T00:00:00Z".into(),
        file: "a.rs".into(),
        status: crate::verdict::Status::Block,
        elapsed_ms: 3,
        gates: vec![PerGateRecord {
            gate: "no-todo".into(),
            status: crate::verdict::Status::Block,
            elapsed_ms: 3,
            reason: None,
        }],
    };
    append(&log, &entry).unwrap();
    let back = read_all(&log).unwrap();
    assert_eq!(back, vec![entry]);
}
```

In `disable.rs` tests (confirm gate-id semantics still work — the function is id-agnostic, this is a guard):

```rust
#[test]
fn disables_named_gate() {
    assert!(is_disabled("code // hector-disable: no-todo\n", "no-todo"));
    assert!(!is_disabled("code // hector-disable: other\n", "no-todo"));
}
```

- [ ] **Step 2: Run** `cargo build -p hector-core 2>&1 | head` → errors (PerRuleRecord/Engine references; LogEntry.rules field).

- [ ] **Step 3: Implement**

`telemetry.rs`:
- Bump `SCHEMA_VERSION` to `3`.
- Rename `PerRuleRecord` → `PerGateRecord { gate: String, status: Status, elapsed_ms: u64, reason: Option<String> }`. Remove the `engine` field and the `use crate::verdict::Engine` import.
- In `LogEntry::Check`, rename `rules` → `gates: Vec<PerGateRecord>`.
- **Delete** the legacy reader path: remove `LogEntryLegacy`, `LogEntryRead`, `parse_status`, `emit_legacy_warning`, `LEGACY_WARNING_EMITTED`, and the `untagged` wrapper. `read_all` parses `LogEntry` directly (the 0.3 freeze ends the deprecation window — this redesign *is* past it). Malformed lines are still warned + dropped.

`disable.rs`:
- The matcher is id-agnostic; ensure the public fn is `pub fn is_disabled(content: &str, gate_id: &str) -> bool`. Update doc comments to say "gate id" instead of "rule id". If the current public name differs (e.g. `is_rule_disabled`), rename to `is_disabled` and update the single caller in `runner::check`.

`runner.rs`:
- After building the verdict in `check`, append a `LogEntry::Check` to `<root>/.hector/log.jsonl` with one `PerGateRecord` per gate that ran. Timestamp: use `chrono::Utc::now().to_rfc3339()` if `chrono` is already a dep; otherwise format from `std::time::SystemTime`. (Check existing deps — the old runner already wrote telemetry, so the timestamp source already exists; reuse it.)

- [ ] **Step 4: Run** `cargo test -p hector-core telemetry disable` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-core/src/telemetry.rs crates/hector-core/src/disable.rs crates/hector-core/src/runner.rs
git commit -m "feat(telemetry)!: per-gate records (schema 3), drop legacy reader; disable by gate id"
```

---

## Task 9: Delete baseline (core) and wire lib

**Files:**
- Delete: `crates/hector-core/src/baseline.rs`
- Modify: `crates/hector-core/src/lib.rs`
- Modify: `crates/hector-core/src/runner.rs` (remove baseline-filter step if present)

- [ ] **Step 1: Remove baseline**

```bash
git rm crates/hector-core/src/baseline.rs
```

In `lib.rs`, delete the `pub mod baseline;` line.

In `runner.rs`, remove any baseline import and the baseline-filter step in the check pipeline (the redesign drops record-and-filter; grandfathering moves into gate scripts).

- [ ] **Step 2: Verify the core crate compiles and core tests pass**

Run: `cargo test -p hector-core`
Expected: PASS — all core unit tests (Tasks 1–8) green. If anything in `runner.rs` still references removed symbols, fix now; this is the core crate's green-gate.

- [ ] **Step 3: Commit**

```bash
git add -A crates/hector-core
git commit -m "refactor(core)!: drop baseline module"
```

---

## Task 10: CLI — rewire `check`, drop migrate/baseline

**Files:**
- Modify: `crates/hector-cli/src/cli.rs`
- Modify: `crates/hector-cli/src/commands/check.rs` (rewrite)
- Delete: `crates/hector-cli/src/commands/{migrate,baseline}.rs`
- Modify: `crates/hector-cli/src/commands/mod.rs`
- Modify: `crates/hector-cli/src/commands/{validate,explain,show_resolved_config,guide}.rs` (compile fixes)

- [ ] **Step 1: Update `cli.rs`**

In the `Check` variant: rename `--rule`/`rules` to `--gate`/`gates`:

```rust
/// Evaluate only this gate id. Repeatable; multiple flags OR'd.
#[arg(long = "gate", action = clap::ArgAction::Append)]
gates: Vec<String>,
```

Add an `--event` flag to `Check`:

```rust
/// What triggered this check, surfaced to gates as $HECTOR_EVENT.
#[arg(long, default_value = "manual")]
event: String,
```

Remove the `Migrate { .. }` and `Baseline { .. }` variants entirely. Update the `match` in `main`/dispatch accordingly.

- [ ] **Step 2: Rewrite `commands/check.rs`**

Replace the file with (keeping the `resolve_content_value` stdin helper):

```rust
use crate::cli::OutputFormat;
use anyhow::{Context, Result};
use hector_core::runner::{CheckInput, CheckOptions, ExplainOutcome, GateExplain, HectorEngine};
use hector_core::verdict::{Status, Verdict};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub fn run(
    file: Option<PathBuf>,
    diff: Option<PathBuf>,
    content: Option<String>,
    format: OutputFormat,
    config: &Path,
    gates: Vec<String>,
    event: String,
    explain: bool,
    allow_external_paths: bool,
) -> Result<i32> {
    let options = CheckOptions {
        gates: HashSet::new(),
        event,
        explain,
        allow_external_paths,
    };
    let mut engine = match HectorEngine::builder().with_options(options).load(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            return Ok(1);
        }
    };
    if let Some(code) = validate_gate_filter(&engine, &gates) {
        return Ok(code);
    }
    engine.set_gate_filter(gates.into_iter().collect());

    match (file, diff) {
        (Some(f), None) => run_file(&engine, f, content, format, explain),
        (None, Some(d)) => run_diff(&engine, &d, format, explain),
        _ => {
            eprintln!("ERROR: provide exactly one of --file or --diff");
            Ok(1)
        }
    }
}

fn run_file(
    engine: &HectorEngine,
    file: PathBuf,
    content: Option<String>,
    format: OutputFormat,
    explain: bool,
) -> Result<i32> {
    let content = match content {
        Some(c) => resolve_content_value(c)?,
        None => std::fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?,
    };
    let report = engine.check_with_explain(CheckInput::File { path: file, content })?;
    if explain {
        print_explain(&report.explain);
    }
    emit(&report.verdict, format)?;
    Ok(exit_code(&report.verdict))
}

/// Check every non-deleted changed file in a unified diff. Gates read each
/// file's current on-disk content (gates don't consume diffs).
fn run_diff(engine: &HectorEngine, diff: &Path, format: OutputFormat, explain: bool) -> Result<i32> {
    let unified = std::fs::read_to_string(diff)?;
    let changed = hector_core::diff::parser::parse_unified(&unified)?;
    let targets: Vec<_> = changed
        .iter()
        .filter(|f| f.op != hector_core::diff::ChangeOp::Deleted)
        .collect();
    if changed.is_empty() {
        eprintln!("ERROR: no changed files in diff");
        return Ok(1);
    }
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut passed = Vec::new();
    let mut explains: Vec<GateExplain> = Vec::new();
    let mut elapsed = 0u64;
    for f in targets {
        let content = std::fs::read_to_string(&f.path).unwrap_or_default();
        let r = engine.check_with_explain(CheckInput::File {
            path: f.path.clone(),
            content,
        })?;
        elapsed = elapsed.saturating_add(r.verdict.elapsed_ms);
        blocks.extend(r.verdict.blocks);
        errors.extend(r.verdict.errors);
        passed.extend(r.verdict.passed);
        explains.extend(r.explain);
    }
    let verdict = Verdict::from_outcomes(blocks, errors, passed, elapsed);
    if explain {
        print_explain(&explains);
    }
    emit(&verdict, format)?;
    Ok(exit_code(&verdict))
}

fn validate_gate_filter(engine: &HectorEngine, gates: &[String]) -> Option<i32> {
    if gates.is_empty() {
        return None;
    }
    let known: HashSet<&str> = engine.gate_ids().collect();
    let unknown: Vec<&str> = gates
        .iter()
        .map(|s| s.as_str())
        .filter(|id| !known.contains(id))
        .collect();
    if unknown.is_empty() {
        None
    } else {
        eprintln!("ERROR: unknown gate id(s): {}", unknown.join(", "));
        Some(1)
    }
}

fn print_explain(rows: &[GateExplain]) {
    for row in rows {
        let outcome = match &row.outcome {
            ExplainOutcome::Fire => "fire".to_string(),
            ExplainOutcome::Pass => "pass".to_string(),
            ExplainOutcome::Skipped { reason } => format!("skipped {reason}"),
        };
        eprintln!("{} {}", row.gate_id, outcome);
    }
}

fn exit_code(v: &Verdict) -> i32 {
    match v.status {
        Status::Block => 2,
        Status::InternalError => 3,
        _ => 0,
    }
}

fn emit(v: &Verdict, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(v)?),
        OutputFormat::Human => {
            for b in &v.blocks {
                eprintln!("block: [{}] {}", b.gate, b.file);
                eprintln!("  {}", b.message);
            }
            for e in &v.errors {
                eprintln!("error: [{}] {} ({})", e.gate, e.file, e.reason);
            }
            println!(
                "{}",
                match v.status {
                    Status::Pass => "pass",
                    Status::Block => "block",
                    Status::InternalError => "internal_error",
                    _ => "unknown",
                }
            );
        }
    }
    Ok(())
}

fn resolve_content_value(value: String) -> Result<String> {
    if value == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read --content from stdin (expected UTF-8)")?;
        Ok(buf)
    } else {
        Ok(value)
    }
}
```

Update the call site in `cli.rs`/dispatch to pass `event` into `commands::check::run`.

- [ ] **Step 3: Delete migrate/baseline commands**

```bash
git rm crates/hector-cli/src/commands/migrate.rs crates/hector-cli/src/commands/baseline.rs
```

In `commands/mod.rs`, remove `pub mod migrate;` and `pub mod baseline;`.

- [ ] **Step 4: Fix the remaining commands to compile**

- `commands/validate.rs`: it calls `parse_file`/load; ensure it no longer references `is_legacy`/`EngineKind`. A valid config now just needs to parse — print `ok` with the gate count.
- `commands/explain.rs` / `show_resolved_config.rs` / `guide.rs`: replace any `EngineKind`/`Rule`/`rule_id` references with the gate equivalents (`config.gates`, `gate_id`). `show_resolved_config` prints the resolved `gates` map after `extends`.
- `commands/trust.rs`: leave as-is for Plan 1 (it still compiles against `trust.rs`, which is untouched and simply unused by `load`). It will be rewritten in Plan 2.

Run: `cargo build` (whole workspace). Fix any remaining references until it builds.

- [ ] **Step 5: Commit**

```bash
git add -A crates/hector-cli
git commit -m "feat(cli)!: gates check (--gate/--event), drop migrate+baseline commands"
```

---

## Task 11: End-to-end CLI tests + fixtures

**Files:**
- Create: `crates/hector-cli/tests/cli_e2e_gates.rs`
- Create: `tests/fixtures/gates/.hector.yml` and sample files
- Delete: obsolete e2e test files that exercised the old engine model (e.g. `cli_e2e_script_rules.rs`, any `*_ast_*`, `*_semantic_*`, `*_baseline_*`, `*_migrate_*` test files)

- [ ] **Step 1: Remove obsolete e2e tests**

```bash
ls crates/hector-cli/tests
git rm crates/hector-cli/tests/cli_e2e_script_rules.rs   # and any ast/semantic/baseline/migrate e2e files
```

(Keep generic CLI tests that don't assume the engine model; port them if small.)

- [ ] **Step 2: Write the e2e tests**

Create `crates/hector-cli/tests/cli_e2e_gates.rs`:

```rust
use assert_cmd::Command;
use std::io::Write;

fn cfg(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join(".hector.yml"), body).unwrap();
}

#[test]
fn exit_2_gate_blocks_and_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// TODO\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args(["check", "--file", file.to_str().unwrap(), "--config", ".hector.yml"])
        .assert()
        .code(2);
}

#[test]
fn clean_file_passes_and_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// clean\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args(["check", "--file", file.to_str().unwrap(), "--config", ".hector.yml"])
        .assert()
        .code(0);
}

#[test]
fn stdin_content_gates_prewrite() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  no-todo:\n    files: \"**/*.rs\"\n    run: \"grep -q TODO && exit 2 || exit 0\"\n",
    );
    // On-disk file is clean; the proposed content (stdin) is dirty -> block.
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "// clean\n").unwrap();

    let mut cmd = Command::cargo_bin("hector").unwrap();
    cmd.current_dir(dir.path()).args([
        "check",
        "--file",
        file.to_str().unwrap(),
        "--content",
        "-",
        "--config",
        ".hector.yml",
    ]);
    cmd.write_stdin("// TODO later\n").assert().code(2);
}

#[test]
fn broken_gate_exits_3() {
    let dir = tempfile::tempdir().unwrap();
    cfg(
        dir.path(),
        "gates:\n  oops:\n    files: \"**/*.rs\"\n    run: \"no-such-binary-xyz\"\n",
    );
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args(["check", "--file", file.to_str().unwrap(), "--config", ".hector.yml"])
        .assert()
        .code(3);
}

#[test]
fn unknown_gate_filter_errors() {
    let dir = tempfile::tempdir().unwrap();
    cfg(dir.path(), "gates:\n  g:\n    files: \"*\"\n    run: \"true\"\n");
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "check", "--file", file.to_str().unwrap(),
            "--gate", "nope", "--config", ".hector.yml",
        ])
        .assert()
        .code(1);
}

#[test]
fn legacy_config_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    cfg(dir.path(), "schema_version: 2\nrules: {}\n");
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "x\n").unwrap();

    Command::cargo_bin("hector")
        .unwrap()
        .current_dir(dir.path())
        .args(["check", "--file", file.to_str().unwrap(), "--config", ".hector.yml"])
        .assert()
        .code(1);
}
```

Add `assert_cmd` and `tempfile` to `crates/hector-cli/Cargo.toml` `[dev-dependencies]` if not present.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 4: Commit**

```bash
git add -A crates/hector-cli/tests tests/fixtures
git commit -m "test(cli): e2e gates check (block/pass/stdin/broken/filter/legacy)"
```

---

## Task 12: Lint, coverage, cleanup

**Files:** (no new code; fixes as needed)

- [ ] **Step 1: Clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Fix all warnings. Pay attention to the cognitive-complexity cap (≤15) on the new `runner::check`/`check_with_explain` and `gate::run_gate` — decompose with helpers (`classify`, `classify_gate`) rather than `#[allow]`.

- [ ] **Step 2: Coverage gate**

Run: `bash scripts/ci-coverage.sh`
Expected: every touched file ≥80% region coverage. Where short, add targeted unit tests (e.g. `InternalReason::as_str` variants, `resolve_timeout` env path, `from_outcomes` precedence already covered). Note from memory: the coverage script may be CI-only locally if `llvm-tools-preview` is unavailable — if it can't run locally, state that and rely on CI.

- [ ] **Step 3: Stale-reference sweep**

Run:
```bash
grep -rn "EngineKind\|RuleEngine\|Severity\|Capabilities\|OutputMode\|schema_version\|baseline\|rule_id\|ast_grep" crates/ docs/ adapters/ 2>/dev/null
```
Resolve any real code references (docs/adapters are updated in later plans, but note CHANGELOG). Remove any now-dead `use` statements flagged by clippy.

- [ ] **Step 4: Update CHANGELOG**

Add an `## [Unreleased]` entry summarizing the gates redesign (breaking): config is now `gates: { files, run }`; exit-2 contract; engines/severity/baseline/migrate removed; trust temporarily unenforced pending the direnv store (Plan 2).

- [ ] **Step 5: Build-artifact cleanup**

Per repo rules, remove any throwaway artifacts produced while verifying (`pr.diff`, scratch binaries). The persistent `target/` stays.

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "chore: clippy/fmt/coverage pass + changelog for gates redesign"
```

---

## Self-Review (completed during authoring)

**Spec coverage (§ → task):**
- §2 config language → Tasks 1, 2.
- §3 verdict contract (exit codes, output, outer codes) → Tasks 3 (exit→outcome), 5 (status), 10 (exit_code mapping + emit).
- §4 ABI (env + stdin + cwd) → Task 3 (`GateEnv`/`run_gate`), Task 6 (materialized in `check`), Task 10 (`--event`). *Adapter side is Plan 4.*
- §5 per-file execution → Task 6 loop.
- §6 trust → **deferred to Plan 2** (Plan 1 removes the gate; explicitly flagged).
- §7 verify/doctor → **deferred to Plan 3**.
- §9 verdict JSON → Task 5.
- §10 telemetry + disable → Task 8.
- §11 deletions → Tasks 4 (engines/deps), 8 (legacy telemetry reader), 9 (baseline), 10 (migrate/baseline commands), 2 (schema apparatus).

**Placeholder scan:** no TBD/TODO; every code step carries complete code. The two "read the file first" notes (Task 6) are scoped to *preserving existing kept logic*, not deferring new code.

**Type consistency:** `Gate{files,run}`, `Config{extends,execution,gates}`, `ExecutionConfig{timeout_secs,max_workers}`, `GateEnv{file,root,event}`, `GateOutcome{Pass,Block{message},Internal(InternalReason)}`, `InternalReason::as_str`, `Verdict::from_outcomes`, `Block{gate,file,message}`, `GateError{gate,file,reason}`, `Status{Pass,Block,InternalError}`, `GateExplain{gate_id,outcome}`, `CheckOptions{gates,event,explain,allow_external_paths}`, `gate_ids`, `set_gate_filter`, `gate_matches_path`, `is_disabled(content,gate_id)`, `PerGateRecord{gate,status,elapsed_ms,reason}`, `LogEntry::Check{ts,file,status,elapsed_ms,gates}` — names used consistently across tasks.

**Known follow-ups (not Plan 1):** rayon parallel gate dispatch; trust store (Plan 2); verify/doctor (Plan 3); adapter `--event` + ABI (Plan 4).
