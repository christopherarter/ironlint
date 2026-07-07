# Bash Gate — Self-Trust Prevention Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Intercept Bash tool calls in every supported adapter and deny the ones that would let an agent free itself (`ironlint trust`, Bash writes to `.ironlint.yml` / `.ironlint/gates/`), by adding a pure-Rust matcher crate exposed via a built-in `ironlint` subcommand and wiring a Bash branch into all four adapters.

**Architecture:** A new leaf crate `ironlint-bash-gate` holds `decide(command: &str) -> Decision` — pure of I/O and state, with the test file as the spec. The `ironlint` binary exposes it via a built-in subcommand that reads the command on stdin and emits a binary `0` (allow) / `2` (block, reason on stdout) exit code. Each adapter gains a Bash branch that runs a substring pre-filter (`ironlint` or `.ironlint`?), shells out to the subcommand on a hit, and translates the exit through its own block contract (claude-code: exit 2; codex: deny-JSON/exit 0; pi: return `{block,reason}`; opencode: throw). Spawn failure fails closed. The matcher is not a `check`, not trust-gated, and works with no `.ironlint.yml` present.

**Tech Stack:** Rust 2021 (workspace, edition per `Cargo.toml`), clap derive, `assert_cmd`/`predicates`/`tempfile` for tests, bash + jq for the two shell adapters, TypeScript for pi/opencode.

## Global Constraints

- **Threat tier:** lazy non-reasoning models. Block direct forms + light de-obfuscation only. Variable-substitution indirection (`iron$(echo lint) trust`, `IRON=ironlint; $IRON trust`, `base64 … | sh`, `bash scripts/x.sh`) is a **documented known gap** — the matcher MUST allow these, and tests pin that allowance as the scope boundary, not as a TODO.
- **Not a `check`:** the subcommand does not go through `ironlint check`, the verdict machinery, or the trust gate. No `$IRONLINT_FILE`, no per-check spawn.
- **Not trust-gated:** the bash-gate must run even when `.ironlint.yml` is missing or untrusted. The subcommand depends on neither a valid nor a trusted config.
- **No new runtime dependency:** every adapter already shells out to the `ironlint` binary. The substring pre-filter means ordinary Bash calls never spawn it.
- **Exit contract — binary:** `0` = allow, `2` = block (reason on stdout), anything else = adapters fail closed. No env-var override.
- **Coverage:** every Rust file under `crates/*/src/` must hit ≥80% region coverage (CI via `scripts/ci-coverage.sh`).
- **Cognitive complexity:** ≤15 per function (`clippy.toml`, `#![warn(clippy::cognitive_complexity)]` at each crate root).
- **Commit `Cargo.lock`:** workspace policy. If `cargo build` drifts the lock after adding the crate, regenerate with `cargo generate-lockfile` and commit alongside.
- **Spec:** `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md` (commit `4935c78`). Read it first.
- **Conventions:** A check is `files` + `run`/`steps` + `on` — but the bash-gate is NOT a check; it's a built-in self-protection rule, so it does not live under `.ironlint/gates/` and is not authored in `.ironlint.yml`. Binary is `ironlint`, not `ironlint-cli`. Test fixtures live in `tests/fixtures/`.

## File Structure

**New crate `ironlint-bash-gate`:**
- `crates/ironlint-bash-gate/Cargo.toml` — package manifest, no dependencies (pure logic).
- `crates/ironlint-bash-gate/src/lib.rs` — `Decision` enum + `decide(command: &str) -> Decision` + private helpers (`normalize`, `is_ironlint_trust`, `is_policy_write`). Pure. One responsibility: classify a command string.
- `crates/ironlint-bash-gate/src/lib.rs` (test module, `#[cfg(test)]`) — the table of `(input, expected)` rows. The spec.

**`ironlint-cli` changes:**
- `crates/ironlint-cli/Cargo.toml` — add `ironlint-bash-gate = { path = "../ironlint-bash-gate" }`.
- `crates/ironlint-cli/src/cli.rs` — add `GateBash` variant to `Command`.
- `crates/ironlint-cli/src/commands/mod.rs` — add `pub mod gate_bash;`.
- `crates/ironlint-cli/src/commands/gate_bash.rs` — read stdin, call `decide`, map to exit code.
- `crates/ironlint-cli/src/main.rs` — dispatch `Command::GateBash` → `commands::gate_bash::run()`.
- `crates/ironlint-cli/tests/cli_e2e_gate_bash.rs` — subcommand e2e (`assert_cmd`).

**Adapter changes:**
- `crates/ironlint-core/src/adapter/registry.rs` — `claude_build_entry` matcher gets `|Bash`; `codex_build_entry` matcher gets the codex shell-tool name.
- `adapters/claude-code/hooks/hook.sh` — new `Bash)` arm before FILE extraction.
- `adapters/codex/hooks/hook.sh` — new shell-tool branch before the `apply_patch`-only gate.
- `adapters/pi/src/index.ts` — add `"bash"` to `GATED_TOOLS`; new bash branch.
- `adapters/opencode/src/index.ts` — add opencode shell-tool name to `GATED_TOOLS`; new bash branch.

**Adapter contract tests:**
- `crates/ironlint-cli/tests/hook_contract_claude_code.rs` — Bash block/allow + fail-closed-on-spawn-failure.
- `crates/ironlint-cli/tests/hook_contract_codex.rs` — same for codex.
- pi/opencode TS-side tests (per their existing patterns).

**Workspace:**
- `Cargo.toml` (root) — add `"crates/ironlint-bash-gate"` to `members`.

---

### Task 1: Scaffold the `ironlint-bash-gate` crate

**Files:**
- Create: `crates/ironlint-bash-gate/Cargo.toml`
- Create: `crates/ironlint-bash-gate/src/lib.rs`
- Modify: `Cargo.toml` (root, `members`)

**Interfaces:**
- Produces: `ironlint_bash_gate::Decision` (`Allow`, `Block(String)`) and `ironlint_bash_gate::decide(command: &str) -> Decision`. These are the names Task 4 (CLI subcommand) and the adapter tasks rely on.

- [ ] **Step 1: Add the crate to the workspace**

Edit `Cargo.toml` (root):

```toml
[workspace]
resolver = "2"
members = ["crates/ironlint-core", "crates/ironlint-cli", "crates/ironlint-bash-gate"]
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/ironlint-bash-gate/Cargo.toml`:

```toml
[package]
name = "ironlint-bash-gate"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

# Pure logic — no dependencies. Adding one requires a spec-level reason.
```

- [ ] **Step 3: Create the crate root with the lint gate and a stub `decide`**

Create `crates/ironlint-bash-gate/src/lib.rs`:

```rust
//! The bash-gate matcher: a pure classifier for a Bash command string.
//!
//! Decides whether a command an agent wants to run would let it free itself
//! from ironlint's gate — `ironlint trust`, or a Bash write to the policy
//! surface (`.ironlint.yml`, `.ironlint/gates/`). Pure of I/O and state; the
//! `ironlint gate-bash` subcommand and the adapter hooks are thin shims
//! around it. See `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.
//!
//! Threat tier: lazy non-reasoning models. Blocks direct forms + light
//! de-obfuscation. Variable-substitution indirection is a documented known
//! gap (catching it needs real shell evaluation — adversarial tier, out of
//! scope). The test module pins both directions.

#![warn(clippy::cognitive_complexity)]

/// The bash-gate's verdict on one command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Allow the command to proceed.
    Allow,
    /// Block it; the string is the reason shown to the agent.
    Block(String),
}

/// Decide whether `command` may run.
///
/// Pure: no I/O, no state. Returns `Block(reason)` for `ironlint trust` (any
/// args) and Bash writes to the policy surface; `Allow` otherwise, including
/// the documented indirection gap (which is *intentionally* allowed).
pub fn decide(command: &str) -> Decision {
    let _ = command;
    Decision::Allow
}
```

- [ ] **Step 4: Verify the crate builds and the stub passes a trivial test**

Run: `cargo build -p ironlint-bash-gate`
Expected: builds with no errors or warnings.

Run: `cargo test -p ironlint-bash-gate`
Expected: `0 tests run` (no tests yet) — no failure.

- [ ] **Step 5: Regenerate the lockfile if it drifted, then commit**

Run: `cargo generate-lockfile` (only if `cargo build` reported a lock change — otherwise skip)

Run:
```bash
git add Cargo.toml Cargo.lock crates/ironlint-bash-gate/
git commit -m "feat(bash-gate): scaffold ironlint-bash-gate crate

