# Hector — Plan & Phase 0.1 Design

**Status:** Draft v0.1
**Date:** 2026-05-11
**Owner:** dynamik-dev
**Companion to:** [`overview.md`](./overview.md)
**Supersedes:** overview.md §4 (architecture), §5 (schema), §10 (phasing), §11 (open questions) — see "Decisions resolved" below.

---

## 1. Summary

This document locks the open decisions from `overview.md`, sequences the work from 0.1 to 1.0, and details the 0.1 implementation. Read `overview.md` first for context (what Hector is and why).

`overview.md` describes Hector at 1.0. This document describes how we get there, and what ships at 0.1 specifically.

## 2. Decisions resolved

The overview spec was a v0.1 draft with omissions. Resolved during brainstorming:

| Topic | overview.md said | Resolved as |
|---|---|---|
| `ast` engine (ast-grep) | Not mentioned | **Ships at 0.1.** Core feature parity with bully. |
| `session` engine (cumulative changeset) | Not mentioned | **Ships at 0.1.** Core feature parity with bully. |
| Trust + per-rule capabilities | Not mentioned | **Ships at 0.1.** Non-negotiable — we ship arbitrary shell exec, we ship the gate. |
| Context scope (`diff \| file \| repo`) | Open Q #2 | **Keep all three.** Default `diff`. |
| Subagent for semantic | Open Q #5 | **Removed.** Direct LLM API call. Bench parity before 0.2. |
| Mojo bindings | Open Q #6 | **Compatible future, not on roadmap.** `LlmClient` trait keeps the door open. |

