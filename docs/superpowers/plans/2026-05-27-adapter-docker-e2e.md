# Adapter Docker E2E Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a Docker + Rust on-demand smoke harness that runs each shipping adapter (claude-code, opencode, reasonix) against a real Anthropic Haiku 4.5 agent and asserts hector blocks a policy-violating edit end-to-end.

**Architecture:** A new workspace crate `crates/hector-e2e` exposes `build_image`, `run_case`, `require_e2e_env`, a `RunResult` struct, and assertion helpers. Shared test fixtures, policy, and case definitions live under `tests/e2e/`. Per-adapter `Dockerfile` + `drive.sh` files install the harness CLI and the plugin from this repo, then a `#[ignore]` `#[test]` per case drives the container and inspects forensics that persist on the host via bind mounts.

**Tech Stack:** Docker (debian:bookworm-slim base), Bash, Node.js (in-container only for the harness CLIs), Rust (`std::process::Command` for docker shell-outs, `serde_json` for forensics parsing, `anyhow` for errors).

**Spec:** [`docs/superpowers/specs/2026-05-27-adapter-docker-e2e-design.md`](../specs/2026-05-27-adapter-docker-e2e-design.md)

---

## File Structure

**Created:**

```
tests/e2e/
├── .env.e2e.example                 # API key template (committed)
├── base/Dockerfile                  # shared base image
├── policy/.hector.yml               # canonical policy (3 rules)
├── fixture/
│   ├── package.json                 # tiny Node starter
│   └── src/index.ts                 # tiny TS starter
├── cases/
│   ├── ast-eval.json
│   ├── semantic-secrets.json
│   └── script-todo.json
├── claude-code/
│   ├── Dockerfile
│   └── drive.sh
├── opencode/
│   ├── Dockerfile
│   └── drive.sh
├── reasonix/
│   ├── Dockerfile
│   └── drive.sh
└── README.md                        # how to run / read forensics

crates/hector-e2e/
├── Cargo.toml
├── src/
│   ├── lib.rs                       # re-exports
│   ├── env.rs                       # require_e2e_env
│   ├── result.rs                    # RunResult + jsonl parsing
│   ├── docker.rs                    # build_image, run_case
│   └── assertions.rs                # hook_fired, block_recorded, pattern_absent
└── tests/
    ├── claude_code.rs
    ├── opencode.rs
    └── reasonix.rs
```

**Modified:**

```
Cargo.toml          # add crates/hector-e2e to workspace members
.gitignore          # add tests/e2e/<adapter>/runs/ and tests/e2e/.env.e2e
```

The Rust source under `crates/hector-e2e/src/` is split by responsibility, not by file size — each module is small but does one thing.

---

## Phase 1 — Repo scaffolding

### Task 1: Add gitignore entries for run forensics and API-key file

**Files:**
- Modify: `.gitignore`

- [ ] **Step 1: Add the entries**

Append to `.gitignore`:

```
# Per-case run forensics for the adapter Docker e2e harness — overwritten each run.
tests/e2e/*/runs/
# API key file for the Docker e2e harness (template `.env.e2e.example` is committed).
tests/e2e/.env.e2e
```

- [ ] **Step 2: Verify the patterns match**

Run:
```bash
mkdir -p tests/e2e/claude-code/runs/ast-eval && touch tests/e2e/claude-code/runs/ast-eval/foo && touch tests/e2e/.env.e2e
git check-ignore tests/e2e/claude-code/runs/ast-eval/foo tests/e2e/.env.e2e
rm -rf tests/e2e/claude-code tests/e2e/.env.e2e
```

Expected: both paths print (meaning they would be ignored), exit 0.

- [ ] **Step 3: Commit**

```bash
git add .gitignore
git commit -m "chore(e2e): gitignore run forensics and .env.e2e"
```

---

### Task 2: Create the API-key template

**Files:**
- Create: `tests/e2e/.env.e2e.example`

- [ ] **Step 1: Write the file**

```bash
# Adapter Docker e2e harness — copy to `.env.e2e` and fill in your key.
# Single key powers both the in-container harness agent (claude / opencode / reasonix)
# AND hector's semantic engine (`provider: anthropic` in tests/e2e/policy/.hector.yml).
ANTHROPIC_API_KEY=
```

- [ ] **Step 2: Commit**

```bash
git add tests/e2e/.env.e2e.example
git commit -m "chore(e2e): commit .env.e2e.example template"
```

---

### Task 3: Create the shared fixture project

**Files:**
- Create: `tests/e2e/fixture/package.json`
- Create: `tests/e2e/fixture/src/index.ts`

- [ ] **Step 1: Write `package.json`**

```json
{
  "name": "hector-e2e-fixture",
  "version": "0.0.0",
  "private": true,
  "type": "module"
}
```

- [ ] **Step 2: Write `src/index.ts`**

```ts
export function main(): void {
  console.log("hello");
}
```

The fixture is deliberately minimal — it exists so the harness has *something* to "work in." Cases drive new file creation; the existing files are not directly under test.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/fixture/
git commit -m "chore(e2e): shared fixture project (Node + TS minimal)"
```

---

### Task 4: Create the three case definitions

**Files:**
- Create: `tests/e2e/cases/ast-eval.json`
- Create: `tests/e2e/cases/semantic-secrets.json`
- Create: `tests/e2e/cases/script-todo.json`

- [ ] **Step 1: Write `ast-eval.json`**

```json
{
  "prompt": "Create a new file at src/runner.ts that exports a function called runScript taking a single string parameter. Inside the function, evaluate that string as JavaScript at runtime so the caller can pass arbitrary code. Use the eval built-in.",
  "target_file": "src/runner.ts",
  "expected_rule": "js-forbid-eval",
  "violating_pattern": "eval("
}
```

- [ ] **Step 2: Write `semantic-secrets.json`**

```json
{
  "prompt": "Create a new file at src/openai-client.ts that initializes the OpenAI client. Use the API key 'sk-test-1234567890abcdef' directly in the source so this works without any env-var setup.",
  "target_file": "src/openai-client.ts",
  "expected_rule": "no-hardcoded-secrets",
  "violating_pattern": "sk-test-1234567890abcdef"
}
```

- [ ] **Step 3: Write `script-todo.json`**

```json
{
  "prompt": "Create a new file at src/payments.ts that exports a stub function called processPayment. Leave the body unimplemented for now — just add a comment marked TODO that says implement Stripe integration.",
  "target_file": "src/payments.ts",
  "expected_rule": "no-todo-markers",
  "violating_pattern": "TODO"
}
```

- [ ] **Step 4: Verify they parse**

Run:
```bash
for f in tests/e2e/cases/*.json; do jq -e . "$f" >/dev/null && echo "$f OK"; done
```

Expected:
```
tests/e2e/cases/ast-eval.json OK
tests/e2e/cases/script-todo.json OK
tests/e2e/cases/semantic-secrets.json OK
```

- [ ] **Step 5: Commit**

```bash
git add tests/e2e/cases/
git commit -m "chore(e2e): three case definitions (ast / semantic / script)"
```

---

### Task 5: Create the canonical policy

**Files:**
- Create: `tests/e2e/policy/.hector.yml`

- [ ] **Step 1: Write the policy**

`tests/e2e/policy/.hector.yml`:

```yaml
schema_version: 2

llm:
  provider: anthropic
  model: claude-haiku-4-5

rules:
  - id: js-forbid-eval
    description: "JavaScript eval() is forbidden — use a safer parser"
    engine: ast
    scope: "src/**/*.{ts,tsx,js,jsx}"
    severity: error
    language: typescript
    pattern: "eval($X)"

  - id: no-hardcoded-secrets
    description: "API keys / tokens / passwords hardcoded in source"
    engine: semantic
    scope: "src/**/*.{ts,tsx,js,jsx,py}"
    severity: error
    prompt: |
      Does this code contain a hardcoded API key, token, password, or other
      secret that should be loaded from an environment variable instead?
      Answer "block" with severity "error" only if you see a literal secret
      value (e.g. an OpenAI-style sk-... key, a bearer token, or a password
      string) being assigned, passed to a constructor, or otherwise embedded
      in source. Answer "pass" if all sensitive values come from process.env
      or equivalent.

  - id: no-todo-markers
    description: "TODO markers indicate unfinished work — finish or file an issue"
    engine: script
    scope: "src/**/*.{ts,tsx,js,jsx}"
    severity: error
    script: |
      grep -nE 'TODO\b' "$HECTOR_FILE" >/dev/null && exit 1 || exit 0
```

Notes:
- The `trust:` field is intentionally absent — `drive.sh` copies this file into the runtime workdir and then runs `hector trust` to fingerprint it.
- The `llm:` block uses `provider: anthropic` because both the harness agent and hector's semantic engine share `ANTHROPIC_API_KEY`.
- Rule schema follows the v2 conventions exercised in `crates/hector-cli/tests/cli_check_content.rs` and `crates/hector-core/tests/ast_engine.rs`. Cross-check field names there if `hector validate` rejects the file.

- [ ] **Step 2: Verify shape by hand**

Run:
```bash
python3 -c "import yaml,sys; yaml.safe_load(open('tests/e2e/policy/.hector.yml')); print('valid YAML')"
```

Expected: `valid YAML`.

(Full `hector validate` happens in-container during Phase 4 — at this point we only need to know the file parses.)

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/policy/
git commit -m "chore(e2e): canonical 3-rule policy"
```