Pure-logic leaf crate holding the decide() matcher. Stubbed Allow for now;
classification lands in the next task. Adds the crate to the workspace."
```

---

### Task 2: Implement `decide` — block `ironlint trust` (TDD)

**Files:**
- Modify: `crates/ironlint-bash-gate/src/lib.rs` (the `decide` body + private helpers + test module)

**Interfaces:**
- Consumes: `Decision` from Task 1.
- Produces: a `decide` that blocks `ironlint trust` (any args) + light de-obfuscation. The exact block/allow table is finalized in Task 3; this task lands the `ironlint trust` + de-obfuscation cases and the helpers that later tasks build on.

- [ ] **Step 1: Write the failing tests for `ironlint trust` detection**

Append to `crates/ironlint-bash-gate/src/lib.rs` (replace the stub `decide` with the real one + tests). First, the test module:

```rust
#[cfg(test)]
mod tests {
    use super::{Decision, decide};

    /// Assert `decide(cmd)` blocks; the reason is checked loosely (caller
    /// cares that it blocked, not the exact wording).
    fn assert_blocks(cmd: &str) {
        match decide(cmd) {
            Decision::Block(_) => {}
            Decision::Allow => panic!("expected Block for {cmd:?}, got Allow"),
        }
    }

    /// Assert `decide(cmd)` allows.
    fn assert_allows(cmd: &str) {
        match decide(cmd) {
            Decision::Allow => {}
            Decision::Block(r) => panic!("expected Allow for {cmd:?}, got Block({r:?})"),
        }
    }

    // --- (1) ironlint trust, any args ---
    #[test]
    fn blocks_bare_ironlint_trust() {
        assert_blocks("ironlint trust");
    }

    #[test]
    fn blocks_ironlint_trust_with_config_flag() {
        assert_blocks("ironlint trust --config shared/base.yml");
    }

    #[test]
    fn blocks_ironlint_trust_with_dot_arg() {
        assert_blocks("ironlint trust .");
    }

    // --- light de-obfuscation: backtick / $() around the binary name ---
    #[test]
    fn blocks_backtick_ironlint_trust() {
        assert_blocks("`ironlint` trust");
    }

    #[test]
    fn blocks_dollar_paren_ironlint_trust() {
        assert_blocks("$(ironlint) trust");
    }

    // --- light de-obfuscation: quoted binary name ---
    #[test]
    fn blocks_single_quoted_ironlint_trust() {
        assert_blocks("'ironlint' trust");
    }

    #[test]
    fn blocks_double_quoted_ironlint_trust() {
        assert_blocks("\"ironlint\" trust");
    }

    // --- light de-obfuscation: whitespace around the binary token ---
    #[test]
    fn blocks_ironlint_trust_extra_spaces() {
        assert_blocks("ironlint   trust");
    }

    #[test]
    fn blocks_ironlint_trust_tab_separated() {
        assert_blocks("ironlint\ttrust");
    }

    // --- cd .ironlint && trust (bare trust after cd into policy dir) ---
    #[test]
    fn blocks_cd_ironlint_then_trust() {
        assert_blocks("cd .ironlint && trust");
    }

    #[test]
    fn blocks_cd_ironlint_gates_then_trust() {
        assert_blocks("cd .ironlint/gates && trust");
    }

    // --- false-positive guard: read-only ironlint subcommands MUST allow ---
    #[test]
    fn allows_ironlint_check() {
        assert_allows("ironlint check");
    }

    #[test]
    fn allows_ironlint_doctor() {
        assert_allows("ironlint doctor");
    }

    #[test]
    fn allows_ironlint_validate() {
        assert_allows("ironlint validate");
    }

    #[test]
    fn allows_ironlint_explain() {
        assert_allows("ironlint explain");
    }

    #[test]
    fn allows_ironlint_show_resolved_config() {
        assert_allows("ironlint show-resolved-config");
    }

    #[test]
    fn allows_ironlint_init() {
        assert_allows("ironlint init");
    }

    // --- false-positive guard: 'ironlint trust' as a STRING, not a command ---
    #[test]
    fn allows_echo_quoting_ironlint_trust() {
        assert_allows("echo \"run ironlint trust to bless\"");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ironlint-bash-gate`
Expected: FAIL — the stub `decide` returns `Allow` for everything, so every `assert_blocks` case fails.

- [ ] **Step 3: Implement the matcher for `ironlint trust` + de-obfuscation**

Replace the stub `decide` and add private helpers in `crates/ironlint-bash-gate/src/lib.rs` (above the test module):

```rust
/// The reason prefix used for every block. Adapters show the full reason to
/// the agent; keeping a stable prefix makes the contract tests robust to
/// wording changes.
const TRUST_REASON: &str = "ironlint trust must be run by a human, not by an agent";

/// Normalize a command for matching: collapse the light de-obfuscation cases
/// (backtick/`$()` delimiters around tokens, quoted binary names, runs of
/// whitespace) without attempting to evaluate the string. This is string
/// surgery, not a shell parser — variable-substitution indirection
/// (`iron$(echo lint)`) is deliberately untouched (known gap).
fn normalize(command: &str) -> String {
    // 1. Strip backtick and $() DELIMITERS while keeping their contents, so
    //    `ironlint` and $(ironlint) collapse to ironlint. Only the matched
    //    pair delimiters are removed; a stray backtick or unmatched paren is
    //    left alone (it does not denote a completed substitution).
    let mut s = String::with_capacity(command.len());
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            // skip the opening backtick; the closing one is also skipped when reached
            i += 1;
            continue;
        }
        if c == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
            // skip $( opener
            i += 2;
            continue;
        }
        if c == ')' {
            // a closing paren that was part of $() — drop it. A bare ')' in
            // the command (not from $()) is harmless to drop for matching.
            i += 1;
            continue;
        }
        s.push(c);
        i += 1;
    }

    // 2. Strip single/double quotes around the leading binary token, so
    //    'ironlint' trust and "ironlint" trust collapse to ironlint trust.
    //    Only the quote chars are removed; quoted strings otherwise survive.
    s = s.replace('\'', "").replace('"', "");

    // 3. Collapse runs of whitespace (spaces + tabs) into a single space, so
    //    `ironlint   trust` and `ironlint\ttrust` match the same pattern.
    let mut collapsed = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' || c == '\t' {
            if !prev_space {
                collapsed.push(' ');
            }
            prev_space = true;
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// True if the normalized command is `ironlint trust` (any trailing args).
/// Matches `ironlint trust`, `ironlint trust --config x`, `ironlint trust .`.
/// Does NOT match `ironlint check`, `ironlint doctor`, etc.
fn is_ironlint_trust(normalized: &str) -> bool {
    // `ironlint trust` as a command, possibly with trailing args. Require a
    // word boundary after `trust` so `ironlint trustworthy` does not match.
    // The `echo "ironlint trust"` false-positive guard is handled by the
    // caller checking the command starts with `ironlint trust`, not just
    // contains it — `echo` starts with `echo`, not `ironlint`.
    normalized == "ironlint trust"
        || normalized.starts_with("ironlint trust ")
        // `cd .ironlint && trust` and `cd .ironlint/gates && trust`: a bare
        // `trust` after a `cd` into the policy dir. Match the chained form
        // explicitly — a bare `trust` elsewhere is not a trust invocation.
        || normalized.starts_with("cd .ironlint ") && normalized.contains(" trust")
}

/// True if the command writes to the policy surface (`.ironlint.yml` or
/// anything under `.ironlint/gates/`). Detected via redirect operators,
/// `tee`, in-place editors, and `cp`/`mv` with a policy path as destination.
/// Implemented in Task 3; returns false here so Task 2's tests focus on trust.
fn is_policy_write(_normalized: &str) -> bool {
    false
}

