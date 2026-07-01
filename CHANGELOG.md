# Changelog

Notable changes to Hector, newest first. In-flight work lives in `plans/`.

## [0.6.0] — 2026-07-01 — init onboarding plan preview

### Changed

- **`hector init` previews what it installs, then confirms.** Harness
  onboarding now renders a per-file plan — grouped per harness and tagged
  `detected` (found on this machine) or `requested` (named with `--harness`) —
  showing every hook/plugin file it writes, the settings key it patches, and the
  `hector-config` authoring skill it installs, then prompts `Proceed? [Y/n]`.
  This replaces the terse "Install hector hooks into …" prompt, which
  understated the footprint (it also installs a skill) and named no paths. The
  two entry paths are unified: explicit `--harness` previously installed
  silently with no preview — it now shows the same plan and confirms. `--yes`
  skips the prompt; `--dry-run` renders the plan and installs nothing. Paths
  display home- and project-relative, and color is applied only when stdout is a
  terminal (piped/CI output stays plain). The `hector init --dry-run` output is
  now this plan tree rather than the former flat `write <path>` list.

## [0.5.0] — 2026-06-30 — temp files, `--force`, stack-agnostic init

### Added

- **`$HECTOR_TMPFILE`.** On a `write` event, when a check's `run`/`steps`
  reference the token, hector materializes the proposed content (the bytes
  also delivered on stdin) to a temp file beside `$HECTOR_FILE` with the same
  extension, exports its absolute path, and removes it after the check. This is
  for file-oriented tools (Biome, ESLint file-mode, `tsc`, ruff) that want a
  real path on disk with the right extension rather than stdin. Creation is
  lazy (only when the check references the token) and write-only — on
  `pre-commit` the files are already on disk at `$HECTOR_FILES`, so the var is
  unset. Materialization is bounded to the project root unless
  `--allow-external-paths`. Additive to the check ABI — no schema change; a
  check that never mentions the token is unaffected.
- **`hector check --force`.** Run the named `--check <id>`(s) against `--file`
  even when the path falls outside their `files` glob — for ad-hoc testing of a
  check against an arbitrary fixture. Scope-only: the check-id filter, inline
  `hector-disable` directives, and the `on:` lifecycle all still apply.
  Requires `--check`; exits `1` if passed without it.

### Changed

- **`hector init` is stack-agnostic.** It no longer detects the toolchain or
  scaffolds tool-specific checks — the Biome / ESLint / ruff wrappers and the
  Rust/Node grep templates are gone, and so is the stack-detection that fed
  them. `init` now emits one universal baseline — `no-fixme` and
  `no-merge-markers` (both read proposed content from stdin) — plus commented,
  copy-paste examples showing the stdin and `$HECTOR_TMPFILE` patterns. Harness
  onboarding (installing hector's hook into claude-code / pi / opencode /
  reasonix) is unchanged.

## [0.4.0] — 2026-06-29 — checks pipeline redesign

### Breaking

- **Config schema.** The top-level `gates:` key is replaced by `checks:`. Each
  check is `files:` (glob or list) + `run:` (shell command) or `steps:` (a
  sequence of `{name, run}`), plus optional `on:` (lifecycle) and `name:`. Old
  `gates:` configs fail at parse time.
- **Nonzero blocks.** Any nonzero exit (1–125) from a check now blocks. The
  old model required exit `2` to block; exits 0, 1, and 3–125 were passes. The
  natural forbid idiom is now `! grep -nE 'pattern'` (no `exit 2` ceremony).
  Checks that were written to exit `2` continue to block — the change is
  additive for existing scripts.
- **Lifecycles: `on: [write, pre-commit]`.** Default is `on: [write]`. The
  `write` lifecycle fires per file on every agent edit, with proposed content
  on stdin. The `pre-commit` lifecycle fires once per check before a commit,
  with `$HECTOR_FILES` (newline-joined staged files) and empty stdin.
- **`$HECTOR_FILES`.** New env var: newline-joined list of all files under
  check (single entry for `write`; all staged files for `pre-commit`).
  `$HECTOR_FILE` is still set for `write` but not for `pre-commit`.