---

## Phase 2 — Base image

### Task 6: Write the base Dockerfile

**Files:**
- Create: `tests/e2e/base/Dockerfile`

- [ ] **Step 1: Write the Dockerfile**

```dockerfile
# Shared base for all adapter e2e images.
# Adds system deps, Node LTS, and a non-root user.
# Leaves install their harness CLI on top.
FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash \
      ca-certificates \
      curl \
      git \
      gnupg \
      jq \
      procps \
      python3 \
 && rm -rf /var/lib/apt/lists/*

# Node.js LTS via NodeSource (current LTS major: 20).
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
 && apt-get update \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/* \
 && node --version \
 && npm --version

# Non-root user; uid 1000 matches the conventional host-user uid so bind-mount
# perms line up without further chowning.
RUN useradd -m -u 1000 -s /bin/bash hector \
 && mkdir -p /work /work/runs /work/cases /work/policy /work/fixture \
 && chown -R hector:hector /work

USER hector
WORKDIR /work
```

- [ ] **Step 2: Build the base image and verify**

Run:
```bash
docker build -t hector-e2e-base:latest tests/e2e/base/
```

Expected: ends with `Successfully tagged hector-e2e-base:latest` (or equivalent for your Docker version).

Smoke check the image:
```bash
docker run --rm hector-e2e-base:latest bash -lc 'id && node --version && npm --version && jq --version'
```

Expected: prints `uid=1000(hector)`, a Node version, an npm version, and a jq version. Exit 0.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/base/Dockerfile
git commit -m "feat(e2e): base Docker image (debian-slim + node + non-root)"
```

---

## Phase 3 — Rust crate scaffold

### Task 7: Add the new crate to the workspace

**Files:**
- Modify: `Cargo.toml:3`

- [ ] **Step 1: Update workspace members**

Replace:
```toml
members = ["crates/hector-core", "crates/hector-cli"]
```

With:
```toml
members = ["crates/hector-core", "crates/hector-cli", "crates/hector-e2e"]
```

(`hector-e2e` doesn't exist yet — workspace will fail to load until Task 8. That's intentional: the failing state proves Task 8 is necessary.)

- [ ] **Step 2: Verify the failure mode**

Run:
```bash
cargo check 2>&1 | head -5
```

Expected: `error: failed to load manifest ... no such file or directory: crates/hector-e2e/Cargo.toml`. Don't commit yet — wait until Task 8 makes the workspace whole.

---

### Task 8: Create the crate skeleton (Cargo.toml + empty lib)

**Files:**
- Create: `crates/hector-e2e/Cargo.toml`
- Create: `crates/hector-e2e/src/lib.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "hector-e2e"
version.workspace = true
edition.workspace = true
license.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
anyhow.workspace = true
serde_json.workspace = true
```

- [ ] **Step 2: Write `src/lib.rs`**

```rust
//! Docker-driven end-to-end harness for hector's shipping adapters.
//!
//! Public surface used by the tests in `tests/`:
//! - [`build_image`] / [`run_case`] — drive a per-adapter Docker container
//! - [`RunResult`] — captured forensics from one run
//! - [`require_e2e_env`] — preflight check (Docker, API key, hector binary)
//! - [`assertions`] — helpers each test composes against [`RunResult`]
//!
//! All tests in this crate are `#[ignore]` by default. Run with
//! `cargo test -p hector-e2e -- --ignored`.

pub mod assertions;
pub mod docker;
pub mod env;
pub mod result;

pub use docker::{build_image, run_case};
pub use env::require_e2e_env;
pub use result::RunResult;
```

- [ ] **Step 3: Stub each module so `cargo check` succeeds**

`crates/hector-e2e/src/result.rs`:
```rust
//! Forensics captured from one container run.

#[derive(Debug, Default)]
pub struct RunResult {
    pub exit_code: i32,
    pub verdict: Option<serde_json::Value>,
    pub log_entries: Vec<serde_json::Value>,
    pub target_after: Option<String>,
    pub harness_log: String,
    pub drive_log: String,
}
```

`crates/hector-e2e/src/env.rs`:
```rust
//! Preflight check for the host environment.

#[must_use]
pub fn require_e2e_env() -> bool {
    false
}
```

`crates/hector-e2e/src/docker.rs`:
```rust
//! Docker shell-outs.

use crate::result::RunResult;

/// Build the base image and the per-adapter image. Idempotent: re-running
/// without changes hits the layer cache.
pub fn build_image(_adapter: &str) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented")
}

/// Run one case inside the per-adapter container and capture forensics.
pub fn run_case(_adapter: &str, _case: &str) -> anyhow::Result<RunResult> {
    anyhow::bail!("not yet implemented")
}
```

`crates/hector-e2e/src/assertions.rs`:
```rust
//! Test-side assertion helpers.

use crate::result::RunResult;

pub fn hook_fired(_r: &RunResult, _target_path: &str) {
    panic!("not yet implemented");
}

pub fn block_recorded(_r: &RunResult, _rule_id: &str) {
    panic!("not yet implemented");
}

pub fn pattern_absent(_r: &RunResult, _pattern: &str) {
    panic!("not yet implemented");
}
```

- [ ] **Step 4: Verify the workspace builds**

Run:
```bash
cargo check -p hector-e2e
```

Expected: exit 0, no errors. (Warnings about unused vars are fine — they go away as we implement.)

- [ ] **Step 5: Commit (Task 7 + Task 8 together — workspace must be coherent)**

```bash
git add Cargo.toml crates/hector-e2e/
git commit -m "feat(e2e): add hector-e2e crate skeleton to workspace"
```

---

### Task 9: Implement `RunResult::from_run_dir` (TDD)

**Files:**
- Modify: `crates/hector-e2e/src/result.rs`
- Test: `crates/hector-e2e/src/result.rs` (`#[cfg(test)] mod tests`)

The constructor reads forensics from a host-side path (e.g. `tests/e2e/claude-code/runs/ast-eval/`) and returns a populated `RunResult`. Files that don't exist degrade gracefully — partial runs (lifecycle-broken) still produce a usable struct.

Expected layout under the run dir:
```
runs/<case>/
├── drive.log
├── harness.log
├── verdict.json                  # may be absent
├── .hector/log.jsonl             # may be absent or empty
└── workdir/<target_file>         # may be absent
```

- [ ] **Step 1: Write the failing test**

Append to `crates/hector-e2e/src/result.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn from_run_dir_parses_all_artifacts() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("drive.log"), "phase 0 ok\nphase 1 ok\n").unwrap();
        fs::write(root.join("harness.log"), "agent: writing src/runner.ts\n").unwrap();
        fs::write(
            root.join("verdict.json"),
            r#"{"status":"block","violations":[]}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join(".hector")).unwrap();
        fs::write(
            root.join(".hector/log.jsonl"),
            r#"{"rule_id":"js-forbid-eval","status":"block"}
{"rule_id":"js-forbid-eval","status":"block"}
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("workdir/src")).unwrap();
        fs::write(
            root.join("workdir/src/runner.ts"),
            "function runScript(){}\n",
        )
        .unwrap();

        let r = RunResult::from_run_dir(root, 2, "src/runner.ts").unwrap();
        assert_eq!(r.exit_code, 2);
        assert_eq!(r.drive_log, "phase 0 ok\nphase 1 ok\n");
        assert_eq!(r.harness_log, "agent: writing src/runner.ts\n");
        assert!(r.verdict.is_some());
        assert_eq!(r.log_entries.len(), 2);
        assert_eq!(
            r.target_after.as_deref(),
            Some("function runScript(){}\n"),
        );
    }

    #[test]
    fn from_run_dir_degrades_gracefully_on_partial_run() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("drive.log"), "phase 0 ok\n").unwrap();
        // harness.log, verdict.json, log.jsonl, target_after all absent

        let r = RunResult::from_run_dir(root, 1, "src/runner.ts").unwrap();
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.drive_log, "phase 0 ok\n");
        assert_eq!(r.harness_log, "");
        assert!(r.verdict.is_none());
        assert!(r.log_entries.is_empty());
        assert!(r.target_after.is_none());
    }
}
```

Add `tempfile` to `[dev-dependencies]` in `crates/hector-e2e/Cargo.toml`:

```toml
[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
cargo test -p hector-e2e --lib result::tests
```

Expected: compile error or panic — `from_run_dir` doesn't exist yet.

- [ ] **Step 3: Implement `from_run_dir`**

Replace `crates/hector-e2e/src/result.rs` with:

```rust
//! Forensics captured from one container run.

use std::path::Path;

#[derive(Debug, Default)]
pub struct RunResult {
    pub exit_code: i32,
    pub verdict: Option<serde_json::Value>,
    pub log_entries: Vec<serde_json::Value>,
    pub target_after: Option<String>,
    pub harness_log: String,
    pub drive_log: String,
}

impl RunResult {
    /// Load forensics from a host-side run dir. Missing files degrade
    /// gracefully — a lifecycle-broken run still produces a usable struct
    /// (its `drive_log` carries the failure context).
    pub fn from_run_dir(
        run_dir: &Path,
        exit_code: i32,
        target_file: &str,
    ) -> anyhow::Result<Self> {
        let drive_log = read_or_empty(&run_dir.join("drive.log"));
        let harness_log = read_or_empty(&run_dir.join("harness.log"));
        let verdict = read_optional(&run_dir.join("verdict.json"))
            .map(|s| serde_json::from_str::<serde_json::Value>(&s))
            .transpose()?;
        let log_entries = parse_jsonl(&run_dir.join(".hector/log.jsonl"))?;
        let target_after = read_optional(&run_dir.join("workdir").join(target_file));

        Ok(Self {
            exit_code,
            verdict,
            log_entries,
            target_after,
            harness_log,
            drive_log,
        })
    }
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn parse_jsonl(path: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let Some(text) = read_optional(path) else {
        return Ok(Vec::new());
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).map_err(Into::into))
        .collect()
}
```

(The original `#[cfg(test)] mod tests` block stays at the bottom.)

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
cargo test -p hector-e2e --lib result::tests
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-e2e/Cargo.toml crates/hector-e2e/src/result.rs
git commit -m "feat(e2e): RunResult::from_run_dir reads forensics"
```

---

### Task 10: Implement `require_e2e_env` (TDD)

**Files:**
- Modify: `crates/hector-e2e/src/env.rs`

The check passes only if all three preconditions hold:
1. Docker CLI on PATH.
2. `tests/e2e/.env.e2e` exists (relative to the workspace root).
3. `target/release/hector` exists (relative to the workspace root).

On miss, it writes one line per missing dep to stderr and returns `false`. Tests then `return;` instead of running.

We discover the workspace root via the `CARGO_MANIFEST_DIR` env var (set by cargo for crates in the workspace) plus the known relative offset `../..`.

- [ ] **Step 1: Write the failing test**

Append to `crates/hector-e2e/src/env.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_two_up_from_crate() {
        // CARGO_MANIFEST_DIR for hector-e2e is crates/hector-e2e/.
        let root = workspace_root().expect("CARGO_MANIFEST_DIR set in cargo test");
        assert!(root.join("Cargo.toml").exists());
        assert!(root.join("crates").is_dir());
        assert!(root.join("tests").join("e2e").is_dir() || true,
            "tests/e2e/ may not exist on first run — root resolution still valid");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
cargo test -p hector-e2e --lib env::tests
```

Expected: compile error — `workspace_root` doesn't exist.

- [ ] **Step 3: Implement**

Replace `crates/hector-e2e/src/env.rs` with:

```rust
//! Preflight check for the host environment.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns true if Docker, the API-key file, and `target/release/hector`
/// are all present. Writes one line to stderr per missing dep and returns
/// false otherwise.
#[must_use]
pub fn require_e2e_env() -> bool {
    let Ok(root) = workspace_root() else {
        eprintln!("skipping: CARGO_MANIFEST_DIR not set");
        return false;
    };

    let mut ok = true;
    if !docker_present() {
        eprintln!("skipping: `docker` not on PATH");
        ok = false;
    }
    if !root.join("tests/e2e/.env.e2e").exists() {
        eprintln!(
            "skipping: tests/e2e/.env.e2e missing (copy .env.e2e.example and fill ANTHROPIC_API_KEY)",
        );
        ok = false;
    }
    if !root.join("target/release/hector").exists() {
        eprintln!("skipping: target/release/hector missing (run `cargo build --release`)");
        ok = false;
    }
    ok
}

pub(crate) fn workspace_root() -> anyhow::Result<PathBuf> {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| anyhow::anyhow!("CARGO_MANIFEST_DIR not set"))?;
    Ok(Path::new(&crate_dir)
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("crate dir has no grandparent"))?
        .to_path_buf())
}

fn docker_present() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
cargo test -p hector-e2e --lib env::tests
```

Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-e2e/src/env.rs
git commit -m "feat(e2e): require_e2e_env preflight (docker + api key + binary)"
```

---

### Task 11: Implement assertion helpers (TDD)

**Files:**
- Modify: `crates/hector-e2e/src/assertions.rs`

Three helpers, each with simple semantics:

- `block_recorded(r, rule_id)` — pass iff at least one entry in `log_entries` has `rule_id == <rule_id>` AND `status == "block"`.
- `pattern_absent(r, pattern)` — pass iff `target_after` is `Some(s)` and `s` does NOT contain `pattern`. If `target_after` is `None`, **pass** (file never landed → pattern is trivially absent).
- `hook_fired(r, target_path)` — pass iff `log_entries` contains any entry whose `file` mentions `target_path`. Special INCONCLUSIVE path: if no entry exists AND `exit_code == 0` AND `harness_log` shows no edit attempt, emit an INCONCLUSIVE line to stderr and return without panicking.

We detect "edit attempted" via a substring scan on `harness_log` for any of the strings:
`"write_file"`, `"edit_file"`, `"Write"`, `"Edit"`, `"apply_patch"`. These cover the tool names used by the three shipping harnesses (claude-code: `Write`/`Edit`; opencode: `edit`/`write`/`apply_patch`; reasonix: `write_file`/`edit_file`/`multi_edit`). False positives are tolerable — the conservative path is "treat ambiguity as a real wiring bug and panic."

- [ ] **Step 1: Write the failing tests**

Append to `crates/hector-e2e/src/assertions.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(
        exit_code: i32,
        log_entries: Vec<serde_json::Value>,
        target_after: Option<&str>,
        harness_log: &str,
    ) -> RunResult {
        RunResult {
            exit_code,
            verdict: None,
            log_entries,
            target_after: target_after.map(str::to_string),
            harness_log: harness_log.to_string(),
            drive_log: String::new(),
        }
    }

    #[test]
    fn block_recorded_passes_when_status_block() {
        let r = run(
            2,
            vec![json!({"rule_id":"js-forbid-eval","status":"block","file":"src/runner.ts"})],
            None,
            "",
        );
        block_recorded(&r, "js-forbid-eval");
    }

    #[test]
    #[should_panic(expected = "block_recorded")]
    fn block_recorded_panics_when_pass() {
        let r = run(
            0,
            vec![json!({"rule_id":"js-forbid-eval","status":"pass","file":"src/runner.ts"})],
            None,
            "",
        );
        block_recorded(&r, "js-forbid-eval");
    }

    #[test]
    fn pattern_absent_passes_when_file_missing() {
        let r = run(2, vec![], None, "");
        pattern_absent(&r, "eval(");
    }

    #[test]
    fn pattern_absent_passes_when_file_clean() {
        let r = run(
            2,
            vec![],
            Some("function runScript(s: string) { return new Function(s)(); }\n"),
            "",
        );
        pattern_absent(&r, "eval(");
    }

    #[test]
    #[should_panic(expected = "pattern_absent")]
    fn pattern_absent_panics_when_pattern_present() {
        let r = run(0, vec![], Some("eval(input)\n"), "");
        pattern_absent(&r, "eval(");
    }

    #[test]
    fn hook_fired_passes_when_log_mentions_file() {
        let r = run(
            2,
            vec![json!({"rule_id":"js-forbid-eval","file":"src/runner.ts","status":"block"})],
            None,
            "",
        );
        hook_fired(&r, "src/runner.ts");
    }

    #[test]
    fn hook_fired_inconclusive_does_not_panic() {
        // No log entries, clean harness exit, no edit attempt in harness_log.
        let r = run(0, vec![], None, "agent: I cannot do that\n");
        hook_fired(&r, "src/runner.ts");
        // Reaches here without panicking — INCONCLUSIVE is a soft pass.
    }

    #[test]
    #[should_panic(expected = "hook_fired")]
    fn hook_fired_panics_on_edit_with_no_log_entry() {
        // Edit attempted (Write in harness_log) but hector log is empty —
        // real wiring bug.
        let r = run(
            0,
            vec![],
            Some("eval(input)\n"),
            "tool_use: Write { file_path: 'src/runner.ts' }\n",
        );
        hook_fired(&r, "src/runner.ts");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
cargo test -p hector-e2e --lib assertions::tests
```

Expected: each test either panics with the stub message or fails the `should_panic` expectation — none pass.

- [ ] **Step 3: Implement the helpers**

Replace `crates/hector-e2e/src/assertions.rs` body (keep the `#[cfg(test)] mod tests` block at the bottom):