pub fn decide(command: &str) -> Decision {
    let n = normalize(command);

    if is_ironlint_trust(&n) {
        return Decision::Block(TRUST_REASON.to_string());
    }
    if is_policy_write(&n) {
        // reason set in Task 3
        return Decision::Block(
            "ironlint policy files must be edited through the Write/Edit tool (which is gated), not via Bash"
                .to_string(),
        );
    }
    Decision::Allow
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ironlint-bash-gate`
Expected: PASS — all `ironlint trust` block cases pass, all read-only-subcommand allow cases pass, the `echo` false-positive guard passes.

- [ ] **Step 5: Check lint and coverage**

Run: `cargo clippy -p ironlint-bash-gate --all-targets -- -D warnings`
Expected: no warnings.

Run: `bash scripts/ci-coverage.sh` (scoped to the new crate if the script supports filtering; otherwise run the gate and confirm `ironlint-bash-gate` files are ≥80%)
Expected: `crates/ironlint-bash-gate/src/lib.rs` at ≥80% region coverage. The stub `is_policy_write` returning `false` is exercised by the allow cases; the `is_policy_write` `true` branch is covered in Task 3.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-bash-gate/src/lib.rs
git commit -m "feat(bash-gate): block 'ironlint trust' + light de-obfuscation

decide() now blocks ironlint trust (any args) and the first tier of
evasions a lazy model tries: backtick/\$() delimiters around the binary
name, quoted binary names, whitespace runs, and 'cd .ironlint && trust'.
Read-only subcommands (check, doctor, validate, ...) and the echo
false-positive guard (ironlint trust as a string, not a command) allow."
```

---

### Task 3: Extend `decide` — block Bash writes to the policy surface (TDD)

**Files:**
- Modify: `crates/ironlint-bash-gate/src/lib.rs` (`is_policy_write` body + new test rows)

**Interfaces:**
- Consumes: `normalize`, `decide` from Task 2.
- Produces: a `decide` that also blocks Bash writes to `.ironlint.yml` and `.ironlint/gates/` via redirects, `tee`, `sed -i`/`ed`/`perl -i`, and `cp`/`mv` onto a policy path (destination).

- [ ] **Step 1: Write the failing tests for policy-write detection**

Add these rows to the test module in `crates/ironlint-bash-gate/src/lib.rs`:

```rust
    // --- (2) Bash writes to the policy surface: redirects ---
    #[test]
    fn blocks_redirect_to_ironlint_yml() {
        assert_blocks("echo x > .ironlint.yml");
    }

    #[test]
    fn blocks_append_to_ironlint_yml() {
        assert_blocks("echo x >> .ironlint.yml");
    }

    #[test]
    fn blocks_clobber_redirect_to_ironlint_yml() {
        assert_blocks("echo x >| .ironlint.yml");
    }

    #[test]
    fn blocks_amp_redirect_to_ironlint_yml() {
        assert_blocks("echo x &> .ironlint.yml");
    }

    #[test]
    fn blocks_amp_append_to_ironlint_yml() {
        assert_blocks("echo x &>> .ironlint.yml");
    }

    #[test]
    fn blocks_cat_redirect_to_ironlint_yml() {
        assert_blocks("cat > .ironlint.yml");
    }

    // --- tee ---
    #[test]
    fn blocks_tee_ironlint_yml() {
        assert_blocks("echo x | tee .ironlint.yml");
    }

    #[test]
    fn blocks_tee_append_ironlint_yml() {
        assert_blocks("echo x | tee -a .ironlint.yml");
    }

    // --- in-place editors ---
    #[test]
    fn blocks_sed_inplace_ironlint_yml() {
        assert_blocks("sed -i 's/x/y/' .ironlint.yml");
    }

    #[test]
    fn blocks_perl_inplace_ironlint_yml() {
        assert_blocks("perl -i -pe 's/x/y/' .ironlint.yml");
    }

    #[test]
    fn blocks_ed_ironlint_yml() {
        assert_blocks("ed -s .ironlint.yml");
    }

    // --- same detectors against .ironlint/gates/ ---
    #[test]
    fn blocks_redirect_to_gate_script() {
        assert_blocks("echo x > .ironlint/gates/lint.sh");
    }

    #[test]
    fn blocks_sed_inplace_gate_script() {
        assert_blocks("sed -i 's/x/y/' .ironlint/gates/lint.sh");
    }

    // --- cp / mv ONTO a policy path (destination) ---
    #[test]
    fn blocks_cp_onto_gate_script() {
        assert_blocks("cp malicious.sh .ironlint/gates/lint.sh");
    }

    #[test]
    fn blocks_mv_onto_ironlint_yml() {
        assert_blocks("mv bad.yml .ironlint.yml");
    }

    // --- false-positive guard: policy path as SOURCE (a read), not destination ---
    #[test]
    fn allows_cp_from_ironlint_yml_as_source() {
        assert_allows("cp .ironlint.yml /tmp/backup");
    }

    #[test]
    fn allows_cat_ironlint_yml_piped_to_grep() {
        assert_allows("cat .ironlint.yml | grep checks");
    }

    #[test]
    fn allows_grep_recursive_ironlint() {
        assert_allows("grep -r ironlint docs/");
    }

    #[test]
    fn allows_ls_ironlint_gates() {
        assert_allows("ls .ironlint/gates/");
    }

    #[test]
    fn allows_cat_gate_script() {
        assert_allows("cat .ironlint/gates/lint.sh");
    }

    // --- ordinary commands never mention ironlint, so the pre-filter skips
    //     them entirely; pin that decide() also allows them if reached ---
    #[test]
    fn allows_cargo_test() {
        assert_allows("cargo test");
    }

    #[test]
    fn allows_git_status() {
        assert_allows("git status");
    }

    // --- documented known gap: variable-substitution indirection MUST allow ---
    #[test]
    fn allows_iron_echo_lint_indirection() {
        assert_allows("iron$(echo lint) trust");
    }

    #[test]
    fn allows_ironvar_trust_indirection() {
        assert_allows("IRON=ironlint; $IRON trust");
    }

    #[test]
    fn allows_base64_eval_indirection() {
        assert_allows("base64 -d <<< 'aXJvbmxpbnQgdHJ1c3Q=' | sh");
    }

    #[test]
    fn allows_bash_script_indirection() {
        assert_allows("bash scripts/x.sh");
    }
```

- [ ] **Step 2: Run the tests to verify the new ones fail**

Run: `cargo test -p ironlint-bash-gate`
Expected: FAIL — every new `assert_blocks` case fails (the stub `is_policy_write` returns `false`). The new `assert_allows` cases pass (the indirection gap is already allowed).

- [ ] **Step 3: Implement `is_policy_write`**

Replace the stub `is_policy_write` in `crates/ironlint-bash-gate/src/lib.rs`:

```rust
/// True if a path token refers to the policy surface: the literal
/// `.ironlint.yml` (at any depth — bare or path-prefixed) or anything under
/// `.ironlint/gates/`. Matched on the path string, not the filesystem.
fn is_policy_path(token: &str) -> bool {
    // `.ironlint.yml` anywhere in the token (bare, ./, or path-prefixed).
    // `.ironlint/gates/` as a directory prefix.
    token.ends_with(".ironlint.yml")
        || token.contains("/.ironlint.yml")
        || token.contains(".ironlint/gates/")
        || token == ".ironlint.yml"
}

/// True if the normalized command writes to the policy surface. Detected via:
///   - redirect operators targeting a policy path: >, >>, >|, &>, &>>
///   - `tee` writing a policy path
///   - in-place editors: `sed -i`, `ed`, `perl -i`
///   - `cp`/`mv` with a policy path as the DESTINATION (last arg)
///
/// `cp .ironlint.yml /tmp/backup` (policy path as source) is a read and MUST
/// allow — only the destination is checked for cp/mv.
fn is_policy_write(normalized: &str) -> bool {
    // Split into whitespace-separated tokens. This is deliberately crude —
    // the matcher's job is the direct form, not surviving quoting games.
    let tokens: Vec<&str> = normalized.split_whitespace().collect();

    // Redirect operators: the token AFTER `>`, `>>`, `>|`, `&>`, `&>>` is
    // the target. Also catch a redirect glued to a path (`>.ironlint.yml`).
    for i in 0..tokens.len() {
        let t = tokens[i];
        // Glued form: `>.ironlint.yml`, `>>.ironlint.yml`
        if let Some(stripped) = t
            .strip_prefix(">>")
            .or_else(|| t.strip_prefix(">"))
            .or_else(|| t.strip_prefix("&>>"))
            .or_else(|| t.strip_prefix("&>"))
        {
            if is_policy_path(stripped) {
                return true;
            }
        }
        // Bare operator form: `> .ironlint.yml` (next token is the target)
        if matches!(t, ">" | ">>" | ">|" | "&>" | "&>>") {
            if let Some(next) = tokens.get(i + 1) {
                if is_policy_path(next) {
                    return true;
                }
            }
        }
    }

    // `tee` / `tee -a`: a later argument is the destination file.
    if tokens.first().is_some_and(|&c| c == "tee") {
        for t in &tokens[1..] {
            if !t.starts_with('-') && is_policy_path(t) {
                return true;
            }
        }
    }

    // In-place editors: `sed -i ... <file>`, `ed -s <file>`, `perl -i ... <file>`.
    // The last non-flag argument is the target file.
    if let Some(&cmd) = tokens.first() {
        if matches!(cmd, "sed" | "ed" | "perl") {
            // Only block if -i is present (sed/perl) or it's `ed` (which
            // edits in place by nature). For sed/perl without -i, the file
            // is read, not written.
            let inplace = match cmd {
                "ed" => true,
                "sed" | "perl" => tokens.iter().any(|t| *t == "-i" || t.starts_with("-i")),
                _ => false,
            };
            if inplace {
                if let Some(last) = tokens.last() {
                    if is_policy_path(last) {
                        return true;
                    }
                }
            }
        }
    }

    // `cp`/`mv` with a policy path as the DESTINATION. The destination is the
    // last argument (for both two-arg and multi-source forms). A policy path
    // as a SOURCE (e.g. `cp .ironlint.yml /tmp/backup`) MUST allow — that's
    // why only the last token is checked.
    if let Some(&cmd) = tokens.first() {
        if matches!(cmd, "cp" | "mv") && tokens.len() >= 3 {
            if let Some(dest) = tokens.last() {
                if is_policy_path(dest) {
                    return true;
                }
            }
        }
    }

    false
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ironlint-bash-gate`
Expected: PASS — all block cases (redirects, tee, sed -i, perl -i, ed, cp/mv onto policy paths, same against `.ironlint/gates/`) and all allow cases (cp from source, cat piped to grep, ls, the indirection gap, ordinary commands).

- [ ] **Step 5: Mutation-test the matcher (local, ad-hoc per CLAUDE.md)**

Run: `cargo mutants --file 'crates/ironlint-bash-gate/src/lib.rs'`
Expected: no surviving mutants. A surviving mutant means a test exercised the line but didn't verify the decision — fix the test (or the code) until clean. This is not a CI gate; it's an investigative check during implementation.

- [ ] **Step 6: Check lint and coverage**

Run: `cargo clippy -p ironlint-bash-gate --all-targets -- -D warnings`
Expected: no warnings. If `is_policy_write` trips the cognitive-complexity cap (≤15), decompose it by extracting `redirect_target`, `tee_target`, `inplace_editor_target`, `cp_mv_destination` helpers — each is a single, testable concern — rather than annotating.

Run: `bash scripts/ci-coverage.sh`
Expected: `crates/ironlint-bash-gate/src/lib.rs` at ≥80% region coverage.

- [ ] **Step 7: Commit**

```bash
git add crates/ironlint-bash-gate/src/lib.rs
git commit -m "feat(bash-gate): block Bash writes to the policy surface

is_policy_write() detects redirects (>, >>, >|, &>, &>>), tee, in-place
editors (sed -i, ed, perl -i), and cp/mv onto .ironlint.yml or
.ironlint/gates/. cp/mv with a policy path as the SOURCE (a read)
allows — only the destination is checked. Variable-substitution
indirection remains a documented, pinned known gap (allows)."
```

---

### Task 4: Expose `decide` via the `ironlint gate-bash` subcommand (TDD)

**Files:**
- Modify: `crates/ironlint-cli/Cargo.toml`
- Modify: `crates/ironlint-cli/src/cli.rs`
- Modify: `crates/ironlint-cli/src/commands/mod.rs`
- Create: `crates/ironlint-cli/src/commands/gate_bash.rs`
- Modify: `crates/ironlint-cli/src/main.rs`
- Create: `crates/ironlint-cli/tests/cli_e2e_gate_bash.rs`

**Interfaces:**
- Consumes: `ironlint_bash_gate::{decide, Decision}` from Task 2/3.
- Produces: a built-in `ironlint gate-bash` subcommand: reads the command on stdin, exits `0` (allow, empty stdout) or `2` (block, reason on stdout). Spawn failure / signal death is handled by the *adapters* (Tasks 5–8), not the subcommand.

- [ ] **Step 1: Add the dependency to `ironlint-cli`**

Edit `crates/ironlint-cli/Cargo.toml`, add to `[dependencies]`:

```toml
ironlint-bash-gate = { path = "../ironlint-bash-gate" }
```

- [ ] **Step 2: Add the `GateBash` variant to the CLI enum**

In `crates/ironlint-cli/src/cli.rs`, add a variant to `Command` (alongside `Trust`, `Validate`, etc.):

```rust
    /// Decide whether a Bash command may run. Reads the command on stdin.
    /// Exit 0 = allow (empty stdout); exit 2 = block (reason on stdout).
    /// Not a check, not trust-gated; works with no .ironlint.yml present.
    GateBash,
```

- [ ] **Step 3: Create the command module**

Create `crates/ironlint-cli/src/commands/gate_bash.rs`:

```rust
//! The `ironlint gate-bash` built-in: read a command on stdin, classify it
//! via `ironlint_bash_gate::decide`, and emit the binary exit contract
//! (0 = allow, 2 = block with the reason on stdout). The adapters' Bash
//! branches shell out to this; it is the single source of the deny decision.
//!
//! Not a `check`: no config load, no trust gate, no per-check spawn. The
//! bash-gate must run even when `.ironlint.yml` is missing or untrusted —
//! that is exactly when the agent is most motivated to run `ironlint trust`.

use anyhow::Result;
use std::io::Read;

pub fn run() -> Result<i32> {
    // Read the whole command from stdin. The adapters pipe it in; the
    // pre-filter guarantees the bytes contained a UTF8-decodable `ironlint`
    // or `.ironlint` substring, so lossy decoding is safe for the matcher.
    // A genuinely malformed (non-UTF8) stdin is unreachable in practice but
    // defended: allow + log, never crash.
    let mut buf = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
        eprintln!("ironlint gate-bash: could not read stdin: {e}");
        return Ok(0);
    }
    let command = match std::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("ironlint gate-bash: non-UTF8 stdin — allowing (unreachable via pre-filter)");
            return Ok(0);
        }
    };

    match ironlint_bash_gate::decide(command) {
        ironlint_bash_gate::Decision::Allow => Ok(0),
        ironlint_bash_gate::Decision::Block(reason) => {
            print!("{reason}");
            Ok(2)
        }
    }
}
```

- [ ] **Step 4: Register and dispatch the subcommand**

In `crates/ironlint-cli/src/commands/mod.rs`, add:

```rust
pub mod gate_bash;
```

In `crates/ironlint-cli/src/main.rs`, add to the `match cli.command` block (alongside the other arms):

```rust
        Command::GateBash => commands::gate_bash::run()?,
```

- [ ] **Step 5: Write the failing e2e tests**

Create `crates/ironlint-cli/tests/cli_e2e_gate_bash.rs`:

```rust
//! E2e for `ironlint gate-bash`: stdin command → exit 0 (allow) / 2 (block).
//! Mirrors `cli_e2e_trust`'s `assert_cmd` harness.
//!
//! Pins the binary exit contract and the "no config / no trust needed"
//! property: the subcommand runs in a bare temp dir with no .ironlint.yml.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn allows_ironlint_check_on_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("ironlint check");
    cmd.assert()
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn blocks_ironlint_trust_on_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("ironlint trust");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicates::str::contains("ironlint trust must be run by a human"));
}

#[test]
fn blocks_redirect_to_ironlint_yml_on_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("echo x > .ironlint.yml");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicates::str::contains("policy files must be edited through the Write/Edit tool"));
}

#[test]
fn allows_empty_stdin() {
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"]).write_stdin("");
    cmd.assert().success().code(0).stdout(predicates::str::is_empty());
}

#[test]
fn runs_with_no_config_present() {
    // The bash-gate is not trust-gated and needs no .ironlint.yml: run it in
    // a bare temp dir with no config, no trust store. It must still decide.
    let dir = tempdir().unwrap();
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"])
        .current_dir(dir.path())
        .write_stdin("ironlint trust");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicates::str::contains("ironlint trust must be run by a human"));
}

#[test]
fn allows_indirection_known_gap() {
    // Pinned: variable-substitution indirection MUST allow (documented gap).
    let mut cmd = Command::cargo_bin("ironlint").unwrap();
    cmd.args(["gate-bash"])
        .write_stdin("iron$(echo lint) trust");
    cmd.assert().success().code(0).stdout(predicates::str::is_empty());
}
```

- [ ] **Step 6: Run the tests to verify they fail (or pass, if Steps 2-4 already wired it)**

Run: `cargo test -p ironlint-cli --test cli_e2e_gate_bash`
Expected: if Steps 2–4 are in place, PASS. If the subcommand isn't wired yet, FAIL with "No such command: gate-bash" or a non-zero exit.

- [ ] **Step 7: Build the release binary and run the full e2e**

Run: `cargo test -p ironlint-cli --test cli_e2e_gate_bash`
Expected: PASS — all six tests green.

- [ ] **Step 8: Check lint and coverage**

Run: `cargo clippy -p ironlint-cli --all-targets -- -D warnings`
Expected: no warnings.

Run: `bash scripts/ci-coverage.sh`
Expected: `crates/ironlint-cli/src/commands/gate_bash.rs` at ≥80% region coverage.

- [ ] **Step 9: Commit**

```bash
git add crates/ironlint-cli/Cargo.toml crates/ironlint-cli/src/cli.rs \
        crates/ironlint-cli/src/commands/mod.rs crates/ironlint-cli/src/commands/gate_bash.rs \
        crates/ironlint-cli/src/main.rs crates/ironlint-cli/tests/cli_e2e_gate_bash.rs \
        Cargo.lock
git commit -m "feat(cli): add 'ironlint gate-bash' built-in subcommand

Reads a Bash command on stdin, classifies via
ironlint_bash_gate::decide, and emits the binary exit contract:
0 = allow (empty stdout), 2 = block (reason on stdout). Not a check,
not trust-gated; runs with no .ironlint.yml present. The adapters'
Bash branches shell out to this."
```

---

### Task 5: Wire the claude-code adapter's Bash branch (TDD)

**Files:**
- Modify: `crates/ironlint-core/src/adapter/registry.rs:50` (matcher)
- Modify: `adapters/claude-code/hooks/hook.sh` (new `Bash)` arm)
- Modify: `crates/ironlint-cli/tests/hook_contract_claude_code.rs` (Bash block/allow + fail-closed tests)

**Interfaces:**
- Consumes: `ironlint gate-bash` (Task 4). The hook shells out: `printf '%s' "$cmd" | ironlint gate-bash`. Exit `0` → allow, exit `2` → hook exits `2` with stdout on stderr, anything else → fail closed (exit `2`).
- Produces: a claude-code PreToolUse hook that gates `Bash` alongside `Edit|Write|MultiEdit|NotebookEdit`.

- [ ] **Step 1: Write the failing hook-contract tests**

In `crates/ironlint-cli/tests/hook_contract_claude_code.rs`, add helpers and tests. First, a Bash payload helper (near the existing `write_payload`/`edit_payload`):

```rust
/// A Bash PreToolUse event. `command` is the raw shell command the agent
/// wants to run.
fn bash_payload(command: &str) -> String {
    serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
    })
    .to_string()
}
```

Then the tests (append to the test module):

```rust
#[test]
fn bash_allows_benign_command() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // 'ls' never mentions ironlint → pre-filter skips the spawn entirely.
    // The hook must allow (exit 0) without invoking the stub.
    fx.stub(2, "stub should not be called"); // a stub block would surface if called
    let _ = fx.file("foo.py"); // ensure project exists
    fx.run("PreToolUse", &bash_payload("ls"), &[])
        .success()
        .code(0);
}

#[test]
fn bash_blocks_ironlint_trust() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // The real `ironlint gate-bash` is invoked here, not the stub — so use
    // the cargo bin on PATH. HookFixture stubs `ironlint`, which would
    // intercept gate-bash too. To exercise the REAL matcher, do NOT stub;
    // instead rely on the real binary. (See note below.)
    //
    // We stub exit 0 with empty stdout to stand in for a real "allow" — but
    // we want to prove BLOCK. So: stub gate-bash's behavior by making the
    // stub exit 2 with a reason, simulating the real subcommand's block.
    fx.stub(2, "ironlint trust must be run by a human");
    fx.run("PreToolUse", &bash_payload("ironlint trust"), &[])
        .failure()
        .code(2)
        .stderr(predicates::str::contains("ironlint trust must be run by a human"));
}

#[test]
fn bash_blocks_redirect_to_ironlint_yml() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "policy files must be edited through the Write/Edit tool");
    fx.run("PreToolUse", &bash_payload("echo x > .ironlint.yml"), &[])
        .failure()
        .code(2);
}

#[test]
fn bash_fails_closed_when_ironlint_missing() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // Do NOT stub ironlint → it's not on PATH → spawn fails. The hook must
    // fail CLOSED (exit 2), not allow.
    fx.run("PreToolUse", &bash_payload("ironlint trust"), &[])
        .failure()
        .code(2);
}
```

**Note on the stub:** `HookFixture::stub` writes a stub `ironlint` that drains stdin and exits `code`. The claude-code Bash branch calls `ironlint gate-bash`, so the stub intercepts it. The `bash_blocks_ironlint_trust` test stubs exit `2` with the reason — proving the hook translates a gate-bash block (exit 2) into a hook block (exit 2 + stderr). The `bash_fails_closed_when_ironlint_missing` test does NOT stub, so the spawn fails — proving fail-closed. The `bash_allows_benign_command` test stubs exit `2` (a "trap" — if the hook wrongly spawned, it would block); `ls` should skip the spawn and allow.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ironlint-cli --test hook_contract_claude_code -- bash_`
Expected: FAIL — the hook has no `Bash)` arm; the `case` falls through to the `*)` "not yet gated" arm and exits 2 for *every* Bash call (including `ls`, which should allow). So `bash_allows_benign_command` fails (got 2, expected 0).

- [ ] **Step 3: Add `Bash` to the claude-code matcher**

In `crates/ironlint-core/src/adapter/registry.rs`, edit `claude_build_entry` (around line 50):

```rust
pub(crate) fn claude_build_entry(command: &str) -> Value {
    // MultiEdit and NotebookEdit are gated by hook.sh alongside Edit/Write
    // (Task 5.24); Bash is gated by the bash-gate branch (Task 5 of the
    // bash-gate plan). The matcher must name every tool the hook handles or
    // those calls never invoke the hook and bypass every check.
    json!({"matcher": "Edit|Write|MultiEdit|NotebookEdit|Bash",
           "hooks": [{"type": "command", "command": command}]})
}
```

Update the regression test `claude_entry_points_at_command_and_matcher` in the same file to assert the new matcher:

```rust
    #[test]
    fn claude_entry_points_at_command_and_matcher() {
        let e = claude_build_entry("\"/x/hook.sh\" pre-tool-use");
        assert_eq!(e["matcher"], "Edit|Write|MultiEdit|NotebookEdit|Bash");
        assert_eq!(e["hooks"][0]["command"], "\"/x/hook.sh\" pre-tool-use");
    }