- **`$HECTOR_EVENT`** is now `write` or `pre-commit` only. The former
  `edit` and `manual` values are gone.
- **Verdict schema 5 / telemetry schema 5.** `Verdict` and `PerCheckRecord`
  carry the updated lifecycle and check vocabulary. Adapters parsing the
  verdict JSON must accept `schema_version: 5`.

### Added

- **`hector watch`** — a read-only live TUI over `.hector/log.jsonl` with a
  Stream (newest-first run feed) and an Explorer (per-check health ranked by
  blocks). Built on existing telemetry; no schema change.
- **`hector update`.** Self-update to the latest GitHub release. Reads the
  cargo-dist install receipt and, when a newer release exists, downloads and
  re-runs the same installer the binary was installed with, then self-replaces.
  A no-op when already current; exits `1` with channel-specific guidance for
  non-installer builds (Homebrew / `cargo install` / source).
- **`hector schema`.** Prints the `hector-config` authoring skill to stdout —
  the same guide that `hector init` installs for each coding agent.
- **`hector init` harness onboarding.** `hector init` now installs hector's
  hook into detected coding agents with a detect-then-confirm UX. Bare `hector
  init` auto-detects installed harnesses (claude-code, reasonix, pi, opencode)
  and prompts before writing anything. New flags: `--harness <name|all>`
  (repeatable; selects explicitly), `--yes` (skip prompt), `--hook-only` (skip
  config scaffold), `--no-hook` (config only, legacy behaviour), `--dry-run`
  (print plan, write nothing), `--uninstall` (remove hook entry + artifacts;
  leaves `.hector.yml` and trust store untouched), `--global` (claude-code and
  pi only: write to user-global settings instead of project). Adapter artifacts
  are written atomically to `~/.config/hector/adapters/<harness>/`; a
  `.hector-adapter.json` sidecar (per-file sha256 + version) tracks installed
  state. Re-runs are idempotent.
- **`hector doctor` per-harness adapter status.** `doctor` now reports a row
  for each installed harness inside the existing `checks[]` array — name is the
  harness id (`claude-code`, `reasonix`, `pi`, `opencode`), same
  `{name, status, detail, remediation}` shape as other checks. A registered
  harness with a missing artifact → `fail` → exit 1; modified or outdated
  artifact → `warn`; ok → `pass`; not installed/registered → row omitted.

### Known gaps

- **`docs/` guide tree.** The in-depth guides under `docs/` (getting-started,
  architecture, writing-checks, reference, adapters) still document the 0.3
  `gates:` / exit-2 model. Migration to the 0.4 checks model is a tracked
  follow-up; until then, prefer `README.md` and `hector schema` as the
  authoritative references.

## [0.3.0] — 2026-06-25 — gates redesign

### Added

- **Trust:** out-of-repo allow-list at `~/.config/hector/trust.json`; `hector
  check` fails closed (exit 1) until `hector trust` blesses the config +
  `.hector/gates/`; `hector init` auto-blesses.

### Breaking

- **Config schema.** The top-level `rules:` key is replaced by `gates:`. Each
  gate declares `files:` (glob or list of globs) and `run:` (shell command).
  Old `schema_version`, `engine:`, `severity:`, `skip:`, and `llm:` fields
  are rejected at parse time.
- **Exit-code contract.** Exit 2 = Block (≥1 gate returned exit 2); exit 3 =
  InternalError (≥1 gate crashed: not-found / timeout / signal); exit 0 = Pass.
- **Verdict schema 4 / telemetry schema 3.** `Verdict` no longer carries
  `deferred_rules`, `engine`, or `severity` fields. Telemetry `LogEntry`
  drops `semantic_verdict` and `semantic_skipped` variants.

### Removed

- `engine:`, `severity:`, `baseline:` / `hector baseline`, `hector migrate`,
  `skip:` — all removed; configs using them fail at load with a pointed error.
- `guide` subcommand folded into `hector explain` (now shows gates in scope
  for a file and their run commands).

### Changed

- `hector init` success message now advises `hector check --file <path>`
  after scaffolding (init auto-blesses, so no separate `hector trust` is needed).
