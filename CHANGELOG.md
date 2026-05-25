# Changelog

Notable changes to Hector, newest first. In-flight work lives in `plans/`.

## Unreleased

### Changed
- **A1 (baseline)**: file-level violations (`line: None`) now require
  both fingerprint AND normalized body match. The prior behavior turned
  baseline into a per-file disable for passthrough script rules (the
  default since R4). v2 baselines continue to match on fingerprint
  alone during a grace period; run `hector baseline refresh` to
  upgrade. Storage schema bumped v2 → v3.

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
- Wire format documented in [`docs/emit-semantic-payload.md`](docs/emit-semantic-payload.md).
- **Library-additive only.** No `Verdict` change, no exit-code change. Existing direct-API users (anthropic / openrouter / ollama) are unaffected.

### Subagent semantic-eval — `hector record-verdict` (H2)

- New CLI subcommand `hector record-verdict --rule <id> --verdict <pass|violation> [--file <path>] [--dir <path>]`. Appends one `LogEntry::SemanticVerdict` record to `.hector/log.jsonl` so subagent-evaluated rules show up in coverage reports. Consumed by the Claude Code adapter's interpreter skill (H3, separate plan).
- `--verdict` is a clap `ValueEnum`; invalid values are rejected at parse time.
- First invocation against a fresh log lazily stamps a `session_init` record so the log starts with the canonical first-record type.
- Exit codes: `0` success, `1` telemetry write failure. Never `2` — `record-verdict` is not a gate.
- Wire format and trust model documented in [`docs/record-verdict.md`](docs/record-verdict.md).
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
- Wire format documented in [`docs/telemetry.md`](docs/telemetry.md).

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