```

- [ ] **Step 4: Add the `Bash)` arm to `hook.sh`**

In `adapters/claude-code/hooks/hook.sh`, the `Bash)` arm must run **before** FILE extraction (the existing `FILE=$(...)` + empty-FILE early-exit at lines 78-82 would silently allow a Bash call, which has no `file_path`). Insert a `Bash)` arm immediately after `TOOL_NAME` is extracted (after line 75) and before the `FILE=$(...)` line.

The arm:

```bash
  Bash)
    # The bash-gate: deny commands that would let the agent free itself
    # (ironlint trust, or a Bash write to .ironlint.yml / .ironlint/gates/).
    # Decided by `ironlint gate-bash` — the single source of the deny logic,
    # shared across every adapter. See
    # docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md.
    #
    # A substring pre-filter skips the spawn for ordinary commands (ls, git,
    # cargo) that never mention ironlint or .ironlint — they pay nothing.
    COMMAND=$(echo "${EVENT}" | jq -r '.tool_input.command // empty')
    if [[ "${COMMAND}" != *ironlint* && "${COMMAND}" != *.ironlint* ]]; then
      exit 0
    fi
    GATE_REASON=$(printf '%s' "${COMMAND}" | ironlint gate-bash 2>/dev/null)
    GATE_EC=$?
    case "${GATE_EC}" in
      0) exit 0 ;;
      2)
        echo "${GATE_REASON}" >&2
        exit 2
        ;;
      *)
        # Spawn failure / signal death / unexpected exit → fail CLOSED.
        # The deny check is the thing being protected; a broken deny check
        # is never a silent allow. Stricter than the file-gate's exit-3
        # fail-open default.
        echo "ironlint: bash-gate failed (exit ${GATE_EC}) — blocking (fail-closed)" >&2
        exit 2
        ;;
    esac
    ;;