- `Explain` subcommand help reworded to gates vocabulary.
- `ExecutionConfig.max_workers` removed (dispatch is sequential; only
  `timeout_secs` is used). Configs that set `execution.max_workers` continue
  to parse (serde ignores unknown fields).

## 0.2 wire-format coordination — superseded by 0.3, never released

> Historical record. The 0.2 line (engines, `llm:`, capability sandboxing, the
> deferred semantic-eval envelope) was developed but never released; the 0.3
> gates redesign above supersedes all of it. Kept for provenance.

This work batched all four contract-shaped changes from
`docs/audits/2026-05-24-check-end-to-end-audit.md` into one CHANGELOG
section so adapters and consumers see them together. Skip to
**Migrating to 0.2** below for the upgrade checklist.

### Removed

> **Supersedes the LLM-related entries below.** The deferred-envelope,
> `semantic`/`session`, and `llm:` work recorded elsewhere in this 0.2
> section was built during the cycle and then removed before release — 0.2
> ships as a static gate. Those entries are kept for development history; the
> bullet here is the net effect.

- **LLM evaluation.** The `semantic` and `session` engines, the `llm:` config
  block, `--emit-semantic-payload`, `--print-prompt`, `check --session`,
  `hector session`, and `hector record-verdict` are gone. Hector is a static
  gate: `script` + `ast` only. Configs containing the removed engines fail at
  load with a pointed error naming the rule; `hector migrate` drops them with
  a notice. Verdict schema is now 3 (drops `deferred_rules` and the
  `semantic`/`session` engine tags); telemetry schema is now 2.

### Breaking

- **C1 (trust):** the trust fingerprint now canonicalizes through
  `serde_json::Value` (RFC 8259) instead of `serde_yaml`'s emitter.
  `serde_yaml`'s output is not normative — scalar style and indent width
  changed across 0.8/0.9/0.10, so a `cargo update` could invalidate every
  checked-in fingerprint with no actual config change. Every checked-in
  `.hector.yml` must be re-signed: `hector trust <path>`. Old fingerprint
  mismatch errors now include a re-sign hint. YAML anchors/aliases are
  rejected with a clear error rather than silently hashed.
- **C5 (prompt sentinel):** the deferred envelope's `evaluator_input`
  now wraps trusted-policy and untrusted-evidence in **per-call random
  delimiters** (`<TP-{32 hex}>…</TP-{token}>`, `<UE-{token}>…`). The
  previous fixed `<TRUSTED_POLICY>`/`<UNTRUSTED_EVIDENCE>` tags were
  guessable, letting attacker-supplied content forge a close-tag and
  inject a fake policy section. Anything parsing the prompt structure
  (interpreter skill, `hector-evaluator` subagent) must read the
  boundaries from the rendered prompt, not assume the literal tags.
- **`DEFERRED_SCHEMA_VERSION` bumped 2 → 3** (non-additive: `evaluator_input`
  shape changed from a single string built without per-rule context to a
  per-rule structure threading `context: file`/`context: repo`; see B5).

### Added
- **B7 (`Status::InternalError`):** new verdict status + CLI exit code
  **3** for engine-level errors (LLM key missing, AST refused diff,
  script spawn failure, context expansion failure). Previously these
  surfaced as `Block` with a confusing `__internal` violation. Adapter
  policy: default to **allow** on exit 3 (fail-open); opt into
  fail-closed via `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`. Both the
  Claude Code hook (`adapters/claude-code/hooks/hook.sh`) and the
  OpenCode plugin honor this.
- **B4 (deferred warnings):** the deferred envelope's
  `payload.warnings` now carries deterministic Warn-severity violations
  the deferred branch used to drop on the floor. Operators were
  silently losing every script/AST warning whenever the deferred
  branch fired.
- **B3 (`claude-code-subagent` + `engine: session` stop path):**
  `hector check --session` under the subagent provider used to print
  *"internal error during session check"* — `check_session`
  hard-required an `LlmClient` and `build_from_config` returns `None`
  for the subagent provider. Now emits a session-aggregate
  `DeferredVerdict` (`file: ""`, `diff: <per-edit framing>`); the
  Claude Code stop hook wraps it in `hookSpecificOutput.additionalContext`
  exactly like the `PostToolUse` per-file branch.