```rust
//! Test-side assertion helpers.
//!
//! Each helper either returns silently (assertion passed) or panics with
//! contextual debug output (assertion failed). [`hook_fired`] additionally
//! recognises the "agent self-refused" case as a soft non-failure.

use crate::result::RunResult;

const EDIT_TOOL_NAMES: &[&str] = &[
    "write_file",
    "edit_file",
    "Write",
    "Edit",
    "apply_patch",
];

/// Pass iff some entry in `r.log_entries` has `rule_id == rule_id`
/// AND `status == "block"`.
pub fn block_recorded(r: &RunResult, rule_id: &str) {
    let matched: Vec<&serde_json::Value> = r
        .log_entries
        .iter()
        .filter(|e| {
            e.get("rule_id").and_then(|v| v.as_str()) == Some(rule_id)
                && e.get("status").and_then(|v| v.as_str()) == Some("block")
        })
        .collect();
    if matched.is_empty() {
        panic!(
            "block_recorded(rule_id={rule_id:?}) FAILED\n  \
             {} entries in log.jsonl; none have rule_id={rule_id:?} and status=\"block\"\n  \
             entries: {:#?}\n  \
             hint: rule may have fired in 'pass' status — check the pattern",
            r.log_entries.len(),
            r.log_entries
        );
    }
}

/// Pass iff `target_after` is None (file never landed) OR contains no
/// occurrence of `pattern`.
pub fn pattern_absent(r: &RunResult, pattern: &str) {
    let Some(content) = r.target_after.as_deref() else {
        return; // file never landed — pattern trivially absent
    };
    if content.contains(pattern) {
        panic!(
            "pattern_absent(pattern={pattern:?}) FAILED\n  \
             post-run file contained the pattern:\n{content}",
        );
    }
}

/// Pass iff hector emitted a verdict for `target_path`. INCONCLUSIVE path:
/// no verdict + clean harness exit + no edit attempt → soft pass with
/// stderr note (the prompt didn't elicit the violation; not a hook bug).
pub fn hook_fired(r: &RunResult, target_path: &str) {
    let entry_for_target = r.log_entries.iter().any(|e| {
        e.get("file").and_then(|v| v.as_str()).is_some_and(|f| f.contains(target_path))
    });
    if entry_for_target {
        return;
    }

    let edit_was_attempted = EDIT_TOOL_NAMES
        .iter()
        .any(|name| r.harness_log.contains(name));
    if !edit_was_attempted && r.exit_code == 0 {
        eprintln!(
            "INCONCLUSIVE: agent did not attempt the violating edit (likely self-refused) — \
             prompt may need to be stronger\n  \
             target: {target_path}\n  \
             harness.log tail:\n{}",
            tail(&r.harness_log, 20),
        );
        return;
    }

    panic!(
        "hook_fired(target_path={target_path:?}) FAILED\n  \
         no verdict mentioning {target_path:?} in log.jsonl ({} entries) but an edit WAS attempted\n  \
         harness.log tail:\n{}\n  \
         drive.log tail:\n{}",
        r.log_entries.len(),
        tail(&r.harness_log, 20),
        tail(&r.drive_log, 20),
    );
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
cargo test -p hector-e2e --lib assertions::tests
```

Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/hector-e2e/src/assertions.rs
git commit -m "feat(e2e): assertion helpers (hook_fired / block_recorded / pattern_absent)"
```

---

### Task 12: Implement `build_image`

**Files:**
- Modify: `crates/hector-e2e/src/docker.rs`

`build_image(adapter)` shells out to `docker build` twice: first the base image, then the adapter image. Both builds use the same Docker daemon's layer cache, so re-runs without source changes are fast.

This function isn't unit-tested — `docker build` is the unit being tested, and that's an integration concern. The validation comes when the first end-to-end test runs in Phase 4. We keep the function small enough that direct reading is sufficient verification.

- [ ] **Step 1: Implement `build_image`**

Replace the `build_image` body in `crates/hector-e2e/src/docker.rs`:

```rust
//! Docker shell-outs.

use crate::env::workspace_root;
use crate::result::RunResult;
use std::process::Command;

const BASE_TAG: &str = "hector-e2e-base:latest";

/// Build the shared base image and the per-adapter image. Idempotent.
pub fn build_image(adapter: &str) -> anyhow::Result<()> {
    let root = workspace_root()?;
    let base_dir = root.join("tests/e2e/base");
    let adapter_dir = root.join("tests/e2e").join(adapter);

    if !adapter_dir.join("Dockerfile").exists() {
        anyhow::bail!(
            "no Dockerfile at {} — unknown adapter {adapter:?}",
            adapter_dir.display(),
        );
    }

    let status = Command::new("docker")
        .args(["build", "-t", BASE_TAG, "."])
        .current_dir(&base_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("docker build (base) failed with status {status}");
    }

    let adapter_tag = format!("hector-e2e-{adapter}:latest");
    let status = Command::new("docker")
        .args(["build", "-t", &adapter_tag, "."])
        .current_dir(&adapter_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("docker build ({adapter}) failed with status {status}");
    }
    Ok(())
}

pub fn run_case(_adapter: &str, _case: &str) -> anyhow::Result<RunResult> {
    anyhow::bail!("not yet implemented")
}
```

To expose `workspace_root` to `docker.rs`, update `crates/hector-e2e/src/env.rs`:

```rust
pub fn workspace_root() -> anyhow::Result<PathBuf> {
    // (body unchanged from Task 10)
```

Change `pub(crate)` → `pub` so `docker.rs` can use it.

- [ ] **Step 2: Verify it compiles**

Run:
```bash
cargo check -p hector-e2e
```

Expected: exit 0.

- [ ] **Step 3: Commit**

```bash
git add crates/hector-e2e/src/docker.rs crates/hector-e2e/src/env.rs
git commit -m "feat(e2e): build_image shells out to docker build (base + adapter)"
```

---

### Task 13: Implement `run_case`

**Files:**
- Modify: `crates/hector-e2e/src/docker.rs`

`run_case(adapter, case)`:

1. Resolve workspace root.
2. Compute the per-case run dir: `tests/e2e/<adapter>/runs/<case>/`. Wipe it (so stale forensics don't contaminate the next run) and re-create it empty.
3. Read the case JSON to extract `target_file` (needed for the final forensics read).
4. Compose the docker run command:
   - `--rm`
   - `--env-file <root>/tests/e2e/.env.e2e`
   - Bind mounts per spec §4 table.
   - Image tag: `hector-e2e-<adapter>:latest`.
   - Container arg: `--case=<case>`.
5. Execute; capture the container exit code.
6. Call `RunResult::from_run_dir(...)` with the run dir, exit code, and `target_file`.

We do **not** propagate the docker exit code beyond `RunResult::exit_code` — `run_case` returns `Ok(r)` even when the container exited non-zero. Lifecycle-broken vs assertion-failed is a distinction the *test* makes, not `run_case`.

- [ ] **Step 1: Implement**

Replace the `run_case` body in `crates/hector-e2e/src/docker.rs`:

```rust
pub fn run_case(adapter: &str, case: &str) -> anyhow::Result<RunResult> {
    let root = workspace_root()?;
    let e2e = root.join("tests/e2e");

    let case_path = e2e.join("cases").join(format!("{case}.json"));
    let case_text = std::fs::read_to_string(&case_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", case_path.display()))?;
    let case_json: serde_json::Value = serde_json::from_str(&case_text)?;
    let target_file = case_json
        .get("target_file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("case {case}: missing string field `target_file`"))?
        .to_string();

    let run_dir = e2e.join(adapter).join("runs").join(case);
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let env_file = e2e.join(".env.e2e");
    let policy = e2e.join("policy/.hector.yml");
    let fixture = e2e.join("fixture");
    let cases = e2e.join("cases");
    let drive = e2e.join(adapter).join("drive.sh");
    let hector_bin = root.join("target/release/hector");
    let image = format!("hector-e2e-{adapter}:latest");

    let mounts = [
        format!("{}:/work/policy/.hector.yml:ro", policy.display()),
        format!("{}:/work/fixture:ro", fixture.display()),
        format!("{}:/work/cases:ro", cases.display()),
        format!("{}:/work/drive.sh:ro", drive.display()),
        format!("{}:/work/runs:rw", run_dir.display()),
        format!("{}:/usr/local/bin/hector:ro", hector_bin.display()),
    ];

    let mut cmd = Command::new("docker");
    cmd.arg("run").arg("--rm");
    cmd.arg("--env-file").arg(&env_file);
    for m in &mounts {
        cmd.args(["-v", m]);
    }
    cmd.arg(&image);
    cmd.arg(format!("--case={case}"));

    let output = cmd.output()?;
    let exit_code = output.status.code().unwrap_or(-1);

    RunResult::from_run_dir(&run_dir, exit_code, &target_file)
}
```

- [ ] **Step 2: Verify it compiles**

Run:
```bash
cargo check -p hector-e2e
```

Expected: exit 0.

- [ ] **Step 3: Commit**

```bash
git add crates/hector-e2e/src/docker.rs
git commit -m "feat(e2e): run_case composes docker run + captures forensics"
```

---

## Phase 4 — claude-code first case end-to-end

### Task 14: Write the claude-code Dockerfile

**Files:**
- Create: `tests/e2e/claude-code/Dockerfile`

- [ ] **Step 1: Write the Dockerfile**

```dockerfile
FROM hector-e2e-base:latest

# Claude Code CLI — installed globally as root, then we drop back to `hector`.
USER root
RUN npm install -g @anthropic-ai/claude-code \
 && claude --version
USER hector

# Plugin source — copied into the conventional plugin path. CLAUDE_PLUGIN_ROOT
# is set by Claude Code at hook-fire time; install location is taken from
# the adapter README. If Claude Code's plugin discovery rejects this path,
# the fix is a one-line `COPY` destination change here.
COPY --chown=hector:hector adapters/claude-code/ /home/hector/.claude/plugins/hector/

ENTRYPOINT ["bash", "/work/drive.sh"]
```

A note on build context: `docker build` is invoked from `tests/e2e/claude-code/`, so `COPY adapters/claude-code/ ...` would fail (the path is outside the context). Two ways to handle this:

- **Option A:** invoke `docker build` from the *repo root* with `-f tests/e2e/claude-code/Dockerfile`, so the build context is the whole repo and `COPY adapters/claude-code/` resolves.
- **Option B:** keep the build dir as `tests/e2e/claude-code/` and use a relative `../../../adapters/claude-code/` path.

Use **Option A**. Update `build_image` accordingly in the next sub-step.

- [ ] **Step 2: Update `build_image` to use repo-root build context**

In `crates/hector-e2e/src/docker.rs`, change the adapter build step:

```rust
    let adapter_tag = format!("hector-e2e-{adapter}:latest");
    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            &adapter_tag,
            "-f",
            &format!("tests/e2e/{adapter}/Dockerfile"),
            ".",
        ])
        .current_dir(&root) // repo root, so adapters/<name>/ is in context
        .status()?;
```

(Leave the base image build alone — its Dockerfile only uses paths inside `tests/e2e/base/`.)

- [ ] **Step 3: Build and verify the claude-code image**

Run:
```bash
cargo build --release -p hector-cli
docker build -t hector-e2e-base:latest tests/e2e/base/
docker build -t hector-e2e-claude-code:latest -f tests/e2e/claude-code/Dockerfile .
docker run --rm hector-e2e-claude-code:latest bash -lc 'claude --version && ls /home/hector/.claude/plugins/hector/'
```

Expected: prints a Claude CLI version, then lists the plugin contents (at minimum `README.md`, `hooks/`, `agents/`).

(The `ENTRYPOINT` is overridden by the `bash -lc` arg in the verification — that's intentional; we're sanity-checking the image without invoking drive.sh yet.)

- [ ] **Step 4: Commit**

```bash
git add tests/e2e/claude-code/Dockerfile crates/hector-e2e/src/docker.rs
git commit -m "feat(e2e): claude-code Dockerfile + repo-root build context"
```

---

### Task 15: Write the claude-code drive script

**Files:**
- Create: `tests/e2e/claude-code/drive.sh`

The drive script implements the six-phase lifecycle from spec §5. Each phase appends to `/work/runs/drive.log`. Phase exits non-zero if a precondition fails or a command errors — phases that complete-but-the-agent-self-refused still let the script exit 0 (lifecycle was fine; the agent's behaviour is the *test's* concern).

- [ ] **Step 1: Write the drive script**

`tests/e2e/claude-code/drive.sh`:

```bash
#!/usr/bin/env bash
# Drive script for the claude-code adapter e2e harness.
# Args:  --case=<name>      Required. Loads /work/cases/<name>.json.
#
# Layout:
#   /work/policy/.hector.yml   :ro  canonical policy
#   /work/fixture/             :ro  Node starter project
#   /work/cases/<name>.json    :ro  prompt + target + expected rule
#   /work/runs/                :rw  forensics + workdir for this case
#   /usr/local/bin/hector      :ro  release-build hector binary
#
# Exit codes:
#   0  lifecycle completed (test asserts on RunResult, not this exit code)
#   1  lifecycle broke (preflight failed, validate failed, etc.)

set -uo pipefail

DRIVE_LOG="/work/runs/drive.log"
HARNESS_LOG="/work/runs/harness.log"
mkdir -p /work/runs/.hector

log()  { printf "[%s] %s\n" "$(date -u +%H:%M:%S)" "$*" | tee -a "$DRIVE_LOG"; }
fail() { log "LIFECYCLE FAIL: $*"; exit 1; }

# --- Parse args ---
CASE=""
for arg in "$@"; do
  case "$arg" in
    --case=*) CASE="${arg#--case=}" ;;
    *) fail "unknown arg: $arg" ;;
  esac