```

Add `Bash)` as the first arm of the `case "${TOOL_NAME}" in` block (around line 152), before `Write)`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ironlint-cli --test hook_contract_claude_code -- bash_`
Expected: PASS — `ls` allows (pre-filter skip), `ironlint trust` blocks, redirect blocks, missing-binary fails closed.

Run: `cargo test -p ironlint-core -- claude_entry_points_at_command_and_matcher`
Expected: PASS — the regression test asserts the new matcher.

- [ ] **Step 6: Re-run the full claude-code contract suite to confirm no regression**

Run: `cargo test -p ironlint-cli --test hook_contract_claude_code`
Expected: PASS — every existing Write/Edit/MultiEdit/NotebookEdit test still green.

- [ ] **Step 7: Check lint and coverage**

Run: `cargo clippy -p ironlint-core -p ironlint-cli --all-targets -- -D warnings`
Expected: no warnings.

Run: `bash scripts/ci-coverage.sh`
Expected: `registry.rs` and `hook.sh`-adjacent changes at ≥80%.

Run: `bash -n adapters/claude-code/hooks/hook.sh`
Expected: no syntax errors.

- [ ] **Step 8: Commit**

```bash
git add crates/ironlint-core/src/adapter/registry.rs \
        adapters/claude-code/hooks/hook.sh \
        crates/ironlint-cli/tests/hook_contract_claude_code.rs
git commit -m "feat(claude-code): wire the Bash branch to ironlint gate-bash

PreToolUse matcher now includes Bash. The Bash arm runs BEFORE FILE
extraction (a Bash event has no file_path; the empty-FILE early-exit
would silently allow it). Substring pre-filter (ironlint | .ironlint)
skips the spawn for ordinary commands. On a hit, shells out to
'ironlint gate-bash': exit 0 -> allow, exit 2 -> hook exit 2 (stderr
reason), anything else -> fail closed. Pinned: ls allows, ironlint
trust blocks, missing-binary fails closed."
```