### Changed
- **A1 (baseline)**: file-level violations (`line: None`) now require
  both fingerprint AND normalized body match. The prior behavior turned
  baseline into a per-file disable for passthrough script rules (the
  default since R4). v2 baselines continue to match on fingerprint
  alone during a grace period; run `hector baseline record` to
  re-record entries under the new schema. Storage schema bumped v2 → v3.
- **B5 (per-rule context expansion):** the deferred envelope's
  `evaluator_input` is now per-rule. Each rule's slice uses the rule's
  declared `context:` scope (`diff` / `file` / `repo`) — `context: file`
  threads the full file body; `context: repo` threads the canonical
  repo-expansion stub. The previous single-string render meant a
  `context: file` rule would silently see only the diff under the
  subagent route while the direct-API route saw the file. Direct-API
  and subagent now produce byte-identical evidence modulo the C5
  per-call sentinel. Session rules also see per-rule scoped aggregates
  (only edits matching the rule's scope).
### Fixed
- **A2 (diff parser)**: POSIX `diff -u` patches with `\t<timestamp>`
  headers now parse correctly. The previous parser only stripped `\r`,
  so the tab and timestamp landed in the `PathBuf` — every scope match
  silently missed and `hector check --diff` returned a clean pass on
  any non-git patch. `build_single_file_diff` had the symmetric bug;
  both call sites are fixed and regression-tested at parser and CLI
  levels.

### Policy
- **C6 (version bump policy):** `SCHEMA_VERSION` only bumps on field
  *removals*, type changes, or semantic re-interpretations — additive
  fields gated by `skip_serializing_if` do not bump. R6's spurious
  `Verdict::SCHEMA_VERSION` 2 → 3 bump (it added the optional
  `deferred_rules` field) is reverted to **2**.
  `DEFERRED_SCHEMA_VERSION` advances to **3** because B5's
  `evaluator_input` shape change is non-additive.

### Migrating to 0.2
1. Pull main and run `hector trust <path>` against every `.hector.yml`
   in your repo — the trust fingerprint algorithm changed (C1).
2. If you have CI that asserts `Verdict::schema_version == 3`, accept
   `>= 2` instead (C6 reverts the spurious bump). The deferred-envelope
   `schema_version` is now `3`.
3. If your CI parses the deferred envelope, update for v3:
   - `payload.warnings` is now present (B4)
   - `payload.evaluator_input` is per-rule (B5)
   - prompt sentinel tags carry a per-call random suffix (C5)
4. If you script around exit codes, add a case for **3** (engine
   internal error, B7). Default to allow; opt into fail-closed via
   `HECTOR_FAIL_CLOSED_ON_INTERNAL=1`.

### Hook output + capability warning quieted (R7)

- Claude Code adapter hook emits exactly one block message per block — verdict JSON on stderr — confirmed by piping a synthesized `PostToolUse` event through `adapters/claude-code/hooks/hook.sh`. The doubled `PostToolUse:Edit hook returned blocking error` headers seen in the audit transcript came from a second plugin (`bully`) installed alongside `hector` in the same Claude Code session, not from Hector emitting twice. No Hector-side change required for this half.
- macOS "capability enforcement is best-effort" advisory is no longer printed from `engine::capability::run_best_effort_macos` on every script-rule run. Routine `hector check` invocations now keep stderr empty on macOS, both from a terminal and through the adapter hook (which spawns ~3 hector processes per edit, bypassing the per-process dedup landed in `f47ef82`).
- The platform-capability story moves to a new `capabilities` doctor row (`hector doctor`): `pass` on Linux (CLONE_NEWNET enforces `network: false`); `warn` on macOS and other non-Linux targets with a `docs/security.md` pointer. Library helper `hector_core::engine::capability::platform_capability_status()` is the single source of truth.
- Doctor JSON shape stays additive: new `capabilities` row lands between `engines` and `adapter`. Schema is additive-only per `docs/doctor.md`.

### `hector init` — workspace & linter detection (R1)

- Scaffolds scopes from the detected workspace shape (`pnpm-workspace.yaml`, `package.json` `workspaces`, Cargo `[workspace] members`, `go.work`). Single-package repos still get `src/**/*.<ext>`; monorepos get per-workspace globs like `apps/**/src/**/*.ts`.
- Detects existing linters (biome, eslint, ruff) and:
  - Skips grep rules that would duplicate the linter (e.g. no `no-console-log` when biome is configured).
  - Scaffolds a passthrough wrapper rule (`biome-check` / `eslint-check`) instead, using the project's package-manager exec command (`pnpm exec` / `yarn exec` / `npx`).
- Always appends a commented-out `llm:` block + example semantic rule so the subagent path is discoverable without docs.
- No flags added — detection is automatic.
- `init.rs` split into a submodule (`commands/init/{mod.rs, detect.rs}`) so detection is testable in isolation.

### Verdict — surface deferred semantic rules on blocked verdicts (R6)

- `Verdict` now carries an optional `deferred_rules: Vec<DeferredRuleRef>` field listing rules that would have been evaluated by the subagent path but were suppressed because a deterministic rule blocked the edit first. Each entry is `{ rule_id, severity, reason }`. Closes the "silently dropped my semantic rule" gap surfaced by the first-run audit (transcript 2).
- `Verdict::SCHEMA_VERSION` bumped `2 → 3` (additive field; envelopes without `deferred_rules` are byte-compatible via `skip_serializing_if = "Vec::is_empty"`).
- Claude Code interpreter skill (`adapters/claude-code/skills/hector/SKILL.md`) now surfaces deferred rules in its block summary so users see that their configured semantic rules are alive even when not evaluated this turn.

### LLM config — surface cleanup for `claude-code-subagent` (R2 + R5)

- `llm.model` is now optional when `provider == claude-code-subagent`. Previously it was required-but-ignored. If set, hector emits a one-time stderr warning per process noting that the subagent uses the Claude Code session's model.
- New optional `llm.evaluator_model: <model-id>` propagates through the `DeferredVerdict` payload so the Claude Code interpreter skill can dispatch the `hector-evaluator` subagent under a specific model (e.g. `haiku` for cheap policy checks). When unset, the subagent's frontmatter `model:` is used. Today Claude Code's subagent dispatch does not accept a per-call model override; the skill surfaces the requested value as an advisory pointing the user at the subagent's frontmatter file. If/when Claude Code adds inline overrides, the skill will pass the value through directly.
- `DEFERRED_SCHEMA_VERSION` bumped to `2` to reflect the new optional payload field. Envelopes without `evaluator_model` are byte-compatible with the prior shape (`skip_serializing_if = "Option::is_none"`).
- **Library-additive only** for direct-API providers (anthropic / openrouter / ollama). Their `model` field stays required.

### Adapters — skip self-check of policy files (R3)

- Both adapters (`adapters/claude-code/hooks/hook.sh`, `adapters/opencode/src/index.ts`) now exit 0 without invoking `hector` when the changed file is `.hector.yml` or `.bully.yml`. Editing the policy file itself no longer fires the trust gate mid-edit and surfaces a misleading "internal error" to the user.
- Match is by basename, so absolute paths work too.

### Script engine — `output:` default flipped to `passthrough` (R4)

- **Breaking (config):** Per-rule `output:` field default changes from `parsed` → `passthrough`. Existing configs that depended on parsed-mode violation extraction must now set `output: parsed` explicitly. The set of supported parsed formats does not grow — we will not chase a parser per tool.
- Rationale: first real-world test (2026-05-23) showed `parsed` mis-handling biome's pretty diagnostic frame as a chain of false violations. Bully's design is passthrough; we match it.
- `hector init` scaffold no longer emits `output: parsed`.

### Subagent semantic-eval — deferred-payload path (H1)

- New CLI flag `hector check --emit-semantic-payload` and new config value `llm.provider: claude-code-subagent`. When either is active, `engine: semantic` and `engine: session` rules are collected into a `DeferredVerdict` JSON envelope on stdout instead of being dispatched to the configured LLM. The envelope is byte-compatible with bully's `additionalContext` payload — the Claude Code adapter (H3, separate plan) wraps it for in-session subagent dispatch.
- Exit code semantics unchanged: deterministic block → 2 (deferred suppressed); pass + envelope → 0; pass + no envelope → 0.
- New module `hector_core::verdict_deferred` exposes `DeferredVerdict`, `DeferredPayload`, `DeferredRule`, and `DEFERRED_SCHEMA_VERSION` (independent of `Verdict::SCHEMA_VERSION`).
- New helper `hector_core::llm::prompt::build_evaluator_input(rules, primary, context)` — concatenates the (system, user) tuple from `build_prompt_split` for inclusion in the envelope's `_evaluator_input` field.
- Wire format was documented in `docs/emit-semantic-payload.md` (removed with the LLM-eval feature).
- **Library-additive only.** No `Verdict` change, no exit-code change. Existing direct-API users (anthropic / openrouter / ollama) are unaffected.

### Subagent semantic-eval — `hector record-verdict` (H2)

- New CLI subcommand `hector record-verdict --rule <id> --verdict <pass|violation> [--file <path>] [--dir <path>]`. Appends one `LogEntry::SemanticVerdict` record to `.hector/log.jsonl` so subagent-evaluated rules show up in coverage reports. Consumed by the Claude Code adapter's interpreter skill (H3, separate plan).
- `--verdict` is a clap `ValueEnum`; invalid values are rejected at parse time.
- First invocation against a fresh log lazily stamps a `session_init` record so the log starts with the canonical first-record type.
- Exit codes: `0` success, `1` telemetry write failure. Never `2` — `record-verdict` is not a gate.
- Wire format and trust model were documented in `docs/record-verdict.md` (removed with the LLM-eval feature).
- **Library-additive only.** No new core surface; reuses `hector_core::telemetry::{append, LogEntry::SemanticVerdict}` shipped in D1.

### Subagent semantic-eval — Claude Code adapter mode (H3)

- New Claude Code adapter mode activated by `llm.provider: claude-code-subagent` in `.hector.yml`. The `PostToolUse` hook routes through `hector check --emit-semantic-payload` (H1) and wraps the resulting `DeferredVerdict` in Claude Code's `hookSpecificOutput.additionalContext` envelope, preamble `AGENTIC LINT SEMANTIC EVALUATION REQUIRED:`. Restores bully's in-session subagent path for Claude Code subscription users — no `ANTHROPIC_API_KEY` required.
- New interpreter skill `adapters/claude-code/skills/hector/SKILL.md` activates on the preamble, judges short single-rule payloads inline, dispatches the `hector-evaluator` subagent for everything else, applies error-severity fixes via `Edit`, and records each rule's verdict through `hector record-verdict` (H2) so coverage telemetry remains accurate.
- New subagent definition `adapters/claude-code/agents/hector-evaluator.md` — read-only, returns `VIOLATIONS:` / `NO_VIOLATIONS:` text, no `Read`/`Grep`/`Glob` tools.
- Direct-API mode (anthropic / openrouter / ollama) is unchanged — the hook only diverges when `.llm.provider == "claude-code-subagent"`.
- Plugin version bumped 0.1.0 → 0.2.0.
- Adapter README documents both modes and the `model:` placeholder requirement.

### Script engine — `output: parsed | passthrough` (E2)

- New per-rule `output:` field on `Rule`. `Parsed` (default) feeds the chosen stream through `engine::output::parse`, which extracts `file:line:col: msg` structure from canonical lint output (clippy `--message-format short`, `ruff`, `eslint --format compact`) and the `grep -n` `<line>:<text>` shape — populating `Violation.line` / `Violation.column`. `Passthrough` preserves the 0.1 behaviour: stdout+stderr land verbatim in `message` with `line: None`.
- Parsed mode emits one `Violation` per record, so a multi-hit lint run no longer collapses into a single concatenated message.
- **Breaking (library):** `engine::script::run_script_rule` now returns `Result<Vec<Violation>>` (was `Result<Option<Violation>>`). The trait impl was already vec-shaped; only direct callers of the free function change.
- New parser guard: `file:line: msg` mode now requires a path separator in the file capture, so `example.com:42: msg` and `grep -n` `<line>:<text>` no longer mis-parse as `{ file: "example.com", line: 42 }`. Windows drive paths (`C:\foo.rs:14:5: msg`) parse correctly.

### OpenCode adapter — pre-flight gating

- The adapter now hooks `tool.execute.before` (was `.after`) and shadow-writes the proposed file content before invoking `hector check --file`, then restores the pre-edit state regardless of verdict. A `block` verdict throws so opencode never executes the tool — previously the write had already landed before hector saw it.
- `tool.execute.after` is still used for `hector session record` (best-effort cross-edit tracking).
- Late-init fix: hooks register unconditionally and re-check `.hector.yml` per invocation, so `hector init` mid-session starts gating without an opencode restart.
- Recognises opencode's native `find` / `replace` / `replaceAll` edit-arg shape (with legacy `oldString` / `newString` as fallback for older opencode versions).
- Module exposes both `default` and named `HectorPlugin` exports so neither loader pattern silently no-ops.

### Capability sandbox — macOS warning dedup

- The "capability enforcement is best-effort on this platform" stderr line now fires at most once per process (was: once per script rule invocation). Extracted into a testable `should_warn_macos_with` helper.

### Telemetry — typed records (D1)

- `.hector/log.jsonl` now carries typed records: `session_init`, `check`, `semantic_verdict`, `semantic_skipped`. Each line has a `type` discriminator. Per-rule outcomes (`PerRuleRecord`) are nested under `Check.rules` instead of being one-line-per-(file,rule). `hector_version` and a telemetry `schema_version` are stamped in every `session_init`.
- **Backwards compat:** `hector_core::telemetry::read_all` accepts the pre-D1 flat shape via an untagged fallback and lifts each line into the closest typed variant. A one-time stderr deprecation warning fires per process when the fallback is used. The fallback will be removed at the 0.3 verdict freeze.
- New CLI subcommand `hector session start` stamps a `session_init` record explicitly. `hector session record` stamps one lazily on its first invocation per session.
- **Breaking (library):** `pub enum LogEntry` replaces `pub struct LogEntry` in `hector_core::telemetry`. Pre-1.0; consumers using the writer should migrate to constructing the appropriate variant.
- Wire format documented in [`docs/operating/telemetry.md`](docs/operating/telemetry.md).

## 0.1b — Engine set complete

### Engines
- `ast`: structural pattern matching via `ast-grep-core`. Rules specify `pattern:` and `language:`.
- `semantic`: LLM-evaluated plain-English rules. Requires an `llm:` block. Anthropic provider only at 0.1b.
- `session`: cumulative-changeset rules fired by `hector check --session`. Useful for "auth changed but no tests" type rules.

### Commands
- `hector init`: detect stack, scaffold a starter `.hector.yml`.
- `hector migrate`: rewrite `.bully.yml` → `.hector.yml`.
- `hector baseline`: record current violations, silence them from future runs.
- `hector check --session`: evaluate session rules and clear `.hector/session.json`.

### Internals
- `RuleEngine` trait for unified engine dispatch.
- `LlmClient` trait + `AnthropicClient` impl.
- `HectorEngine::builder()` to inject LLM dependencies.
- `HectorEngine::check` returns `Result<Verdict>` (engine errors surface as `engine: trust` violations).
- Telemetry log at `.hector/log.jsonl`.

### Preflight fixes from 0.1a review
- Configs with unimplemented engines fail at load time (no silent passes).
- Invalid scope globs fail at load time.
- `// hector-disable:` comments now silence violations when line numbers are present.
- `--diff` mode plumbs the diff through to script rules.
- `.bully.yml` configs print a deprecation warning.

## Coming in 0.1c / Plan C
- Claude Code adapter (plugin.json, PostToolUse + Stop hooks, skills ported from bully).
- `CheckInput::Staged` (git index).
- Full repo-context expansion.