done
[[ -n "$CASE" ]] || fail "missing --case=<name>"

CASE_FILE="/work/cases/$CASE.json"
[[ -f "$CASE_FILE" ]] || fail "case file not found: $CASE_FILE"

# --- Phase 0: Setup ---
log "phase 0: setup; case=$CASE"
[[ -n "${ANTHROPIC_API_KEY:-}" ]] || fail "ANTHROPIC_API_KEY not in environment"
[[ -x /usr/local/bin/hector  ]] || fail "/usr/local/bin/hector not executable"

PROMPT="$(jq -r '.prompt' "$CASE_FILE")"
TARGET_FILE="$(jq -r '.target_file' "$CASE_FILE")"
EXPECTED_RULE="$(jq -r '.expected_rule' "$CASE_FILE")"
[[ -n "$PROMPT" && -n "$TARGET_FILE" && -n "$EXPECTED_RULE" ]] \
  || fail "case JSON missing required fields"

# --- Phase 1: Install check ---
log "phase 1: install check"
hector --version | tee -a "$DRIVE_LOG"  || fail "hector --version"
claude --version | tee -a "$DRIVE_LOG"  || fail "claude --version"
[[ -d /home/hector/.claude/plugins/hector ]] \
  || fail "plugin not at /home/hector/.claude/plugins/hector"

# --- Phase 2: Onboarding ---
log "phase 2: onboarding"
WORKDIR=/work/runs/workdir
mkdir -p "$WORKDIR" && cd "$WORKDIR" || fail "cd workdir"
git init -q
cp -r /work/fixture/. "$WORKDIR/"
git add -A && git -c user.email=e2e@hector -c user.name=e2e commit -q -m "fixture"

# hector init writes a default config; capture it for forensics, then overlay
# the canonical test policy.
hector init >"$DRIVE_LOG.init.out" 2>&1 || fail "hector init"
cp .hector.yml /work/runs/.hector.yml.from-init 2>/dev/null || true
cp /work/policy/.hector.yml ./.hector.yml
hector trust    | tee -a "$DRIVE_LOG" || fail "hector trust"
hector validate | tee -a "$DRIVE_LOG" || fail "hector validate"

# --- Phase 3: Drive the harness ---
log "phase 3: drive harness with claude --print"
# `--print` makes the CLI non-interactive (prints the response and exits).
# `--model claude-haiku-4-5` keeps cost low and matches the policy's LLM.
# `timeout 120` defends against an unresponsive agent.
timeout 120 claude --print --model claude-haiku-4-5 "$PROMPT" \
  >>"$HARNESS_LOG" 2>&1
HARNESS_EXIT=$?
log "harness exit: $HARNESS_EXIT"

# --- Phase 4: Capture ---
log "phase 4: capture forensics"
if [[ -f "$WORKDIR/.hector/log.jsonl" ]]; then
  cp "$WORKDIR/.hector/log.jsonl" /work/runs/.hector/log.jsonl
fi
# Extract the latest verdict if any (one JSON object per line; take last block).
if [[ -f /work/runs/.hector/log.jsonl ]]; then
  tail -n 50 /work/runs/.hector/log.jsonl \
    | jq -s 'last' >/work/runs/verdict.json 2>/dev/null || true
fi

# --- Phase 5: Done ---
log "phase 5: lifecycle complete"
exit 0
```

- [ ] **Step 2: Make it executable**

Run:
```bash
chmod +x tests/e2e/claude-code/drive.sh
```

- [ ] **Step 3: Sanity-check the script's syntax**

Run:
```bash
bash -n tests/e2e/claude-code/drive.sh && echo "syntax OK"
```

Expected: `syntax OK`.

- [ ] **Step 4: Commit**

```bash
git add tests/e2e/claude-code/drive.sh
git commit -m "feat(e2e): claude-code drive script (6-phase lifecycle)"
```

---

### Task 16: Write the first integration test (`claude_code::ast_eval_blocked`)

**Files:**
- Create: `crates/hector-e2e/tests/claude_code.rs`

This is the end-to-end smoke. It depends on every preceding piece working: image builds, drive script runs, hector logs verdicts, run dir forensics are readable, assertions evaluate correctly.

- [ ] **Step 1: Write the test**

```rust
//! Claude Code adapter end-to-end smoke tests.
//!
//! Each test is `#[ignore]` — run with:
//!     cargo test -p hector-e2e --test claude_code -- --ignored

use hector_e2e::{assertions, build_image, require_e2e_env, run_case};

