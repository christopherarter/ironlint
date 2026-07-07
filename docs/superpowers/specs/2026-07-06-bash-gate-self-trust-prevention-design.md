# Bash gate — preventing agent self-trust via `ironlint trust`

**Date:** 2026-07-06
**Status:** Design (pre-implementation)
**Scope:** new crate `ironlint-bash-gate`; `ironlint-cli` (one subcommand); all four adapters (`claude-code`, `codex`, `pi`, `opencode`)

## Problem

ironlint's trust store lives outside the repo precisely so a config "can't
vouch for itself" — a pulled, generated, or freshly-edited config is untrusted
until *you*, on *this* machine, bless it (`docs/security/trust.md`). That
protection is moot if the coding agent can simply run `ironlint trust` (or edit
`.ironlint.yml` / a gate script) through the **Bash** tool, which no adapter
intercepts today.

The claude-code PreToolUse hook (`adapters/claude-code/hooks/hook.sh`) matches
`Edit|Write|MultiEdit|NotebookEdit` only — `Bash` is ungated. The other three
adapters are the same: codex gates only `apply_patch`; pi explicitly declines to
gate `bash` (`adapters/pi/src/index.ts:100` — *"shell redirections like `cat >
foo` are too brittle to parse — universal adapter gap"*); opencode gates only
`edit`/`write`. A lazy agent that hits an ironlint block can run `ironlint
trust` via Bash with no gate, blessing the very config that constrains it.

## Goal

Intercept Bash tool calls in every supported adapter and deny the ones that
would let an agent free itself: `ironlint trust` (any args), and Bash writes to
the policy surface (`.ironlint.yml` and anything under `.ironlint/gates/`).
The Write/Edit path to those files stays open — it is already gated by
ironlint's existing PreToolUse hook — so the change closes the *ungated* Bash
escape without removing the legitimate, gated edit path.

## Threat model (decided)

**Lazy, mostly smaller non-reasoning models** reaching for the obvious escape,
not prompt-injected adversaries. The matcher blocks the direct forms and a
first tier of light obfuscation; it deliberately does **not** defeat
variable-substitution indirection (`iron$(echo lint) trust`, `IRON=ironlint; $
IRON trust`, `base64 … | sh`, invoking through a script file). That is a
documented known gap, not a TODO for this tier — catching it requires real
shell evaluation, which crosses into the adversarial tier this design
explicitly scopes out. The design's honesty lives in pinning both what it
catches and what it doesn't.

## Non-goals

- No real shell parsing / AST lexing. Variable-substitution indirection is out
  of scope for this threat tier.
- No sandboxing. A blessed check still runs with full user privileges; this
  gate is about preventing the agent from *granting itself* trust, not about
  what a trusted check can do once it runs.
- No new runtime dependency. Every adapter already shells out to the `ironlint`
  binary for `ironlint check`; the new subcommand reuses it.
- Not a general `ironlint-govern` `tool_call` product. This is a built-in
  self-protection rule — a natural stepping stone toward `ironlint-govern eval`
  (`specs/2026-07-01-ironlint-govern-portable-governance-design.md` §8), but it
  does not pre-build the govern crate or the kernel extraction.

## Architecture

### The matcher lives in the `ironlint` binary, not in any adapter

A new built-in subcommand holds the deny logic as a pure function of the
command string → `Decision { Allow, Block(reason) }`. Each adapter's Bash
branch is a thin shim: a substring pre-filter (in-adapter, no spawn) → shell
out to the subcommand → translate the exit code through *that adapter's*
existing block contract. One matcher, one place to read, no cross-language
drift.

```
agent Bash call
  → adapter PreToolUse / tool_call handler
  → substring pre-filter: does the command mention "ironlint" or ".ironlint"?
       no  → exit 0 (allow)                 ← ordinary ls/git/cargo pay nothing
       yes → printf '%s' "$cmd" | ironlint <gate-subcmd>
              → exit 0 = allow, exit 2 = block (reason on stdout)
  → adapter translates exit code via its OWN block contract:
       claude-code: exit 2 (stderr reason)
       codex:        deny-JSON on stdout, exit 0
       pi:           return { block: true, reason }
       opencode:     throw new Error(reason)
```

### Three properties that make this not-a-`check`

1. **It is not a `check`.** It does not go through `ironlint check`, the
   verdict machinery, or the trust gate. There is no file content, no
   `$IRONLINT_FILE`, no per-check spawn. It is a separate, narrower built-in.
2. **It is NOT trust-gated — by design.** The trust gate verifies
   user-authored config/checks haven't been tampered with since the human
   blessed them. The bash-gate is ironlint's own shipped logic; there is
   nothing to "trust." Requiring trust here would be circular: the gate exists
   to stop the agent from *granting itself* trust. The bash-gate must run even
   when `.ironlint.yml` is missing or untrusted — that is exactly when the
   agent is most motivated to run `ironlint trust`. The subcommand depends on
   neither a valid nor a trusted config.
3. **No new runtime dependency.** Every adapter already shells out to the
   `ironlint` binary. The substring pre-filter means ordinary Bash calls
   (`ls`, `git status`, `cargo test`) never spawn it. Only commands mentioning
   `ironlint` or `.ironlint` pay one process spawn — the same cost every edit
   already pays.

### Exit contract — binary

The subcommand emits a deliberately binary exit code, matching the
`ironlint-govern` contract (govern spec §6: *"0 = allow, non-zero = block"*):

- `0` — allow the Bash call to proceed.
- `2` — block; the reason is on stdout.
- anything else (signal death, missing binary, unexpected exit) — adapters
  **fail closed**. This is stricter than the file-gate's exit-3 fail-open
  default; the deny check is the thing being protected, so a broken deny check
  is never a silent allow.

No env-var override (decided). No `IRONLINT_ALLOW_TRUST_BASH` escape hatch.

## The deny matcher

### One pure function

```rust
// ironlint-bash-gate
pub enum Decision { Allow, Block(String) }
pub fn decide(command: &str) -> Decision;
```

Pure of I/O, pure of state. Everything else (pre-filter, subcommand wrapper,
exit-code translation) is plumbing around it. This is the unit-testable
surface; the test file is the spec.

### What it blocks

**(1) `ironlint trust`, any args.** The primary escape. `ironlint trust`,
`ironlint trust --config shared/base.yml`, `ironlint trust .` — all blocked.
Reason: *"ironlint trust must be run by a human, not by an agent — it blesses
the config that gates the agent."*

**(2) Bash writes to the policy surface** — `.ironlint.yml` and anything under
`.ironlint/gates/`. The Write/Edit path to these files stays open (already
gated); closing the Bash path forces the escape through the gated path.
Detected via:
- redirect operators targeting those paths (`>`, `>>`, `>|`, `&>`, `&>>`),
- `tee` writing them,
- in-place editors (`sed -i`, `ed`, `perl -i`),
- copy/move *onto* them (`cp`, `mv` with a policy path as the destination).

`cp`/`mv` are matched only when a policy path is the **destination** (the
second argument for two-arg forms, or the last for multi-source forms) —
`cp .ironlint.yml /tmp/backup` (policy path as *source*) is a read, not a
write, and must allow. Reason: *"ironlint policy files must be edited through
the Write/Edit tool (which is gated), not via Bash."*

### De-obfuscation (one normalization pass, then match)

A lazy model's first moves, collapsed before matching — string operations, not
a shell parser:

- **Strip backtick / `$()` delimiters** around the binary name: `` `ironlint`
  trust `` and `$(ironlint) trust` → `ironlint trust`. Match on the *contents*
  of the substitution, not the delimiters.
- **Collapse whitespace** around the binary token: `ironlint   trust`,
  `ironlint\ttrust` → `ironlint trust`.
- **Resolve leading quoted fragments**: `'ironlint' trust`, `"ironlint" trust`
  → `ironlint trust`.
- **`cd .ironlint && trust`-style**: a `cd` into `.ironlint/` chained with
  `&&`/`;`/`|` before a bare `trust` — collapsed so the `trust` token is
  caught even without the binary prefix.

### Direct forms caught by the segment/wrapper layer (code-review hardening)

After the initial pass, a review surfaced direct-form escapes a lazy model
plausibly emits (not indirection — the binary is invoked by literal name, the
write targets a literal path). These are caught by a second layer on top of
the de-obfuscation pass:

- **Chained commands**: `ironlint check || ironlint trust` (the textbook
  escape — "the check failed, so just trust it"), plus `&&`, `;`, `|`. The
  command is split at the shell separators into segments; a `trust` or policy
  write in ANY segment blocks the whole command. (`>|` clobber is NUL-sentinel-
  protected so its `|` isn't mis-split as a pipe.)
- **`or`-confusion (non-separator token)**: `ironlint check or ironlint trust`
  — `or` is not a shell operator (that's `||`), so a lazy model confusing the
  two leaves the whole string as ONE segment (segments() correctly does not
  split on bare `or`). sh runs `ironlint check`, then `or` (command not found),
  then `ironlint trust` — trust fires. `is_ironlint_trust` checks EVERY
  ironlint binary occurrence in the segment (not just the first token), so the
  second binary's `trust` subcommand is caught even though the first binary's
  `check` short-circuits. The single-binary stray-positional case
  (`ironlint check trust`, where `trust` is a positional to `check` that clap
  rejects) still allows — only a real second `ironlint trust` invocation blocks.
- **Prefix wrappers**: `nohup`, `env [VAR=val]…`, `exec`, `eval`,
  `timeout <N>`, and `sh`/`bash -c '<cmd>'` — stripped before the binary check,
  recovering the direct form. For `sh`/`bash`, the `-c` command-string argument
  is descended into (normalize already stripped its quotes, so its tokens are
  re-checked); without `-c` (`sh script.sh`) the wrapper breaks — that's the
  script-file indirection gap (adversarial tier). **Bare `VAR=val ironlint
  trust`** is semantically identical to `env VAR=val ironlint trust` (sh
  exports the assignment to the command's env); the bare prefix is stripped the
  same way `env`'s assignments are, via the shared `is_assignment` helper (a
  strict shell-identifier check — letters/digits/underscore, not digit-leading
  — so a leading `--config=x.yml` flag is NOT over-skipped).
- **Path-prefixed binary**: `/usr/local/bin/ironlint trust`, `./ironlint trust`
  — `is_ironlint_binary` matches the literal name OR a path ending `/ironlint`.
- **Global flags before the subcommand**: `ironlint -v trust`,
  `ironlint --verbose trust` — skipped; the first non-flag token decides
  (`trust` blocks, a read-only subcommand allows). (Note: clap rejects
  `--config` before a subcommand at runtime, so `ironlint --config x.yml
  trust` can't actually run — but the no-value global-flag forms are real.)
- **End-glued redirects**: `echo x>.ironlint.yml` (the most common form — no
  space before the `>`) — a token containing a redirect op AND ending in a
  policy path blocks; the start-glued and bare-op forms were already caught.
- **Additional write primitives**: `dd of=<policy>`, `install`/`rsync` with a
  policy destination, `sponge <policy>`. (`cp`/`mv` destination check
  extended to `install`/`rsync`; `dd`'s `of=` operand checked, source `if=`
  not.)
- **Subshell / brace grouping**: `(ironlint trust)`, `{ ironlint trust; }` —
  `(` `)` `{` `}` stripped in normalize, collapsing to the direct form.

### What it does NOT block (known gap, pinned by tests)

- Variable-substitution indirection: `iron$(echo lint) trust`,
  `$(printf 'ironlint') trust`, `IRON=ironlint; $IRON trust` — the binary
  name is assembled at runtime. Catching this means parsing shell evaluation;
  out of scope for this threat tier.
- `base64 -d <<< '...' | sh`, `eval` over assembled strings — same tier.
- Invoking through a script file: `bash scripts/x.sh` where `x.sh` contains
  `ironlint trust` — the command string is `bash scripts/x.sh`, no `ironlint`
  token. The substring pre-filter never reaches the matcher.

The test file asserts both directions: shapes we catch (must `Block`) and
shapes we don't (must `Allow` — pinned so a future "improvement" that
false-positives on them trips a review flag, not a silent catch).

### False positives are the harder constraint

A missed `ironlint trust` variant is a gap a lazy model *might* stumble into;
a false positive on `cargo test` or `git status` is a *guaranteed* broken
workflow the human hits immediately. The allow-list over-indexes on commands
that look dangerous but aren't: every read-only ironlint subcommand (`check`,
`doctor`, `validate`, `explain`, `show-resolved-config`, `init`), `cat
.ironlint.yml | grep checks`, `grep -r ironlint docs/`, `ls .ironlint/gates/`,
a heredoc that *mentions* `.ironlint.yml` without writing to it, and the
subtle case `echo "run ironlint trust to bless"` — the string `ironlint trust`
appears in an `echo`, not as a command. The matcher must distinguish "the
command *is* `ironlint trust`" from "the command *mentions* `ironlint trust`."

## Placement — new crate `ironlint-bash-gate`

A new leaf crate holds *only* `decide` + its tests. The CLI depends on it;
adapters shell out to the CLI. Rationale:

- **Cleanest boundary.** The matcher is isolated, testable in isolation, and a
  future `ironlint-govern` (or `ironlint-kernel`) re-exports it with zero
  migration.
- **Respects the existing crate roles.** `ironlint-core` is the *linter*
  (write-product) library; the bash-gate is conceptually a `tool_call`-product
  concern (closer to govern). Dropping it into core would muddy that boundary
  now and force a churny relocation when govern arrives. `ironlint-cli` is
  meant to be a *thin* binary (one-function adapters into core); dropping the
  deny logic there breaks that thinness. A small focused crate is the repo's
  established pattern (two-crate workspace today, govern planned as a third).
- **Small.** Likely one source file + one test file. The overhead is a
  `Cargo.toml` and a workspace line.

The subcommand wiring in `ironlint-cli` is ~15 lines: a clap subcommand reads
stdin, calls `ironlint_bash_gate::decide`, maps `Allow → exit 0` /
`Block(reason) → { stdout=reason, exit 2 }`. Spawn failure / signal death
fails closed (handled in the adapters; the subcommand itself is a pure
function and cannot crash on bad input — it takes a `&str`).

### Subcommand name — open

`ironlint gate-bash` is the working name. It is a built-in, not a `check`, and
the seed of what `ironlint-govern eval` becomes later. The `govern` namespace
is deliberately not claimed before that crate exists. Alternatives under
consideration: `bash-gate`, `guard-bash`. Pinned at implementation time.

## Adapter changes

Each adapter gets the same shape: a Bash branch that runs the substring
pre-filter, shells out to the subcommand, and translates the exit code through
its own block contract. The block contract differs per harness (inherent to
each API) — each adapter already does this for `ironlint check`'s exit codes.

### Common pre-filter (identical in all four)

```
does $command contain "ironlint" or ".ironlint"?
  no  → allow, no spawn
  yes → printf '%s' "$command" | ironlint <gate-subcmd>
```

The fast path. It must never false-*block* — its only job is to skip the
spawn. A command that mentions `ironlint` but is benign (`ironlint check`,
`grep ironlint docs/`) still goes through the subcommand, which allows it. The
pre-filter is an optimization, not a decision.

### claude-code (`adapters/claude-code/hooks/hook.sh`, bash)

- Add `Bash` to the matcher in `claude_build_entry`
  (`crates/ironlint-core/src/adapter/registry.rs:50`):
  `"Edit|Write|MultiEdit|NotebookEdit|Bash"`.
- New `Bash)` arm in the `case "${TOOL_NAME}"` block, **before** FILE
  extraction. This is essential: a Bash event has no `file_path`, so the
  existing empty-FILE early-exit (`hook.sh:79-82`) would silently allow every
  Bash call if the arm came after.
- Extracts `tool_input.command` via jq, runs the pre-filter, and on a hit
  shells out to `ironlint <gate-subcmd>`. Subcommand exit `2` → hook exits `2`
  with the reason on stderr (claude-code's PreToolUse deny contract). Exit `0`
  → allow.
- Spawn failure / signal death → **fail closed**, exit `2` with a clear stderr
  message. Stricter than the file-gate's exit-3 fail-open default.

### codex (`adapters/codex/hooks/hook.sh`, bash)

- Add the codex shell-tool name to the matcher (currently
  `apply_patch|Edit|Write`, `registry.rs:61`). Codex reports file edits as
  `tool_name:"apply_patch"` regardless of matcher alias, and the hook *allows
  anything not `apply_patch`* (`hook.sh:71`), so a branch before the
  `apply_patch`-only gate catches shell-tool calls.
- Block contract differs from claude-code: codex blocks with a
  `permissionDecision:"deny"` JSON on stdout and **exit 0** — never an exit
  code. The deny path reuses the existing `deny()` function (`hook.sh:36-47`)
  and exits 0; the allow path exits 0 with empty stdout.
- **Shell-tool name to confirm at implementation time** from the codex adapter
  design spec under `specs/`.

### pi (`adapters/pi/src/index.ts`, TypeScript)

- Add `"bash"` to `GATED_TOOLS` (currently `{"write","edit"}`, line 102).
- **Reverses a documented decision.** Line 100 says *"bash is intentionally
  not gated (shell redirections like `cat > foo` are too brittle to parse —
  universal adapter gap)."* The shared Rust matcher closes that gap — the
  parsing no longer lives in the adapter. The comment is replaced with one
  pointing at `ironlint <gate-subcmd>` as the canonical parser.
- Block contract: the existing `tool_call` handler returns
  `{ block: true, reason }` (line 225); the bash branch does the same.
- The existing `failOpenOrClosed` helper (line 141) is the *opposite* posture;
  the bash branch must not reuse it for spawn failures. Spawn failure /
  signal death → fail closed (return a block).

### opencode (`adapters/opencode/src/index.ts`, TypeScript)

- Add opencode's shell-tool name to `GATED_TOOLS` (currently `{"edit","write"}`,
  line 12).
- Block contract: opencode blocks by **throwing** (lines 173, 177); the bash
  branch throws on deny.
- **Shell-tool name to confirm at implementation time** (is it `bash`?
  `execute`?).

## Testing strategy

The test file is the spec. Three layers, each with a distinct job.

### Layer 1 — `ironlint-bash-gate` unit tests (the matcher, pure)

`decide(command: &str) -> Decision` is pure, so the test file is a table of
`(input, expected)` rows. Three categories:

**Block cases (must `Block`):** the direct forms and the light-deobfuscation
forms we catch.
- `ironlint trust`, `ironlint trust --config shared/base.yml`, `ironlint trust .`
- `` `ironlint` trust ``, `$(ironlint) trust`, `'ironlint' trust`,
  `ironlint   trust` (whitespace)
- `cd .ironlint && trust`, `cd .ironlint/gates && trust`
- Writes to policy surface: `echo x > .ironlint.yml`, `echo x >> .ironlint.yml`,
  `cat > .ironlint.yml`, `tee .ironlint.yml`, `tee -a .ironlint.yml`,
  `sed -i 's/x/y/' .ironlint.yml`, `perl -i -pe '...' .ironlint.yml`,
  `ed -s .ironlint.yml`, `cp malicious.sh .ironlint/gates/lint.sh`,
  `mv bad.yml .ironlint.yml`, and the same for `.ironlint/gates/lint.sh`
- All redirect variants: `>`, `>>`, `>|`, `&>`, `&>>`

**Allow cases — the false-positive guard (must `Allow`):** the harder
constraint. Commands that look dangerous but aren't.
- Every read-only ironlint subcommand: `ironlint check`, `ironlint doctor`,
  `ironlint validate`, `ironlint explain`, `ironlint show-resolved-config`,
  `ironlint init`
- `cat .ironlint.yml | grep checks`, `grep -r ironlint docs/`,
  `ls .ironlint/gates/`, `cat .ironlint/gates/lint.sh`
- `cp .ironlint.yml /tmp/backup` — policy path as the **source**, not the
  destination (a read, not a write). Pins the `cp` source-vs-destination
  distinction.
- A heredoc that *mentions* `.ironlint.yml` without writing to it
- `cargo test`, `git status`, `ls`
- `echo "run ironlint trust to bless"` — the string `ironlint trust` appears in
  an `echo`, not as a command. Pinned explicitly.

**Known-gap allow cases (must `Allow`, documented as the scope boundary):**
`iron$(echo lint) trust`, `IRON=ironlint; $IRON trust`, `bash scripts/x.sh`,
`base64 -d <<< '...' | sh`. These *must* allow, with a comment naming them as
the documented gap.

**Mutation testing** (per CLAUDE.md — local, ad-hoc, not a CI gate):
`cargo mutants --file 'crates/ironlint-bash-gate/src/lib.rs'`. A surviving
mutant in `decide` means a test exercised the line but didn't verify the
decision. Run during implementation.

### Layer 2 — CLI subcommand e2e (`assert_cmd`, mirroring `cli_e2e_trust`)

The thin subcommand wrapper: stdin → exit code. Pin:
- `ironlint <gate-subcmd>` with `ironlint trust` on stdin → exit `2`, stdout =
  reason
- with `ironlint check` on stdin → exit `0`, empty stdout
- with empty stdin → exit `0` (nothing to decide; allow, no crash)
- with malformed bytes (non-UTF8) → exit `0`, log to stderr. This path is
  unreachable in practice (the pre-filter only forwards commands that
  contained a UTF8-decodable `ironlint`/`.ironlint` substring), but the
  subcommand must handle it without crashing. Allow-and-log is the safe default
  for an unreachable-but-defended path.
- No config present, no trust store — subcommand runs anyway (Architecture
  property 2). Pinned: the bash-gate works in a bare directory with no
  `.ironlint.yml`.

### Layer 3 — adapter hook-contract tests (per adapter)

The four `hook_contract_*.rs` files already drive each adapter via `assert_cmd`
against simulated PreToolUse events. Each gets a new test class: a Bash event
with a dangerous command → the adapter's block contract fires; a Bash event
with a benign command → allow.
- `hook_contract_claude_code.rs`: Bash `ironlint trust` event → hook exits `2`,
  reason on stderr. Bash `ls` event → exit `0`.
- `hook_contract_codex.rs`: Bash shell-tool event with `ironlint trust` →
  deny-JSON on stdout, exit `0`. Benign → empty stdout, exit `0`.
- pi and opencode: their existing contract-test patterns extended with the Bash
  path (TS-side tests rather than `assert_cmd` — TBD per adapter's existing
  setup).
- **Fail-closed-on-spawn-failure test** for each adapter: simulate a missing
  `ironlint` binary (or a signal death) and assert the adapter *blocks*, not
  allows. This is the posture that differs from the file-gate's fail-open
  default — it must be pinned.

### Coverage gate

Per CLAUDE.md, every Rust file under `crates/*/src/` must hit ≥80% region
coverage (CI via `scripts/ci-coverage.sh`). The matcher is pure and small —
hitting the gate is straightforward, but the false-positive test cases are
what make it *meaningful* coverage. The mutation run during implementation is
the real check.

## Relationship to `ironlint-govern`

This design is a stepping stone, not a substitute. The long-term home for
"intercept any tool call, decide via policy" is `ironlint-govern eval` over the
extracted `ironlint-kernel` (`specs/2026-07-01-ironlint-govern-portable-governance-design.md`
§8). When that crate lands, the natural move is to relocate `decide` into
govern's policy evaluation and re-export it — the crate boundary makes that a
re-export, not a rewrite. Building the bash-gate now does not pre-build the
kernel extraction or the govern CLI; it ships a focused, tested self-protection
rule that the govern work later absorbs.

## Open questions

1. **Subcommand name** — `gate-bash` (working), `bash-gate`, or `guard-bash`.
   Pinned at implementation.
2. **codex / opencode shell-tool names** — confirmed from the adapter design
   specs under `specs/` during implementation, not a design fork.
3. **`ironlint-govern` relocation timing** — when govern lands, does
   `ironlint-bash-gate` fold in as a built-in policy, or stay a standalone
   crate re-exported by govern? Decided then; this design only ensures the
   crate boundary makes either move cheap.
