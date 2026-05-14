# Changelog

Notable changes to Hector, newest first. In-flight work lives in `plans/`.

## Unreleased

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