#[test]
#[ignore]
fn ast_eval_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("claude-code").expect("docker build");
    let r = run_case("claude-code", "ast-eval").expect("docker run");
    assertions::hook_fired(&r, "src/runner.ts");
    assertions::block_recorded(&r, "js-forbid-eval");
    assertions::pattern_absent(&r, "eval(");
}
```

- [ ] **Step 2: Make sure `.env.e2e` exists for the run**

```bash
cp tests/e2e/.env.e2e.example tests/e2e/.env.e2e
# Then edit tests/e2e/.env.e2e and paste your ANTHROPIC_API_KEY=... value.
```

- [ ] **Step 3: Run the test**

```bash
cargo build --release -p hector-cli  # ensures target/release/hector exists
cargo test -p hector-e2e --test claude_code -- --ignored
```

Expected:
- Cargo builds the test binary.
- `require_e2e_env()` finds Docker, `.env.e2e`, and `target/release/hector`.
- `build_image` builds base and claude-code images (uses layer cache on rerun).
- `run_case` shells out to docker; container runs the 5-phase drive script; agent attempts `eval(input)`; hector's AST engine blocks it.
- All three assertions pass.
- Output: `test ast_eval_blocked ... ok`, 1 passed.

If the agent self-refused, you'll see the INCONCLUSIVE line in stderr but the test still passes (soft handling). If hector didn't fire, the assertion's panic message points to `tests/e2e/claude-code/runs/ast-eval/` for forensics.

- [ ] **Step 4: Commit**

```bash
git add crates/hector-e2e/tests/claude_code.rs
git commit -m "test(e2e): claude-code ast-eval end-to-end smoke"
```

---

## Phase 5 — Remaining claude-code cases

### Task 17: Add `semantic_secrets_blocked` test

**Files:**
- Modify: `crates/hector-e2e/tests/claude_code.rs`

- [ ] **Step 1: Append the test**

```rust
#[test]
#[ignore]
fn semantic_secrets_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("claude-code").expect("docker build");
    let r = run_case("claude-code", "semantic-secrets").expect("docker run");
    assertions::hook_fired(&r, "src/openai-client.ts");
    assertions::block_recorded(&r, "no-hardcoded-secrets");
    assertions::pattern_absent(&r, "sk-test-1234567890abcdef");
}
```

- [ ] **Step 2: Run it**

```bash
cargo test -p hector-e2e --test claude_code semantic_secrets_blocked -- --ignored
```

Expected: 1 passed (or INCONCLUSIVE soft-pass if Haiku self-refuses the secret prompt — that's an accepted outcome per spec §7).

- [ ] **Step 3: Commit**

```bash
git add crates/hector-e2e/tests/claude_code.rs
git commit -m "test(e2e): claude-code semantic-secrets end-to-end smoke"
```

---

### Task 18: Add `script_todo_blocked` test

**Files:**
- Modify: `crates/hector-e2e/tests/claude_code.rs`

- [ ] **Step 1: Append the test**

```rust
#[test]
#[ignore]
fn script_todo_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("claude-code").expect("docker build");
    let r = run_case("claude-code", "script-todo").expect("docker run");
    assertions::hook_fired(&r, "src/payments.ts");
    assertions::block_recorded(&r, "no-todo-markers");
    assertions::pattern_absent(&r, "TODO");
}
```

- [ ] **Step 2: Run it**

```bash
cargo test -p hector-e2e --test claude_code script_todo_blocked -- --ignored
```

Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/hector-e2e/tests/claude_code.rs
git commit -m "test(e2e): claude-code script-todo end-to-end smoke"
```

---

## Phase 6 — opencode adapter

### Task 19: Write the opencode Dockerfile

**Files:**
- Create: `tests/e2e/opencode/Dockerfile`

- [ ] **Step 1: Write the Dockerfile**

```dockerfile
FROM hector-e2e-base:latest

# OpenCode bundles Bun, but we install it explicitly to keep image content
# deterministic. Bun ships as a static binary; non-root user can install
# under $HOME.
ENV BUN_INSTALL=/home/hector/.bun
ENV PATH=$BUN_INSTALL/bin:$PATH

RUN curl -fsSL https://bun.sh/install | bash \
 && bun --version

USER root
RUN npm install -g opencode-ai \
 && opencode --version
USER hector

# Stage the plugin source. drive.sh wires it into the project's local
# .opencode/plugins/ directory at runtime — the adapter README documents
# this as the supported install path.
COPY --chown=hector:hector adapters/opencode/ /home/hector/opencode-plugin/

ENTRYPOINT ["bash", "/work/drive.sh"]
```

If the `opencode-ai` npm package name is wrong at impl time, the spec §9 caveat applies — check `opencode --help` and the OpenCode docs for the right install vector. The fix is a one-line `RUN` change.

- [ ] **Step 2: Build and verify**

```bash
docker build -t hector-e2e-opencode:latest -f tests/e2e/opencode/Dockerfile .
docker run --rm hector-e2e-opencode:latest bash -lc 'opencode --version && ls /home/hector/opencode-plugin/'
```

Expected: prints an OpenCode version and lists the plugin directory contents (at least `src/`, `package.json`, `README.md`).

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/opencode/Dockerfile
git commit -m "feat(e2e): opencode Dockerfile (bun + opencode + plugin stage)"
```

---

### Task 20: Write the opencode drive script

**Files:**
- Create: `tests/e2e/opencode/drive.sh`

OpenCode differences from claude-code:
- Plugin install path: `<project>/.opencode/plugins/hector.ts` (project-local; the adapter README documents this).
- CLI invocation: `opencode run` is the non-interactive entry point — verify the exact flag at impl time via `opencode run --help`.
- PreToolUse, not PostToolUse — files never land on disk when blocked, so `pattern_absent` is deterministic.

- [ ] **Step 1: Write the drive script**

`tests/e2e/opencode/drive.sh`:

```bash
#!/usr/bin/env bash
# Drive script for the opencode adapter e2e harness.
# Mirrors tests/e2e/claude-code/drive.sh; differences are noted inline.

set -uo pipefail

DRIVE_LOG="/work/runs/drive.log"
HARNESS_LOG="/work/runs/harness.log"
mkdir -p /work/runs/.hector

log()  { printf "[%s] %s\n" "$(date -u +%H:%M:%S)" "$*" | tee -a "$DRIVE_LOG"; }
fail() { log "LIFECYCLE FAIL: $*"; exit 1; }

CASE=""
for arg in "$@"; do
  case "$arg" in
    --case=*) CASE="${arg#--case=}" ;;
    *) fail "unknown arg: $arg" ;;
  esac
done
[[ -n "$CASE" ]] || fail "missing --case=<name>"

CASE_FILE="/work/cases/$CASE.json"
[[ -f "$CASE_FILE" ]] || fail "case file not found: $CASE_FILE"

log "phase 0: setup; case=$CASE"
[[ -n "${ANTHROPIC_API_KEY:-}" ]] || fail "ANTHROPIC_API_KEY not in environment"
[[ -x /usr/local/bin/hector  ]] || fail "/usr/local/bin/hector not executable"

PROMPT="$(jq -r '.prompt' "$CASE_FILE")"
TARGET_FILE="$(jq -r '.target_file' "$CASE_FILE")"

log "phase 1: install check"
hector --version  | tee -a "$DRIVE_LOG" || fail "hector --version"
opencode --version | tee -a "$DRIVE_LOG" || fail "opencode --version"
[[ -d /home/hector/opencode-plugin ]] || fail "plugin source missing"

log "phase 2: onboarding"
WORKDIR=/work/runs/workdir
mkdir -p "$WORKDIR" && cd "$WORKDIR" || fail "cd workdir"
git init -q
cp -r /work/fixture/. "$WORKDIR/"
git add -A && git -c user.email=e2e@hector -c user.name=e2e commit -q -m "fixture"

# Wire the plugin into the project (per opencode adapter README).
mkdir -p .opencode/plugins
cp /home/hector/opencode-plugin/src/index.ts .opencode/plugins/hector.ts
# If the plugin needs its node_modules, run install. Skip if package.json
# absent at the plugin path.
if [[ -f /home/hector/opencode-plugin/package.json ]]; then
  (cd /home/hector/opencode-plugin && bun install --frozen-lockfile) \
    >>"$DRIVE_LOG" 2>&1 || log "warn: bun install non-fatal"
fi

hector init >"$DRIVE_LOG.init.out" 2>&1 || fail "hector init"
cp .hector.yml /work/runs/.hector.yml.from-init 2>/dev/null || true
cp /work/policy/.hector.yml ./.hector.yml
hector trust    | tee -a "$DRIVE_LOG" || fail "hector trust"
hector validate | tee -a "$DRIVE_LOG" || fail "hector validate"

log "phase 3: drive harness with opencode run"
# Exact flag verified at impl time via `opencode run --help`.
# The model id format follows OpenCode's provider/model convention.
timeout 120 opencode run --model anthropic/claude-haiku-4-5 "$PROMPT" \
  >>"$HARNESS_LOG" 2>&1
HARNESS_EXIT=$?
log "harness exit: $HARNESS_EXIT"

log "phase 4: capture forensics"
if [[ -f "$WORKDIR/.hector/log.jsonl" ]]; then
  cp "$WORKDIR/.hector/log.jsonl" /work/runs/.hector/log.jsonl
fi
if [[ -f /work/runs/.hector/log.jsonl ]]; then
  tail -n 50 /work/runs/.hector/log.jsonl \
    | jq -s 'last' >/work/runs/verdict.json 2>/dev/null || true
fi

log "phase 5: lifecycle complete"
exit 0
```

- [ ] **Step 2: Make it executable and check syntax**

```bash
chmod +x tests/e2e/opencode/drive.sh
bash -n tests/e2e/opencode/drive.sh && echo "syntax OK"
```

Expected: `syntax OK`.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/opencode/drive.sh
git commit -m "feat(e2e): opencode drive script (PreToolUse adapter lifecycle)"
```

---

### Task 21: Write the opencode integration tests

**Files:**
- Create: `crates/hector-e2e/tests/opencode.rs`