Open questions deferred past 0.1: credentials in CI (#1, gates 0.3 pre-commit work), semantic verdict caching (#3, additive), rule packs / registry (#4, post-1.0).

## 3. Phasing

| Phase | Theme | Deliverables |
|-------|-------|--------------|
| **0.1** | Bully parity, in Rust | `hector-core` crate + `hector` binary. Engines: `script`, `ast`, `semantic`, `session`. Trust + capabilities. Schema v2 with `context: diff \| file \| repo`. `.bully.yml` reads with deprecation warning. Anthropic provider only. Claude Code adapter at parity (PostToolUse + Stop, skills, no subagent). Commands: `check`, `validate`, `init`, `migrate`, `trust`, `baseline`. JSON verdict locked-but-unstable. |
| **0.2** | Provider + tool-agnostic | OpenAI provider. Aider adapter (config-only). pre-commit adapter. `hector doctor`. Telemetry/baseline parity with bully; review tooling. |
| **0.3** | Universal adapter + watch | MCP server (`hector serve --mcp`). `hector watch` daemon. JSON verdict frozen. CI/credentials story (overview.md open Q #1). |
| **0.4** | Provider breadth + distribution | Gemini + Ollama providers. cargo-dist pipeline. Homebrew tap, npm/pip postinstall shims, install.sh. |
| **1.0** | Stable + documented | Verdict contract guaranteed stable across minor versions. Docs site at hector.dev. Marketplace rule-packs design (impl deferred). |

Cross-cutting at 0.1: the verdict JSON locks early so 0.2+ only *consumes* it; the engine trait accepts all four engines from day one so we don't refactor at 0.3 when MCP needs to surface session results.

## 4. Crate / module layout (0.1)

```
hector/                        ← repo root
├── Cargo.toml                 ← workspace
├── crates/
│   ├── hector-core/           ← library
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── config/        ← parser, scope match, extends, schema v1+v2
│   │   │   ├── engine/        ← Engine trait + script/ast/semantic/session impls
│   │   │   ├── diff/          ← unified-diff parser, line mapping
│   │   │   ├── verdict.rs     ← Verdict, Violation, Status, JSON schema
│   │   │   ├── trust.rs       ← fingerprint, capability gate
│   │   │   ├── baseline.rs
│   │   │   ├── telemetry.rs
│   │   │   ├── disable.rs     ← `hector-disable:` comment scanner
│   │   │   └── llm/           ← LlmClient trait + anthropic impl
│   │   └── tests/
│   ├── hector-cli/            ← binary; thin
│   │   └── src/main.rs
│   └── hector-ast-grep/       ← thin wrapper (linked-crate, fallback to shell)
├── adapters/
│   └── claude-code/
│       ├── plugin.json
│       ├── hooks/
│       │   ├── post_tool_use.sh
│       │   └── stop.sh
│       └── skills/
│           ├── hector-init/
│           ├── hector-author/
│           └── hector-review/
├── specs/                     ← design docs
├── docs/                      ← user-facing (rule authoring, telemetry, migration)
└── examples/                  ← .hector.yml samples per stack
```

**Two notable choices:**

- **Workspace, not single crate.** Keeps the CLI thin and the library reusable (future MCP server binary, future third-party SDKs).
- **ast-grep: link the crate.** Use `ast_grep_core` directly to avoid subprocess overhead and PATH dependency. Wrapper crate isolates the dependency so we can swap to shelling out if the public API proves unstable (see §13 risks).

## 5. Public API surface — `hector-core`

```rust
pub struct Engine {
    config: ResolvedConfig,
    llm: Box<dyn LlmClient>,
}

impl Engine {
    pub fn load(config_path: &Path) -> Result<Self>;
    pub fn check(&self, input: CheckInput) -> Verdict;
    pub fn check_session(&self, changeset: &Changeset) -> Verdict;
}

pub enum CheckInput {
    Diff { file: PathBuf, unified_diff: String },
    File { path: PathBuf, content: String },
    Staged,
}

pub struct Changeset {
    pub edits: Vec<EditRecord>,            // accumulated per-edit
    pub root: PathBuf,
}

pub struct Verdict {
    pub schema_version: u32,
    pub hector_version: String,
    pub status: Status,                    // Pass | Warn | Block
    pub violations: Vec<Violation>,
    pub passed_checks: Vec<String>,
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

Built-in `LlmClient` impl at 0.1: `AnthropicClient`. Third parties implement the trait in their own crate and inject at construction.

## 6. Schema v2 (final)

`.hector.yml`:

```yaml
schema_version: 2

# Required only if any rule has engine: semantic or engine: session.
llm:
  provider: anthropic              # anthropic | openai | gemini | ollama
  model: claude-sonnet-4-6
  api_key_env: ANTHROPIC_API_KEY
  base_url: null

extends:
  - ../shared/hector-base.yml

# Generated by `hector trust`. Mismatch blocks all runs.
trust:
  fingerprint: "sha256:abc123..."

rules:
  no-as-any:
    description: "No `as any` casts. Use a precise type or `unknown` plus narrowing."
    engine: ast                    # script | ast | semantic | session
    scope: ["src/**/*.ts"]
    severity: error                # error | warning
    pattern: "$EXPR as any"        # ast-grep, ast engine only
    language: ts                   # optional; inferred from file ext
    capabilities:                  # default: network: false, writes: none
      network: false
      writes: none

  test-coverage-on-auth:
    description: "Auth changes need test changes in the same session."
    engine: session                # fires at session end over cumulative changeset
    scope: ["src/auth/**"]
    severity: error
    context: repo                  # diff | file | repo — semantic/session only

  no-console-log:
    description: "No console.log in committed source."
    engine: script
    scope: ["src/**/*.ts"]
    severity: error
    script: "grep -nE 'console\\.log\\(' {file} && exit 1 || exit 0"
    capabilities:
      network: false
      writes: cwd-only
```

**Fields by engine:**

| Field | `script` | `ast` | `semantic` | `session` |
|-------|---------|------|-----------|-----------|
| `description` | required | required | required (also the prompt) | required (also the prompt) |
| `scope` | required | required | required | required |
| `severity` | required | required | required | required |
| `script` | required | — | — | — |
| `pattern` | — | required | — | — |
| `language` | — | optional | — | — |
| `context` | — | — | optional (default `diff`) | optional (default `repo`) |
| `capabilities` | optional (default deny) | optional (default deny) | n/a | n/a |
| `fix_hint` | optional | optional | optional | optional |

**Backward compat:** `.bully.yml` files at `schema_version: 1` parse unchanged. A deprecation warning prints once per run suggesting `hector migrate`. Schema-v1 rules at engine `script`, `ast`, `semantic`, `session` all map directly. `capabilities` block is absent on v1 rules — defaults apply (deny).

## 7. Trust + capability gate

**Trust file format.** `trust:` is embedded in `.hector.yml`, not a sidecar. The fingerprint covers the canonicalized YAML body minus the `trust:` block itself.

**Canonicalization** for fingerprinting:

1. Parse YAML.
2. Strip `trust:` block.
3. Strip comments.
4. Sort keys recursively.
5. Re-serialize as canonical YAML (block style, no anchors, no aliases).
6. `sha256(canonical_bytes)`.

`hector trust` writes the fingerprint back into `.hector.yml`. Every other command re-computes and compares; mismatch → exit 1 with message: `"config changed since last trust — review changes and run 'hector trust' to acknowledge"`.

A missing or empty `trust:` block is treated as a mismatch (i.e. untrusted). `hector init` does **not** auto-trust — the user reviews the scaffolded config and runs `hector trust` explicitly. First `hector check` after `init` will fail with the trust-mismatch message; this is intentional friction.

**Capability gate.** Per-rule `capabilities`:

| Field | Values | Default | Enforcement |
|-------|--------|---------|-------------|
| `network` | `false`, `true` | `false` | Linux: child process in new netns. macOS: best-effort (PF route blackhole if available; otherwise log and continue — documented gap). |
| `writes` | `none`, `cwd-only`, `tmp`, `unrestricted` | `none` | Linux: bind-mount cwd RO unless `cwd-only` (RW). macOS: open syscalls intercepted via launchd sandbox profile where possible; best-effort. |

Capability violations are themselves `Violation` records with `rule_id: "<rule>__capability"`, `engine: "trust"`, never panics.

`hector check` failing the trust check exits 1 (internal error), not 2 (block). Distinct from rule violations.

## 8. Verdict JSON contract

Locked at 0.1, additive-only changes pre-1.0, frozen at 0.3 (when MCP and external adapters land).

```json
{
  "schema_version": 1,
  "hector_version": "0.1.0",
  "status": "block",
  "violations": [
    {
      "rule_id": "no-console-log",
      "severity": "error",
      "engine": "script",
      "file": "src/app.ts",
      "line": 42,
      "column": null,
      "message": "console.log not permitted in src/",
      "suggestion": null,
      "context": null
    }
  ],
  "passed_checks": ["no-as-any", "test-coverage-on-auth"],
  "elapsed_ms": 1340
}
```

`status`: `"pass" | "warn" | "block"`.

**Stability rules pre-1.0:**

- New top-level fields: allowed (additive).
- New violation fields: allowed (additive).
- Renames / removals / type changes: bump `schema_version`.

`passed_checks` is the bully convention — gives the LLM credit for what it didn't violate, shaping future behavior.

## 9. Engines

### 9.1 `script` engine

Shell-out, exit-code interpretation. `{file}` is substituted; unified diff piped on stdin. Exit 0 = pass; non-zero = violation; stderr captured and surfaced in `message`. Wrapped by capability gate (§7).

### 9.2 `ast` engine

`ast_grep_core` invoked in-process. `pattern` parsed by the matched file's language (`language:` field overrides inference). Each match emits a separate `Violation` (one rule fires N times → N violations); `line` is the match start, `column` is set when ast-grep supplies it.

Fallback if `ast_grep_core` API proves unstable: shell out to `ast-grep --pattern` and parse JSON output. The `hector-ast-grep` wrapper crate isolates this swap.

### 9.3 `semantic` engine

Direct call to `LlmClient::evaluate`. Prompt assembled from:
- The rule's `description` (= the policy).
- Context: unified diff (`context: diff`), or full file content (`context: file`), or relevant repo slice (`context: repo` — file + 2-hop import neighbors, bounded at ~32k tokens).
- `passed_checks` so far this run.

No tool-use, no agent loop. Rules are batched per `(provider, model, context_value)` triple — one model call per batch. Prompt construction logic and bench harness ported from `bully/src/bully/semantic/`.

### 9.4 `session` engine

Fires at session end (driven by adapter — Stop hook for Claude Code). Sees the accumulated changeset since session start, not a single diff.

Changeset persistence: `.hector/session.json`, written per PostToolUse invocation. Schema:

```json
{
  "session_id": "<adapter-supplied>",
  "started_at": "2026-05-11T18:00:00Z",
  "edits": [
    { "file": "src/auth.ts", "diff": "...", "timestamp": "..." }
  ]
}
```

`hector check --session` reads `.hector/session.json`, evaluates session rules over the aggregated diff, clears state. Session rules default to `context: repo` (a single edit rarely tells the full story).

## 10. CLI commands at 0.1

| Command | Purpose |
|---------|---------|
| `hector check` | Run pipeline. Flags: `--file`, `--diff <path>`, `--staged`, `--session`, `--rule <id>`, `--print-prompt`, `--format human\|json`. |
| `hector lint <path>` | Sugar for `check --file <path>`. |
| `hector init` | Detect stack, scaffold `.hector.yml`. |
| `hector validate` | Parse + enum-check config; resolve `extends:`. |
| `hector trust` | Compute fingerprint; write into `.hector.yml`. |
| `hector baseline` | Record current violations to `.hector/baseline.json`. |
| `hector migrate` | Rewrite `.bully.yml` → `.hector.yml`, bump schema, prompt for `llm:` block, move `.bully/` → `.hector/`. |

**Exit codes (stable contract):**

| Code | Meaning |
|------|---------|
| `0` | Pass — no violations, or only warnings |
| `1` | Internal error — config invalid, trust mismatch, LLM unreachable |
| `2` | Blocking violation — ≥1 `error`-severity violation found |

Deferred past 0.1: `hector watch` (0.3), `hector serve --mcp` (0.3), `hector doctor` (0.2), `hector adapter <name> install` (0.2).

## 11. Claude Code adapter at 0.1

**Files:**

- `adapters/claude-code/plugin.json` — Claude Code plugin manifest.
- `adapters/claude-code/hooks/post_tool_use.sh` — wraps `hector check --diff <stdin> --format json`. Exit 2 on block, 0 otherwise. Stderr → user-visible message.
- `adapters/claude-code/hooks/stop.sh` — wraps `hector check --session --format json`. Same exit-code mapping.
- `adapters/claude-code/skills/hector-init/` — ported from `bully-init`.
- `adapters/claude-code/skills/hector-author/` — ported from `bully-author`.
- `adapters/claude-code/skills/hector-review/` — ported from `bully-review`.

**Subagent removed.** Bully's `bully-evaluator` subagent provided context isolation (no `Read`/`Grep`/`Glob`, only sees the diff). At Hector, semantic evaluation goes directly through the configured `LlmClient`. The trade-off (per overview.md open Q #5):

- **Lost:** Claude Code's subagent isolation; arbitrarily-tooled subagent context.
- **Gained:** ~30–60% token reduction (no agent scaffolding), provider portability, lower latency.

Bench parity before 0.2: run a 10–20 rule fixture set in both implementations, compare verdict agreement and token cost. Document in `docs/bench/subagent-removal.md`.

## 12. Migration from bully

Per overview.md §9, plus the following implementation details:

1. `hector adapter claude-code install` — defer to 0.2 (the command). At 0.1, ship install instructions in README + `plugin.json` ready for `/plugin install`.
2. `hector validate` accepts `.bully.yml` with deprecation log line on first parse.
3. `hector migrate`:
    - Reads `.bully.yml`, rewrites as `.hector.yml` (rename only, schema_version 1 → 2).
    - Prompts for `llm:` block — provider/model/api_key_env. Skippable if no semantic rules exist.
    - Inserts `trust:` block with computed fingerprint.
    - Moves `.bully/log.jsonl` → `.hector/log.jsonl`, `.bully/baseline.json` → `.hector/baseline.json`, `.bully/session.json` → `.hector/session.json` if present.
    - Leaves `.bully.yml` in place by default, optionally deletes with `--clean`.
4. Skill name redirects: `/bully-init` → `/hector-init` etc. — best-effort within Claude Code adapter, one-line deprecation log. Drop after 0.4.

Rule semantics preserved exactly. User-visible config change is cosmetic.

## 13. Testing strategy

**Test pyramid:**

- **Unit (`hector-core`)** — config parser (every YAML edge case from bully's corpus), scope matcher, diff parser, capability gate, trust fingerprinting, disable-comment scanner.
- **Engine fixtures** — table-driven: `(rule, file, diff) → expected_verdict`. Port bully's fixtures verbatim so parity is observable.
- **Golden JSON** — snapshot each `hector check` against a small fixture repo; PRs fail on unintended verdict drift.
- **Anthropic LlmClient** — `wiremock-rs` for HTTP stubbing in CI. One nightly integration test against real API gated on `HECTOR_NIGHTLY=1`.
- **Claude Code adapter** — shell-based: stub `hector` with canned JSON, drive hook scripts, assert exit code + stderr.
- **Migration** — every `.bully.yml` fixture parses; round-trip through `hector migrate` produces semantically-equivalent `.hector.yml`.

**Bench parity (gates 0.2 graduation):**

- Port `bully/bench` harness.
- Run 10–20 rule fixture set on bully vs. Hector.
- Verdict agreement ≥ 95% on identical inputs.
- Document divergences; explain or fix.

## 14. Sequencing inside 0.1

Rough ordering; implementation plan refines with explicit dependencies.

| Step | Work |
|------|------|
| A | Workspace skeleton + `Verdict` types + golden JSON snapshots |
| B | Config parser (schema v1 + v2 + `extends:`) — port bully's parser logic |
| C | Script engine + capability gate |
| D | Trust system — fingerprint, gate, `hector trust` |
| E | Diff parser + scope matching |
| F | AST engine (link `ast_grep_core`) |
| G | Anthropic `LlmClient` (wiremock-stubbed) |
| H | Semantic engine — `diff` / `file` / `repo` context expansion |
| I | Session engine — changeset persistence, end-of-session evaluation |
| J | CLI surface — check, validate, init, migrate, trust, baseline |
| K | Claude Code adapter — plugin.json, hooks, skills |
| L | Migration UX polish — `hector migrate`, redirects, deprecation log |

Parallelizable: F/G after E. I depends on H (shared prompt infra). K depends on J + I.

## 15. Risks

| Risk | Mitigation |
|------|-----------|
| `ast_grep_core` Rust API is unstable / private | `hector-ast-grep` wrapper isolates dep. Fall back to shelling out to `ast-grep` CLI if blocked at step F. Spike step F first. |
| macOS capability sandboxing weaker than Linux | Document gap in `docs/security.md`. Ship Linux strict, macOS best-effort. Re-evaluate at 1.0. |
| Semantic prompt parity with bully | Copy prompts byte-for-byte at first. Bench gate at 0.2. Optimize after parity proven. |
| Subagent removal degrades semantic quality | Bench-gate at 0.2 with 95% agreement threshold. Roll back to subagent-style isolation (via prompt scaffolding) if needed — cheaper than re-introducing the agent loop. |
| Session-state IO contention on rapid edits | Atomic write via tempfile-then-rename. Document the per-edit cost. If it bites, batch flush at Stop time only. |
| Anthropic-only at 0.1 caps adoption | Acceptable for parity release. 0.2 OpenAI ships ~4 weeks after 0.1; doc it as the explicit follow-up. |

## 16. Compatible futures (out of scope, do not foreclose)

- **Mojo / MAX-backed `LlmClient`** for offline inference. `LlmClient` trait already accommodates; a third-party `hector-llm-mojo` crate is a weekend project if Modular ships stable Rust FFI.
- **Rule packs / registry** (`hector pack add react-strict`). Design namespacing in `extends:` semantics now so it's painless to add later.
- **Semantic verdict caching** keyed on `(rule_id, diff_hash, model)`. Additive; deferred until cost signals demand it.

## 17. Open questions deferred past 0.1

| # | Question | Decision deadline |
|---|----------|-------------------|
| 1 | Credentials in pre-commit / CI for semantic rules — skip+warn or `--strict`? | Before 0.3 (pre-commit adapter) |
| 3 | Semantic verdict caching on disk | When cost telemetry exists (0.2+) |
| 4 | Rule packs / registry | Post-1.0 |
