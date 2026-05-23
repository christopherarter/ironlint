# Changelog

Notable changes to Hector, newest first. In-flight work lives in `plans/`.

## Unreleased

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