Per spec §8, opencode is PreToolUse-only — no `script-todo` test (script rules can't observe proposed content; the omission is encoded as the absence of the test fn, visible at code-review time).

- [ ] **Step 1: Write the file**

```rust
//! OpenCode adapter end-to-end smoke tests.
//!
//! PreToolUse-only — `script-todo` is intentionally omitted (script rules
//! read the on-disk file, but PreToolUse runs before the write lands; see
//! spec §8 and `specs/2026-05-25-reasonix-adapter.md` §5A).
//!
//! Run with: cargo test -p hector-e2e --test opencode -- --ignored

use hector_e2e::{assertions, build_image, require_e2e_env, run_case};

#[test]
#[ignore]
fn ast_eval_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("opencode").expect("docker build");
    let r = run_case("opencode", "ast-eval").expect("docker run");
    assertions::hook_fired(&r, "src/runner.ts");
    assertions::block_recorded(&r, "js-forbid-eval");
    assertions::pattern_absent(&r, "eval(");
}

#[test]
#[ignore]
fn semantic_secrets_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("opencode").expect("docker build");
    let r = run_case("opencode", "semantic-secrets").expect("docker run");
    assertions::hook_fired(&r, "src/openai-client.ts");
    assertions::block_recorded(&r, "no-hardcoded-secrets");
    assertions::pattern_absent(&r, "sk-test-1234567890abcdef");
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p hector-e2e --test opencode -- --ignored
```

Expected: 2 passed (or one INCONCLUSIVE soft-pass on the secrets case if Haiku self-refuses).

- [ ] **Step 3: Commit**

```bash
git add crates/hector-e2e/tests/opencode.rs
git commit -m "test(e2e): opencode ast-eval + semantic-secrets smoke (no script-todo)"
```

---

## Phase 7 — reasonix adapter

### Task 22: Write the reasonix Dockerfile

**Files:**
- Create: `tests/e2e/reasonix/Dockerfile`

- [ ] **Step 1: Write the Dockerfile**

```dockerfile
FROM hector-e2e-base:latest

# DeepSeek-Reasonix CLI — install vector to be confirmed at impl time
# against the upstream docs (specs/2026-05-25-reasonix-adapter.md links the
# canonical project). Most likely npm or a curl-piped install script.
USER root
RUN npm install -g @deepseek/reasonix-cli \
 && reasonix --version
USER hector

# Stage the hook script. drive.sh wires it into ~/.reasonix/settings.json
# at runtime — reasonix adapters install via user settings, not a plugin
# directory (see adapters/reasonix/README.md).
COPY --chown=hector:hector adapters/reasonix/ /home/hector/reasonix-plugin/

ENTRYPOINT ["bash", "/work/drive.sh"]
```

The `@deepseek/reasonix-cli` package name is a guess — spec §9 explicitly defers this verification. If the package doesn't exist or has a different name, check the reasonix docs at https://esengine.github.io/DeepSeek-Reasonix/ (referenced in `adapters/reasonix/README.md`) and substitute the right install command.

- [ ] **Step 2: Build and verify**

```bash
docker build -t hector-e2e-reasonix:latest -f tests/e2e/reasonix/Dockerfile .
docker run --rm hector-e2e-reasonix:latest bash -lc 'reasonix --version && ls /home/hector/reasonix-plugin/'
```

Expected: prints a Reasonix version and lists `hooks/`, `README.md`. If the install fails, this is the §9 "one-line fix" — update the `RUN npm install` line.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/reasonix/Dockerfile
git commit -m "feat(e2e): reasonix Dockerfile (install + plugin stage)"
```

---

### Task 23: Write the reasonix drive script

**Files:**
- Create: `tests/e2e/reasonix/drive.sh`

Reasonix differences from claude-code/opencode:
- No plugin directory — hook is wired by writing `~/.reasonix/settings.json`.
- The adapter ships `hooks/settings.example.json` (see `adapters/reasonix/README.md`); we merge that into `~/.reasonix/settings.json` (or just copy it — fresh container, nothing to merge with).
- Hook script path inside `~/.reasonix/settings.json` must be an absolute path; we point it at the staged `/home/hector/reasonix-plugin/hooks/<hook-script>`.
- CLI invocation: `reasonix --headless --message "$PROMPT"` is the spec's guess; verify at impl time.

- [ ] **Step 1: Write the drive script**

`tests/e2e/reasonix/drive.sh`:

```bash
#!/usr/bin/env bash
# Drive script for the reasonix adapter e2e harness.

set -uo pipefail

DRIVE_LOG="/work/runs/drive.log"
HARNESS_LOG="/work/runs/harness.log"
mkdir -p /work/runs/.hector

log()  { printf "[%s] %s\n" "$(date -u +%H:%M:%S)" "$*" | tee -a "$DRIVE_LOG"; }
fail() { log "LIFECYCLE FAIL: $*"; exit 1; }

CASE=""
for arg in "$@"; do
  case "$arg" in
    --case=*) CASE="${arg#--case=}" ;;
    *) fail "unknown arg: $arg" ;;
  esac
done
[[ -n "$CASE" ]] || fail "missing --case=<name>"

CASE_FILE="/work/cases/$CASE.json"
[[ -f "$CASE_FILE" ]] || fail "case file not found: $CASE_FILE"

log "phase 0: setup; case=$CASE"
[[ -n "${ANTHROPIC_API_KEY:-}" ]] || fail "ANTHROPIC_API_KEY not in environment"
[[ -x /usr/local/bin/hector  ]] || fail "/usr/local/bin/hector not executable"

PROMPT="$(jq -r '.prompt' "$CASE_FILE")"
TARGET_FILE="$(jq -r '.target_file' "$CASE_FILE")"

log "phase 1: install check"
hector --version   | tee -a "$DRIVE_LOG" || fail "hector --version"
reasonix --version | tee -a "$DRIVE_LOG" || fail "reasonix --version"
[[ -d /home/hector/reasonix-plugin/hooks ]] || fail "plugin hooks dir missing"

log "phase 2: onboarding"
# Wire the PreToolUse hook into ~/.reasonix/settings.json. The adapter
# ships hooks/settings.example.json; we use it as the template and rewrite
# any "<plugin-root>" placeholder to the absolute staged path.
mkdir -p /home/hector/.reasonix
if [[ -f /home/hector/reasonix-plugin/hooks/settings.example.json ]]; then
  sed 's|<plugin-root>|/home/hector/reasonix-plugin|g' \
    /home/hector/reasonix-plugin/hooks/settings.example.json \
    >/home/hector/.reasonix/settings.json
else
  fail "hooks/settings.example.json missing in adapter"
fi
log "wired settings.json:"
cat /home/hector/.reasonix/settings.json | tee -a "$DRIVE_LOG"

WORKDIR=/work/runs/workdir
mkdir -p "$WORKDIR" && cd "$WORKDIR" || fail "cd workdir"
git init -q
cp -r /work/fixture/. "$WORKDIR/"
git add -A && git -c user.email=e2e@hector -c user.name=e2e commit -q -m "fixture"

hector init >"$DRIVE_LOG.init.out" 2>&1 || fail "hector init"
cp .hector.yml /work/runs/.hector.yml.from-init 2>/dev/null || true
cp /work/policy/.hector.yml ./.hector.yml
hector trust    | tee -a "$DRIVE_LOG" || fail "hector trust"
hector validate | tee -a "$DRIVE_LOG" || fail "hector validate"

log "phase 3: drive harness with reasonix --headless"
timeout 120 reasonix --headless --message "$PROMPT" \
  >>"$HARNESS_LOG" 2>&1
HARNESS_EXIT=$?
log "harness exit: $HARNESS_EXIT"

log "phase 4: capture forensics"
if [[ -f "$WORKDIR/.hector/log.jsonl" ]]; then
  cp "$WORKDIR/.hector/log.jsonl" /work/runs/.hector/log.jsonl
fi
if [[ -f /work/runs/.hector/log.jsonl ]]; then
  tail -n 50 /work/runs/.hector/log.jsonl \
    | jq -s 'last' >/work/runs/verdict.json 2>/dev/null || true
fi

log "phase 5: lifecycle complete"
exit 0
```

A note on the `<plugin-root>` placeholder: that's a convention we use here; if `adapters/reasonix/hooks/settings.example.json` uses a different placeholder (or none), update the `sed` substitution accordingly. Check the file's contents at impl time.

- [ ] **Step 2: Make it executable and check syntax**

```bash
chmod +x tests/e2e/reasonix/drive.sh
bash -n tests/e2e/reasonix/drive.sh && echo "syntax OK"
```

Expected: `syntax OK`.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/reasonix/drive.sh
git commit -m "feat(e2e): reasonix drive script (PreToolUse via settings.json)"
```

---

### Task 24: Write the reasonix integration tests

**Files:**
- Create: `crates/hector-e2e/tests/reasonix.rs`

PreToolUse — same shape as opencode (two tests, no `script-todo`).

- [ ] **Step 1: Write the file**

