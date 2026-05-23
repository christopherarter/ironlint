# Hector — Rebuild Specification

**Status:** Draft v0.1
**Date:** May 2026
**Owner:** dynamik-dev
**Replaces:** [dynamik-dev/bully](https://github.com/dynamik-dev/bully)

---

## 1. Summary

Hector is a tool-agnostic policy-enforcement pipeline for AI coding agents. Every edit produced by an agent is checked against a project-local rules file (`.hector.yml`). Violations of `error` severity block the edit; violations of `warning` severity are reported. Rules can be deterministic shell scripts or natural-language descriptions evaluated by an LLM against a unified diff.

This is a ground-up rewrite of `bully` in Rust, with host integration (currently hard-coded to Claude Code) extracted into per-host adapters.

## 2. Goals

- **Tool-agnostic core:** the engine has no knowledge of any specific coding agent.
- **Single-binary distribution:** install via `cargo`, `brew`, `npm`, `pip`, or curl-pipe-sh.
- **Backward-compatible config:** existing `.bully.yml` files parse with a deprecation warning.
- **Pluggable LLM providers:** Anthropic, OpenAI, Gemini, Ollama as first-class; trait-based for third parties.
- **Adapter ecosystem:** ship official adapters for Claude Code, Aider, MCP, and pre-commit at 1.0.

## 3. Non-goals

- Rule-authoring UI. Skills/slash-commands stay inside individual host adapters (e.g. the Claude Code adapter), not the core.
- Auto-fix. Hector reports; it does not modify code.
- Replacing language-native linters. Hector orchestrates; ruff/eslint/etc. remain underneath.
- Hosted/SaaS version. CLI only.

## 4. Architecture

Three layers. The boundary between them is the stable contract; everything else is implementation detail.

```
┌─────────────────────────────────────────────────────────┐
│ Adapters (Claude Code, Aider, MCP, pre-commit, watch)   │
│  - capture edit events from a host                       │
│  - shell out to `hector check --format json`            │
│  - translate verdict back into host response             │
└─────────────────────────────────────────────────────────┘
                          │  (JSON over stdout)
┌─────────────────────────────────────────────────────────┐
│ hector  (CLI binary, Rust)                              │
│  - argument parsing, I/O, formatting                     │
│  - exit-code mapping (0 / 1 / 2)                        │
└─────────────────────────────────────────────────────────┘
                          │  (library API)
┌─────────────────────────────────────────────────────────┐
│ hector-core  (Rust crate)                               │
│  - config loader (.hector.yml + extends)                │
│  - script engine (shell-out + exit-code interpretation) │
│  - semantic engine (LLM call against diff)              │
│  - baseline manager, telemetry, disable-comments        │
│  - LlmClient trait + built-in providers                 │
└─────────────────────────────────────────────────────────┘
```

### 4.1 `hector-core` (library)

Public surface (sketch):

```rust
pub struct Engine { config: ResolvedConfig, llm: Box<dyn LlmClient> }

impl Engine {
    pub fn load(config_path: &Path) -> Result<Self>;
    pub fn check(&self, input: CheckInput) -> Verdict;
}

pub enum CheckInput {
    Diff { file: PathBuf, unified_diff: String },
    File { path: PathBuf, content: String },
    Staged,                              // read from git index
}

pub struct Verdict {
    pub status: Status,                  // Pass | Warn | Block
    pub violations: Vec<Violation>,
    pub elapsed_ms: u64,
}

pub trait LlmClient: Send + Sync {
    fn evaluate(
        &self,
        rules: &[SemanticRule],
        diff: &str,
        context: Option<&str>,
    ) -> Result<Vec<RuleVerdict>>;
}
```

Built-in implementations: `AnthropicClient`, `OpenAiClient`, `GeminiClient`, `OllamaClient`. Third parties implement `LlmClient` in their own crate and inject at construction.

### 4.2 `hector` (CLI binary)

Thin wrapper over the core. All commands accept `--format human|json` (default: human in a TTY, JSON otherwise).

| Command                         | Purpose                                                                                     |
| ------------------------------- | ------------------------------------------------------------------------------------------- |
| `hector check`                  | Run pipeline. Flags: `--file`, `--diff <path>`, `--staged`, `--rule <id>`, `--print-prompt` |
| `hector lint <path>`            | Convenience: `check --file <path>`                                                          |
| `hector init`                   | Detect stack, scaffold `.hector.yml`                                                        |
| `hector validate`               | Parse + enum-check config; resolve `extends:`                                               |
| `hector baseline`               | Record current violations to `.hector/baseline.json`                                        |
| `hector watch [path]`           | Run as daemon, check on filesystem change                                                   |
| `hector serve --mcp`            | Expose `check_edit` as an MCP server over stdio                                             |
| `hector doctor`                 | Environment diagnostic                                                                      |
| `hector adapter <name> install` | Install an official adapter into the user's host config                                     |
| `hector migrate`                | Rewrite `.bully.yml` → `.hector.yml`                                                        |

**Exit codes** (stable contract):

| Code | Meaning                                                  |
| ---- | -------------------------------------------------------- |
| `0`  | Pass — no violations, or only warnings                   |
| `1`  | Internal error — config invalid, LLM unreachable, etc.   |
| `2`  | Blocking violation — ≥1 `error`-severity violation found |

### 4.3 Adapters

An adapter is _not_ Rust code. It is a small set of host-specific files (manifest, hook script, install instructions) that wire `hector check` into the host's edit lifecycle. Official adapters live under `adapters/<name>/` in this repo and are installed by `hector adapter <name> install`.

Every adapter does the same four things:

1. Capture an edit event from the host.
2. Construct a `CheckInput` (preferring a unified diff when available).
3. Invoke `hector check --format json`.
4. Translate the verdict into the host's expected response (exit code, error message, blocking signal).

## 5. Config schema

`.hector.yml`, schema version 2:

```yaml
schema_version: 2

# Required only if any rule has engine: semantic.
llm:
  provider: anthropic # anthropic | openai | gemini | ollama
  model: claude-sonnet-4-6
  api_key_env: ANTHROPIC_API_KEY # env var name (no secrets in config)
  base_url: null # override for proxies / Ollama

extends:
  - ../shared/hector-base.yml

rules:
  no-console-log:
    description: "No console.log in committed source — use the project logger."
    engine: script
    scope: ["src/**/*.ts", "src/**/*.tsx"]
    severity: error
    script: "grep -nE 'console\\.log\\(' {file} && exit 1 || exit 0"

  prefer-derived-state:
    description: >
      React components should not use useEffect to derive state from props.
      Compute the value directly during render (or with useMemo if expensive).
    engine: semantic
    scope: "src/**/*.tsx"
    severity: warning
    context: diff # diff | file | repo (default: diff)
```

**Backward compatibility:** `.bully.yml` files at `schema_version: 1` parse unchanged. A deprecation warning prints once per run suggesting rename + bump.

## 6. Verdict JSON contract

The interface every adapter depends on. Versioned via `schema_version`; will not break across minor versions.

```json
{
  "schema_version": 1,
  "hector_version": "1.0.0",
  "status": "block",
  "violations": [
    {
      "rule_id": "no-console-log",
      "severity": "error",
      "engine": "script",
      "file": "src/app.ts",
      "line": 42,
      "message": "console.log not permitted in src/",
      "suggested_fix": null
    }
  ],
  "elapsed_ms": 1340
}
```

`status` is the only field adapters strictly need; the rest is for surfacing details to the human or the agent.

## 7. Adapter specifications

### 7.1 Claude Code (`adapters/claude-code/`)

Recreates current Bully behavior.

- **Hook:** `PostToolUse` matcher `Edit|Write`. Bash wrapper invokes `hector check --diff <stdin> --format json`, exits `2` on block, `0` otherwise.
- **Plugin manifest:** `plugin.json` for the `/plugin install` flow.
- **Skills:** `hector-init`, `hector-author`, `hector-review` (ported from Bully).
- **Two semantic-eval paths.** Direct-API mode (default, set via `llm.provider: anthropic | openrouter | ollama`) calls the configured LLM provider directly. Subagent mode (opt-in via `llm.provider: claude-code-subagent`) routes through an in-session Claude Code subagent — required for subscription-only users since headless `claude -p` is not subscription-funded. The hook detects mode via `hector show-resolved-config` and emits a `hookSpecificOutput.additionalContext` envelope under subagent mode; the `hector` skill interprets it and dispatches the `hector-evaluator` subagent. See [`specs/2026-05-14-subagent-semantic-eval.md`](./2026-05-14-subagent-semantic-eval.md).

### 7.2 Aider (`adapters/aider/`)

Cheapest, highest-leverage adapter. Aider supports `--lint-cmd` natively and feeds lint output back to the LLM in a fix loop.

- **Install:** `hector adapter aider install` writes `lint-cmd: hector lint --format human` into the user's `.aider.conf.yml`.
- **Behavior:** Aider's existing lint loop drives iteration; Hector just emits violations with non-zero exit.
- **Code:** zero glue — just config.

### 7.3 MCP (built into the binary)

`hector serve --mcp` exposes one tool over stdio:

```
hector.check_edit(file: string, diff: string) -> Verdict
```

Any MCP-capable agent (Claude Code, Cursor, Codex CLI, Continue, …) can call it. This is the universal adapter — when no host-native hook exists, ship the MCP server.

### 7.4 Pre-commit (`adapters/precommit/`)

`.pre-commit-hooks.yaml`:

```yaml
- id: hector
  name: Hector policy check
  entry: hector check --staged
  language: system
  pass_filenames: false
```

Runs on `git commit`. Tool-agnostic by construction: catches edits from any agent, IDE, or human. The universal safety net.

### 7.5 Watch mode (built into the binary)

`hector watch [path]` runs as a daemon, debounces file events, runs the pipeline on changed files, prints JSON-lines to stdout. Users wire it into their editor of choice. No host integration required.

## 8. Distribution

Single Rust binary, multiple delivery channels, one source of truth via `cargo-dist`:

- **GitHub Releases:** Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64
- **Homebrew tap:** `brew install dynamik-dev/hector/hector`
- **npm:** `npm install -g @hector/cli` — postinstall fetches the matching binary (the ruff pattern)
- **PyPI:** `pip install hector-cli` — same wrapping pattern
- **Cargo:** `cargo install hector`
- **Install script:** `curl -fsSL https://hector.dev/install.sh | sh`

## 9. Migration from Bully

1. `hector adapter claude-code install` installs the new plugin.
2. `hector validate` accepts existing `.bully.yml`, emits a deprecation warning.
3. `hector migrate` rewrites `.bully.yml` → `.hector.yml`, bumps `schema_version`, and inserts the `llm:` block from interactive prompts.
4. Skill names renamed (`bully-init` → `hector-init`, etc.). Old skill names redirect with a one-line deprecation log for one minor version.
5. Telemetry path moves from `.bully/log.jsonl` to `.hector/log.jsonl`. Migrate command moves the file.

Rule semantics are preserved exactly; the user-visible config change is purely cosmetic.

## 10. Phasing

| Phase | Deliverables                                                                                   |
| ----- | ---------------------------------------------------------------------------------------------- |
| 0.1   | `hector-core` crate; `hector` binary with `check`, `validate`, `init`; Anthropic provider only |
| 0.2   | Claude Code adapter at parity with current Bully; OpenAI provider; baseline + telemetry ported |
| 0.3   | MCP server (`serve --mcp`); Aider adapter; pre-commit adapter                                  |
| 0.4   | Gemini + Ollama providers; `watch` mode; `migrate` command                                     |
| 1.0   | Stable verdict contract frozen; all distribution channels live; docs site at hector.dev        |

## 11. Open questions

1. **Credentials in pre-commit / CI.** Semantic rules need an API key, which may not be present in `git commit` contexts. Proposal: skip semantic rules + warn by default; `--strict` flag to hard-fail. Confirm before 0.3.

2. **Rule context scope.** Some semantic rules ("no unused exports", "consistent naming") need whole-file or repo context, not just the diff. The proposed `context: diff | file | repo` field adds complexity but is probably necessary. Decide before locking schema v2.

3. **Semantic verdict caching.** A `(rule_id, diff_hash, model)` triple is deterministic-ish. Should the core cache verdicts on disk to avoid re-evaluating on retries? Likely yes for cost, but invalidation is subtle (model versioning, prompt changes).

4. **Rule packs / registry.** Should we ship `hector pack add react-strict` for curated rule sets from a registry, or keep everything user-authored as Bully does today? Defer to post-1.0; design the namespacing now so it's not painful to add later.

5. **Subagent-removal impact.** ~~Existing Bully users may benefit from the Claude Code subagent's context isolation. Direct API calls have different cost/latency characteristics. Benchmark a representative repo (10–20 rules, mixed engines) before committing to the removal.~~ **Resolved (2026-05-23).** The `claude -p` allowance withdrawal made the subagent path the only viable option for Claude Code subscription users, which changed the math: rather than benchmarking direct-vs-subagent, both ship as user-selectable modes. See [`specs/2026-05-14-subagent-semantic-eval.md`](./2026-05-14-subagent-semantic-eval.md) and the §7.1 update above.

6. **Mojo bindings, eventually.** The trait-based LLM client means a Mojo-backed `LlmClient` could ship later if a Mojo HTTP/async story matures. Not on the roadmap; noted to avoid foreclosing it.