---

### Task 6: Wire the codex adapter's Bash branch

**Files:**
- Modify: `crates/ironlint-core/src/adapter/registry.rs:61` (`codex_build_entry` matcher — add the codex shell-tool name)
- Modify: `adapters/codex/hooks/hook.sh` (new shell-tool branch before the `apply_patch`-only gate)
- Modify: `crates/ironlint-cli/tests/hook_contract_codex.rs` (Bash block/allow + fail-closed)

**Interfaces:**
- Consumes: `ironlint gate-bash` (Task 4). Codex's block contract differs from claude-code: a `permissionDecision:"deny"` JSON on stdout with **exit 0** (never an exit code). Reuses the existing `deny()` helper.
- Produces: a codex PreToolUse hook that gates the codex shell tool alongside `apply_patch`.

- [ ] **Step 1: Confirm the codex shell-tool name**

The codex adapter docs (`docs/adapters/codex.md`) and design spec (`specs/2026-07-02-drop-reasonix-add-codex-adapter-design.md`) describe `apply_patch` for edits but did not surface the shell-tool name in the initial scan. Confirm it by checking codex's PreToolUse event shape — either from codex's own docs, or empirically by inspecting a captured Bash event.

Run:
```bash
grep -rn -iE 'tool_name.*shell|"shell"|tool_name.*bash|"bash"|codex.*shell.*tool' specs/ docs/adapters/ adapters/codex/ 2>/dev/null
```