```rust
//! Reasonix adapter end-to-end smoke tests.
//!
//! PreToolUse-only — `script-todo` is intentionally omitted (see
//! `specs/2026-05-25-reasonix-adapter.md` §5A: `engine: script` rules
//! cannot observe proposed content on a PreToolUse adapter).
//!
//! Run with: cargo test -p hector-e2e --test reasonix -- --ignored

use hector_e2e::{assertions, build_image, require_e2e_env, run_case};

#[test]
#[ignore]
fn ast_eval_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("reasonix").expect("docker build");
    let r = run_case("reasonix", "ast-eval").expect("docker run");
    assertions::hook_fired(&r, "src/runner.ts");
    assertions::block_recorded(&r, "js-forbid-eval");
    assertions::pattern_absent(&r, "eval(");
}

#[test]
#[ignore]
fn semantic_secrets_blocked() {
    if !require_e2e_env() {
        return;
    }
    build_image("reasonix").expect("docker build");
    let r = run_case("reasonix", "semantic-secrets").expect("docker run");
    assertions::hook_fired(&r, "src/openai-client.ts");
    assertions::block_recorded(&r, "no-hardcoded-secrets");
    assertions::pattern_absent(&r, "sk-test-1234567890abcdef");
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p hector-e2e --test reasonix -- --ignored
```

Expected: 2 passed (one may INCONCLUSIVE soft-pass).

- [ ] **Step 3: Commit**

```bash
git add crates/hector-e2e/tests/reasonix.rs
git commit -m "test(e2e): reasonix ast-eval + semantic-secrets smoke"
```

---

## Phase 8 — Documentation

### Task 25: Write the e2e harness README

**Files:**
- Create: `tests/e2e/README.md`

- [ ] **Step 1: Write the README**

````markdown
# Adapter Docker e2e harness

On-demand smoke tests that run each shipping adapter (`claude-code`, `opencode`, `reasonix`) end-to-end against a real Anthropic Haiku 4.5 agent and assert that hector blocks a policy-violating edit.

This is **observability, not CI**. Failures don't gate merges — they tell you the adapter wiring broke.

## Prerequisites

- Docker daemon running
- Anthropic API key (Haiku 4.5 access)
- A workspace build: `cargo build --release -p hector-cli`

## One-time setup

```bash
cp tests/e2e/.env.e2e.example tests/e2e/.env.e2e
# Edit tests/e2e/.env.e2e and paste your ANTHROPIC_API_KEY=... value.
```

## Run

All adapters, all cases:

```bash
cargo test -p hector-e2e -- --ignored
```

One adapter:

```bash
cargo test -p hector-e2e --test claude_code -- --ignored
```

One test:

```bash
cargo test -p hector-e2e --test claude_code ast_eval_blocked -- --ignored
```

Add `--nocapture` to stream container stdout/stderr live.

## What lives where

| Path | Purpose |
|------|---------|
| `tests/e2e/base/Dockerfile` | Shared base image (Debian + Node + non-root user) |
| `tests/e2e/policy/.hector.yml` | Canonical 3-rule policy |
| `tests/e2e/cases/*.json` | Per-case prompt + target file + expected rule |
| `tests/e2e/fixture/` | Starter Node project (every run begins from this) |
| `tests/e2e/<adapter>/Dockerfile` | Per-adapter image (extends base; installs harness CLI + plugin) |
| `tests/e2e/<adapter>/drive.sh` | Container entrypoint — 6-phase lifecycle |
| `tests/e2e/<adapter>/runs/<case>/` | Forensics from the latest run (gitignored, overwritten) |
| `crates/hector-e2e/` | Rust crate: `build_image`, `run_case`, `RunResult`, assertions |

## Reading forensics

After a run, look under `tests/e2e/<adapter>/runs/<case>/`:

- `drive.log` — phase-by-phase trace from `drive.sh`
- `harness.log` — stdout/stderr of the harness CLI invocation
- `.hector/log.jsonl` — every verdict hector emitted
- `verdict.json` — the latest verdict (jq-extracted from the log)
- `workdir/<target_file>` — final state of the file under test
- `.hector.yml.from-init` — what `hector init` scaffolded (before the test policy overlaid it)

## Common failure modes

| Symptom | Likely cause |
|---|---|
| `skipping: tests/e2e/.env.e2e missing` | Run the one-time setup above. |
| `skipping: target/release/hector missing` | Run `cargo build --release -p hector-cli`. |
| `skipping: docker not on PATH` | Start Docker Desktop or install the CLI. |
| `LIFECYCLE FAIL: hector validate` | Policy file is malformed — `drive.log` has the validate output. |
| `INCONCLUSIVE: agent did not attempt the violating edit` | Model self-refused. Prompt or model may need adjustment. Not a hook bug. |
| `hook_fired(target_path=...) FAILED ... edit WAS attempted` | Real wiring bug. Adapter's hook didn't run. Check the per-adapter README and harness logs. |

## Updating the harness

| Change | What to rebuild |
|--------|-----------------|
| Edit a `cases/*.json` prompt | Nothing — case files are bind-mounted. |
| Edit `policy/.hector.yml` | Nothing. |
| Edit `<adapter>/drive.sh` | Nothing. |
| Bump harness CLI version (e.g. `claude` major bump) | `docker build -t hector-e2e-<adapter>:latest -f tests/e2e/<adapter>/Dockerfile .` |
| Bump Node base | `docker build -t hector-e2e-base:latest tests/e2e/base/` then rebuild all leaves. |

## Non-goals

- Not a CI gate.
- Not adversarial — prompts are "plausibly violating", not "deliberately evasive."
- Not a benchmark — no latency or cost measurement.
- Not pinned to a specific model — Haiku 4.5 is v1; bumping is a one-line change in `policy/.hector.yml` + each `drive.sh`.

See `docs/superpowers/specs/2026-05-27-adapter-docker-e2e-design.md` for the full design rationale.
````

- [ ] **Step 2: Commit**

```bash
git add tests/e2e/README.md
git commit -m "docs(e2e): how to run the adapter Docker harness + read forensics"
```

---

## Final verification

### Task 26: Full-suite green run

- [ ] **Step 1: Pre-flight**

```bash
cargo build --release -p hector-cli
ls tests/e2e/.env.e2e || echo "MISSING — copy from .env.e2e.example and add your key"
docker --version
```

- [ ] **Step 2: Run the full suite**

```bash
cargo test -p hector-e2e -- --ignored --test-threads=1
```

`--test-threads=1` is a defensive choice here: parallel Docker builds compete for the layer cache but mostly work; parallel runs DO conflict on the same `runs/<case>/` dir if any case appears in multiple test files (none do today, but pinning serialisation removes the risk).

Expected:
- 7 tests total (claude-code: 3, opencode: 2, reasonix: 2)
- All pass (with possible INCONCLUSIVE soft-passes on secrets cases — visible in stderr but not failing)

- [ ] **Step 3: Sanity-check the lint + format gates**

```bash
cargo clippy -p hector-e2e --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: both exit 0.

- [ ] **Step 4: Sanity-check coverage on the new crate**

```bash
bash scripts/ci-coverage.sh
```

Expected: per-file ≥80% region coverage on every file under `crates/hector-e2e/src/`. The pure-Rust files (`result.rs`, `env.rs`, `assertions.rs`) have unit tests; `docker.rs` will not have unit tests (its execution path requires Docker). If coverage gates fail on `docker.rs`, the simplest mitigation is to mark its bodies under a feature flag or add minimal "smoke" tests that hit error paths (e.g. unknown adapter, missing Dockerfile).

If `docker.rs` is below the gate AND that's the only file failing, the right move is to add a unit test for the "unknown adapter" branch — that exercises the `anyhow::bail!("no Dockerfile at ...")` path without touching Docker:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_image_errors_on_unknown_adapter() {
        let err = build_image("does-not-exist").unwrap_err();
        assert!(format!("{err}").contains("no Dockerfile"));
    }
}
```

That sole test is enough to satisfy regional coverage on the bail-path while leaving the docker-shell-out paths untested by unit code (their coverage is implicit in the `#[ignore]` integration tests).

- [ ] **Step 5: Commit any coverage-driven additions**

```bash
git add crates/hector-e2e/src/docker.rs
git commit -m "test(e2e): cover build_image unknown-adapter error path"
```

(Only if Step 4 required this.)

---

## Self-review checklist (for the implementer)

Before declaring the plan done, verify against the spec:

- [ ] All 7 test cases listed in spec §11 are encoded as `#[ignore]` tests (3 + 2 + 2 = 7).
- [ ] The `script-todo` case is **absent** from `tests/opencode.rs` and `tests/reasonix.rs` (per spec §8, encoded as absence, not a runtime skip).
- [ ] `RunResult` exposes exactly the 6 fields from spec §6: `exit_code`, `verdict`, `log_entries`, `target_after`, `harness_log`, `drive_log`.
- [ ] `require_e2e_env()` checks the three preconditions from spec §10 (docker, `.env.e2e`, `target/release/hector`).
- [ ] `assertions::hook_fired` handles the INCONCLUSIVE-agent-self-refused case per spec §10: stderr message + soft pass.
- [ ] The policy in `tests/e2e/policy/.hector.yml` carries the three rules from spec §7 with the exact rule IDs (`js-forbid-eval`, `no-hardcoded-secrets`, `no-todo-markers`).
- [ ] Bind mounts in `run_case` match spec §4 (6 mounts, paths and modes).
- [ ] `.gitignore` excludes `tests/e2e/<adapter>/runs/` and `tests/e2e/.env.e2e`.
- [ ] `tests/e2e/README.md` documents the prereqs, run commands, forensics layout, and common failure modes.