If the name is not found in-repo, consult the codex CLI documentation (https://github.com/openai/codex or its docs site) for the PreToolUse `tool_name` it emits for shell commands. Pin the exact string (case-sensitive — codex's matcher is a regex). Record the confirmed name in a comment in `codex_build_entry`.

If the name cannot be confirmed, STOP and surface the blocker — do not guess.

- [ ] **Step 2: Add the codex shell-tool name to the matcher**

In `crates/ironlint-core/src/adapter/registry.rs`, edit `codex_build_entry` (around line 61). Add the confirmed shell-tool name to the matcher's `|`-separated list (alongside `apply_patch|Edit|Write`):

```rust
pub(crate) fn codex_build_entry(command: &str) -> Value {
    // `apply_patch` is codex's file-edit tool. `<SHELL_TOOL_NAME>` (confirmed
    // in Step 1) is codex's shell tool — gated by the bash-gate branch. The
    // matcher must name both or the bash-gate never fires.
    json!({"matcher": "apply_patch|Edit|Write|<SHELL_TOOL_NAME>",
           "hooks": [{"type": "command", "command": command,
                      "timeout": 120, "statusMessage": "ironlint check"}]})
}
```

Replace `<SHELL_TOOL_NAME>` with the confirmed name. Update the `codex_entry_matches_apply_patch` regression test to assert the new matcher includes the shell tool.

- [ ] **Step 3: Write the failing hook-contract tests**

In `crates/ironlint-cli/tests/hook_contract_codex.rs`, add a Bash payload helper and tests. The codex block contract is deny-JSON-on-stdout/exit-0 (reuses `assert_deny`):

```rust
fn bash_payload(cwd: &std::path::Path, command: &str) -> String {
    serde_json::json!({
        "tool_name": "<SHELL_TOOL_NAME>",
        "cwd": cwd.display().to_string(),
        "tool_input": { "command": command },
    })
    .to_string()
}

#[test]
fn bash_allows_benign_command() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "trap"); // a stub block would surface if the hook wrongly spawned
    fx.run("pre-tool-use", &bash_payload(fx.project.path(), "ls"), &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn bash_blocks_ironlint_trust() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "ironlint trust must be run by a human");
    let out = fx.run("pre-tool-use", &bash_payload(fx.project.path(), "ironlint trust"), &[]);
    out.success().code(0);
    assert_deny(&out.get_output().stdout, "ironlint trust must be run by a human");
}

#[test]
fn bash_fails_closed_when_ironlint_missing() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    // No stub → spawn fails. Codex must emit a deny (fail-closed), not allow.
    let out = fx.run("pre-tool-use", &bash_payload(fx.project.path(), "ironlint trust"), &[]);
    out.success().code(0);
    assert_deny(&out.get_output().stdout, "fail-closed");
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p ironlint-cli --test hook_contract_codex -- bash_`
Expected: FAIL — the codex hook currently allows anything not `apply_patch` (line 71); the shell-tool call falls through to `exit 0` with empty stdout, so `bash_blocks_ironlint_trust` fails (no deny JSON).

- [ ] **Step 5: Add the shell-tool branch to `adapters/codex/hooks/hook.sh`**

In `adapters/codex/hooks/hook.sh`, the current gate (line 71) is:

```bash
if [[ "${TOOL_NAME}" != "apply_patch" ]]; then
  exit 0
fi
```

Replace it with a branch that catches the shell tool BEFORE the apply_patch-only gate:

```bash
# The bash-gate: shell-tool calls are classified by `ironlint gate-bash`.
# Must run BEFORE the apply_patch-only gate below, which would otherwise
# allow every non-apply_patch tool. Block contract = deny-JSON/exit 0
# (Codex never blocks via exit code). See
# docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md.
if [[ "${TOOL_NAME}" == "<SHELL_TOOL_NAME>" ]]; then
  COMMAND=$(printf '%s' "${EVENT}" | jq -r '.tool_input.command // empty')
  if [[ "${COMMAND}" != *ironlint* && "${COMMAND}" != *.ironlint* ]]; then
    exit 0   # pre-filter skip: ordinary commands pay nothing
  fi
  GATE_REASON=$(printf '%s' "${COMMAND}" | ironlint gate-bash 2>/dev/null)
  GATE_EC=$?
  case "${GATE_EC}" in
    0) exit 0 ;;
    2) deny "${GATE_REASON}" ;;
    *)
      deny "ironlint: bash-gate failed (exit ${GATE_EC}) — fail-closed"
      ;;
  esac
fi

if [[ "${TOOL_NAME}" != "apply_patch" ]]; then
  exit 0
fi
```

Replace `<SHELL_TOOL_NAME>` with the confirmed name (both occurrences).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ironlint-cli --test hook_contract_codex -- bash_`
Expected: PASS.

Run: `cargo test -p ironlint-cli --test hook_contract_codex`
Expected: PASS — no regression in the existing apply_patch tests.

- [ ] **Step 7: Check lint, syntax, coverage**

Run: `cargo clippy -p ironlint-core -p ironlint-cli --all-targets -- -D warnings`
Expected: no warnings.

Run: `bash -n adapters/codex/hooks/hook.sh`
Expected: no syntax errors.

Run: `bash scripts/ci-coverage.sh`
Expected: ≥80% on touched files.

- [ ] **Step 8: Commit**

```bash
git add crates/ironlint-core/src/adapter/registry.rs \
        adapters/codex/hooks/hook.sh \
        crates/ironlint-cli/tests/hook_contract_codex.rs
git commit -m "feat(codex): wire the Bash branch to ironlint gate-bash

PreToolUse matcher now includes the codex shell tool. A shell-tool
branch runs BEFORE the apply_patch-only gate (which would otherwise
allow every non-apply_patch tool). Substring pre-filter skips the spawn
for ordinary commands. On a hit, shells out to 'ironlint gate-bash':
exit 0 -> allow, exit 2 -> deny (deny-JSON/exit-0 per codex contract),
anything else -> fail-closed deny. Reuses the existing deny() helper."
```

---

### Task 7: Wire the pi adapter's Bash branch

**Files:**
- Modify: `adapters/pi/src/index.ts` (`GATED_TOOLS`, the bash branch, the comment at line 100)
- Modify: pi TS-side tests (per the existing pi test pattern)

**Interfaces:**
- Consumes: `ironlint gate-bash` (Task 4). pi's block contract: return `{ block: true, reason }`.
- Produces: a pi `tool_call` handler that gates `bash` alongside `write`/`edit`.

- [ ] **Step 1: Write the failing TS test for the bash branch**

Follow the existing pi test pattern (locate it under `adapters/pi/` — likely `adapters/pi/test/` or a co-located `.test.ts`). Add a test that feeds a `bash` tool_call with `ironlint trust` and asserts a `{ block: true }` return, and one with `ls` that asserts allow.

```typescript
// Adjust imports to match the existing pi test harness.
import ironlintExtension from "../src/index";

// A minimal mock pi API capturing the handler's return.
function mockPi() {
  let captured: unknown = undefined;
  return {
    api: {
      on: (_event: string, handler: (event: any) => unknown) => {
        (mockPi as any)._handler = handler;
      },
      cwd: "/fake/project",
    },
    capture: (event: any) => (mockPi as any)._handler(event),
  };
}
```

(Pattern the assertions after the existing pi tests — the exact test file path and mocking convention should be discovered by reading the existing pi tests first.)

```typescript
it("blocks 'ironlint trust' via bash", () => {
  const { api, capture } = mockPi();
  ironlintExtension(api);
  const result = capture({ toolName: "bash", input: {} });
  // ... assert result is { block: true, reason: ... }
});

it("allows 'ls' via bash (pre-filter skip)", () => {
  const { api, capture } = mockPi();
  ironlintExtension(api);
  const result = capture({ toolName: "bash", input: {} });
  // ... assert result is undefined (allow)
});
```

**Note:** pi's `tool_call` event gives `toolName` but the command string lives in `input` — confirm the exact field name by reading the existing pi tests and `computeProposedContent`'s usage of `input`. Pin it before writing the branch.

- [ ] **Step 2: Run the tests to verify they fail**

Run the pi test command (per the adapter's existing setup — likely `bun test` or `bun run test` in `adapters/pi/`).
Expected: FAIL — `bash` is not in `GATED_TOOLS`; the handler returns early (`undefined`/allow) for every bash call.

- [ ] **Step 3: Add `"bash"` to `GATED_TOOLS` and write the bash branch**

In `adapters/pi/src/index.ts`:

1. Add `"bash"` to the `GATED_TOOLS` set (line 102):

```typescript
const GATED_TOOLS = new Set(["write", "edit", "bash"]);
```

2. Replace the comment at line 100 (the "intentionally not gated" note) — the shared Rust matcher now closes that gap:

```typescript
// pi tools we gate. `bash` is gated by the bash-gate branch below, which
// shells out to `ironlint gate-bash` (the shared Rust matcher) — closing
// the "shell redirections are too brittle to parse" gap that previously
// kept bash ungated. See
// docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md.
```

3. In the `tool_call` handler (around line 205), add a bash branch BEFORE the `write`/`edit` path. The bash branch reads the command, runs the pre-filter, shells out to `ironlint gate-bash`, and returns `{ block: true, reason }` on a block:

```typescript
    if (toolName === "bash") {
      const command = (input as any).command ?? "";
      // Substring pre-filter: ordinary commands never mention ironlint or
      // .ironlint, so they skip the spawn entirely.
      if (!command.includes("ironlint") && !command.includes(".ironlint")) {
        return
      }
      const res = runIronLint(["gate-bash"], command);
      if (res.exitCode === 0) return // allow
      if (res.exitCode === 2) {
        return { block: true, reason: res.stdout || "ironlint blocked this bash command" }
      }
      // Spawn failure / signal death / unexpected exit → fail CLOSED.
      // The deny check is the thing being protected; a broken deny check
      // is never a silent allow.
      return {
        block: true,
        reason: `ironlint: bash-gate failed (exit ${res.exitCode}) — fail-closed`,
      }
    }
```

Place this branch right after `if (!toolName || !GATED_TOOLS.has(toolName)) return` (line 210), before `const filePath = getPath(input)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run the pi test command.
Expected: PASS — `ironlint trust` blocks, `ls` allows (pre-filter skip).

- [ ] **Step 5: Check no regression in the existing pi write/edit tests**

Run the full pi test suite.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add adapters/pi/src/index.ts adapters/pi/test/
git commit -m "feat(pi): wire the Bash branch to ironlint gate-bash

Reverses the documented 'bash is intentionally not gated' decision —
the shared Rust matcher (ironlint gate-bash) closes the 'shell
redirections too brittle to parse' gap that kept bash ungated.
Substring pre-filter skips the spawn for ordinary commands. On a hit,
shells out to 'ironlint gate-bash': exit 0 -> allow, exit 2 -> return
{ block: true, reason }, anything else -> fail-closed block."
```

---

### Task 8: Wire the opencode adapter's Bash branch

**Files:**
- Modify: `adapters/opencode/src/index.ts` (`GATED_TOOLS`, the bash branch)
- Modify: opencode TS-side tests

**Interfaces:**
- Consumes: `ironlint gate-bash` (Task 4). opencode's block contract: **throw** (the existing write/edit path throws on exit 2).
- Produces: an opencode `tool.execute.before` handler that gates the opencode shell tool alongside `edit`/`write`.

- [ ] **Step 1: Confirm the opencode shell-tool name**

The opencode adapter docs mention only `edit`/`write`. Confirm the shell-tool name by reading the opencode plugin SDK types (`@opencode-ai/plugin`) and/or the opencode docs for the tool name it surfaces for shell commands (likely `bash`, `shell`, or `execute`).

Run:
```bash
grep -rn -iE 'tool.*bash|tool.*shell|tool.*execute|"bash"|"shell"|"execute"' adapters/opencode/ node_modules/@opencode-ai/plugin/ 2>/dev/null | head
```

Pin the exact string. If it cannot be confirmed, STOP and surface the blocker.

- [ ] **Step 2: Write the failing TS test for the bash branch**

Follow the existing opencode test pattern. Add a test that feeds a `<SHELL_TOOL_NAME>` `tool.execute.before` event with `ironlint trust` and asserts a throw, and one with `ls` that asserts no throw.

- [ ] **Step 3: Run the tests to verify they fail**

Run the opencode test command (per the adapter's existing setup — `bun test` or `bun run test` in `adapters/opencode/`).
Expected: FAIL — the shell tool is not in `GATED_TOOLS`; the handler returns early for it.

- [ ] **Step 4: Add the shell tool to `GATED_TOOLS` and write the bash branch**

In `adapters/opencode/src/index.ts`:

1. Add the confirmed shell-tool name to `GATED_TOOLS` (line 12):

```typescript
const GATED_TOOLS = new Set(["edit", "write", "<SHELL_TOOL_NAME>"]);
```

2. In the `tool.execute.before` handler (around line 69), add a bash branch BEFORE the `edit`/`write` path. opencode blocks by **throwing** (mirroring the existing exit-2 path at lines 173/177):

```typescript
      if (input.tool === "<SHELL_TOOL_NAME>") {
        const command = (args as any).command ?? "";
        // Substring pre-filter: ordinary commands skip the spawn.
        if (!command.includes("ironlint") && !command.includes(".ironlint")) {
          return
        }
        // Spawn via Bun.spawn (async, like the check path — a sync spawn
        // blocks opencode's event loop for the full duration).
        let exitCode: number | null = null
        let stdout = ""
        try {
          const proc = Bun.spawn(["ironlint", "gate-bash"], {
            stdin: new TextEncoder().encode(command),
            stdout: "pipe",
            stderr: "pipe",
            env: process.env,
          })
          ;[stdout, , exitCode] = await Promise.all([
            new Response(proc.stdout).text(),
            new Response(proc.stderr).text(),
            proc.exited,
          ])
        } catch (err) {
          // Spawn failure (missing binary) → fail CLOSED.
          throw new Error(`ironlint: bash-gate failed — fail-closed`)
        }
        if (exitCode === null || exitCode >= 128) {
          // Signal death → fail closed.
          throw new Error(`ironlint: bash-gate killed by signal — fail-closed`)
        }
        if (exitCode === 0) return // allow
        if (exitCode === 2) {
          throw new Error(`ironlint blocked this bash command:\n${stdout}`)
        }
        // Any other exit → fail closed.
        throw new Error(`ironlint: bash-gate unexpected exit ${exitCode} — fail-closed`)
      }
```

Place this branch right after `if (!GATED_TOOLS.has(input.tool)) return` (line 74), before `const args = ...`.

- [ ] **Step 5: Run the tests to verify they pass**

Run the opencode test command.
Expected: PASS — `ironlint trust` throws, `ls` allows.

Run the full opencode test suite.
Expected: PASS — no regression.

- [ ] **Step 6: Commit**

```bash
git add adapters/opencode/src/index.ts adapters/opencode/test/
git commit -m "feat(opencode): wire the Bash branch to ironlint gate-bash

GATED_TOOLS now includes the opencode shell tool. The bash branch
runs the substring pre-filter, then shells out to 'ironlint gate-bash'
via Bun.spawn (async, like the check path). Exit 0 -> allow, exit 2 ->
throw (opencode's block contract, mirroring the exit-2 check path),
anything else / signal death / spawn failure -> fail-closed throw."
```

---

### Task 9: Update docs and CLAUDE.md

**Files:**
- Modify: `docs/adapters/claude-code.md`, `docs/adapters/codex.md`, `docs/adapters/pi.md`, `docs/adapters/opencode.md` — note that Bash is now gated and point at the spec.
- Modify: `docs/security/trust.md` — add a note that the bash-gate closes the `ironlint trust` escape hatch.
- Modify: `docs/reference/cli.md` — document `ironlint gate-bash`.
- Modify: `CLAUDE.md` (project instructions) — add the bash-gate to the "What this is" summary and the exit-code contract note.

- [ ] **Step 1: Document the `gate-bash` subcommand in the CLI reference**

In `docs/reference/cli.md`, add a section for `ironlint gate-bash`:

```markdown
## ironlint gate-bash

Reads a Bash command on stdin and decides whether it may run. Exit 0 = allow
(empty stdout); exit 2 = block (reason on stdout). Used internally by every
adapter's Bash branch; you don't call it by hand.

Not a `check`: no config load, no trust gate, no per-check spawn. The
bash-gate must run even when `.ironlint.yml` is missing or untrusted — that
is exactly when the agent is most motivated to run `ironlint trust`.

See `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`
for the threat model and the documented known gap (variable-substitution
indirection).
```

- [ ] **Step 2: Note the Bash gate in each adapter doc**

In each of `docs/adapters/claude-code.md`, `docs/adapters/codex.md`, `docs/adapters/pi.md`, `docs/adapters/opencode.md`, add a short "Bash gate" subsection:

```markdown
### Bash gate

In addition to file edits, this adapter gates `Bash` (the agent's shell tool).
Commands that would let the agent free itself — `ironlint trust`, or a Bash
write to `.ironlint.yml` / `.ironlint/gates/` — are denied. Ordinary commands
are not slowed: a substring pre-filter skips the decision entirely for
commands that never mention `ironlint` or `.ironlint`. The deny decision is
shared across every adapter via `ironlint gate-bash`.
```

- [ ] **Step 3: Add a note to the trust doc**

In `docs/security/trust.md`, add a paragraph after the "Blessing a config" section:

```markdown
## The agent can't bless its own config

The Bash tool is gated too: an agent running `ironlint trust` (or editing
`.ironlint.yml` / a gate script through Bash) is denied. The Write/Edit path
to those files stays open — it is already gated — so the change closes the
*ungated* Bash escape without removing the legitimate edit path. See
`docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.
```

- [ ] **Step 4: Update CLAUDE.md**

In `CLAUDE.md`, add to the "What this is" summary a note that Bash is gated, and add `gate-bash` to the list of CLI commands. Add a one-line note to the exit-code-contract section that the bash-gate is a separate built-in (not a `check`, not trust-gated).

- [ ] **Step 5: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "docs: document the Bash gate and 'ironlint gate-bash'

Per-adapter docs note Bash is now gated; trust.md notes the agent can't
bless its own config via Bash; cli.md documents gate-bash; CLAUDE.md
summary updated."
```

---

### Task 10: Final verification

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test`
Expected: PASS — all crates, all integration tests, all hook-contract tests green.

- [ ] **Step 2: Run clippy across the workspace**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Run the coverage gate**

Run: `bash scripts/ci-coverage.sh`
Expected: every Rust file under `crates/*/src/` at ≥80% region coverage.

- [ ] **Step 4: Run the mutation check on the matcher (local, ad-hoc)**

Run: `cargo mutants --file 'crates/ironlint-bash-gate/src/lib.rs'`
Expected: no surviving mutants.

- [ ] **Step 5: Clean up build artifacts produced during verification**

Run: `cargo clean -p ironlint-bash-gate` (if a standalone build was produced during verification)
Run: `rm -f target/release/ironlint` (only if a release binary was built for a one-off check — the persistent `target/` you iterate in stays)

- [ ] **Step 6: Request code review**

Per CLAUDE.md: "After completing a coding task, request code review from a separate agent." Dispatch a review (the `/code-review` skill or a fresh subagent) against the full diff (`git diff main`).
