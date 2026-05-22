# Hector — Subagent-Mediated Semantic Eval (Subscription Parity)

**Status:** Draft v0.1
**Date:** 2026-05-14
**Owner:** dynamik-dev
**Companion to:** [`overview.md`](./overview.md), [`2026-05-12-bully-parity-closures.md`](./2026-05-12-bully-parity-closures.md)
**Walks back:** [`overview.md`](./overview.md) §7.1 ("Subagent removed"), §11.5 (the now-closed open question on subagent-removal impact).

---

## 1. Summary

Restore bully's subagent-mediated semantic-evaluation path as an opt-in mode of the Claude Code adapter. Claude Code subscription users have no usable LLM for `engine: semantic` rules today — the direct-API path Hector ships requires an `ANTHROPIC_API_KEY` they don't have, and the headless `claude -p` workaround was withdrawn from subscription allowances. Bully solved this by having the PostToolUse hook emit a payload in Claude Code's `hookSpecificOutput.additionalContext`, then dispatching an in-session subagent to evaluate it — subagent tokens bill against the parent session's subscription. This spec ports that mechanism into Hector while keeping the direct-API path the default for API-key users.

Core stays provider-agnostic. The subagent path lives entirely in the Claude Code adapter, with two small core-side additions (a flag and a subcommand) that the adapter consumes.

## 2. Non-goals

- **No change to the direct-API path.** Users with `ANTHROPIC_API_KEY` (or OpenRouter, or Ollama) keep the existing behavior — same prompt, same wire format, same latency profile. The subagent path is selected per-adapter, opt-in.
- **No new providers.** The subagent path is Claude-Code-specific by construction. Aider, pre-commit, and MCP adapters are not affected; they continue to require an API key.
- **No auto-fix.** The subagent returns verdicts; the parent session applies fixes via its own `Edit` tool (same as bully). Hector still does not modify code.
- **No verdict-schema break.** The shape Hector emits to adapters is unchanged. The new payload travels in a new field on the existing JSON, additive.
- **No exit-code change.** `0 / 1 / 2` stays. The new deferred-semantic-payload path always exits `0` (deferred eval is not a block).

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Claude Code session (parent agent)                              │
│                                                                 │
│  1. Edit/Write tool call                                        │
│  2. PostToolUse hook fires → adapter hook.sh                    │
│  3. hook.sh: `hector check --file X --emit-semantic-payload`    │
│                                                                 │
│     → core runs script + AST as today                           │
│     → if any block:    emit standard verdict, exit 2            │
│     → if semantic left: emit payload JSON, exit 0               │
│     → if nothing left: emit standard verdict, exit 0            │
│                                                                 │
│  4. hook.sh wraps payload in hookSpecificOutput.additionalContext│
│                                                                 │
│  5. Next turn: `hector` skill activates on the                  │
│     "AGENTIC LINT SEMANTIC EVALUATION REQUIRED" preamble        │
│                                                                 │
│  6. Skill dispatches `hector-evaluator` subagent with the       │
│     `_evaluator_input` string (or inline-judges single-rule     │
│     short-diff payloads)                                        │
│                                                                 │
│  7. Subagent returns VIOLATIONS / NO_VIOLATIONS text            │
│                                                                 │
│  8. Skill: error-severity violations → `Edit` to fix;           │
│           every verdict → `hector record-verdict ...` for       │
│           telemetry parity with the direct-API path             │
└─────────────────────────────────────────────────────────────────┘
```

Two core-side additions, three adapter-side additions, and a docs sweep.

## 4. Decisions to ratify

| Topic | Default proposed | Alternative |
|---|---|---|
| Mode selection | New value `provider: claude-code-subagent` in `.hector.yml`'s `llm:` block — explicit, visible in config diffs, fingerprint-covered. | Auto-detect on missing API key + Anthropic provider. *Rejected — hides billing-mode intent and surprises CI runs that legitimately want hard-fail on missing creds.* |
| Payload shape | Mirror bully's `additionalContext` JSON byte-for-byte (`file`, `diff`, `passed_checks`, `evaluate`, `_evaluator_input`). | Hector-native shape. *Rejected — bully's skill is the reference implementation; matching shape lets us port the skill text near-verbatim.* |
| `record-verdict` auth | None — local-only convenience wrapper around appending a `LogEntry::SemanticVerdict` to `.hector/log.jsonl`. | HMAC-signed tokens. *Rejected — an in-session agent already has shell access to the log file; auth is theater.* |
| Inline vs dispatch threshold | Skill-side, mirroring bully: inline if single rule AND diff ≤ 15 lines; dispatch otherwise. | Always dispatch. *Rejected — bully tuned this empirically; inline saves a subagent invocation on trivial diffs.* |

These should be confirmed in the corresponding plan doc; this spec records the intended direction so per-feature sessions don't re-litigate.

---

## H1. `hector check --emit-semantic-payload` (core) 🔴 critical

**Bully reference**
- `src/bully/pipeline/pipeline.py` runs deterministic checks, collects `passed_checks`, then constructs the additionalContext payload (`bully/pipeline/semantic.py:build_payload`). The hook prints this payload as a Claude Code `hookSpecificOutput` JSON envelope.

**Hector current state**
- `crates/hector-cli/src/commands/check.rs` runs the full check (script + AST + semantic + session) and emits a `Verdict` JSON. There is no way to ask Hector "run deterministic only and hand me the would-be-semantic payload."

**Why it matters**
- Without this, the adapter would have to re-implement scope-matching, skip-pattern logic, and rule selection in shell to derive what bully's `build_payload` produces in Python. That re-implementation would drift from core behavior. The flag pushes the policy into one place.

**Proposed design**
1. Recognize a new `llm.provider` value: `claude-code-subagent`. Add it to `crates/hector-core/src/llm/mod.rs::build_from_config`'s match. The arm returns `Ok(None)` with no stderr warning (unlike the API-key-missing path); the runner consults this to decide between "error: missing LLM" and "emit deferred payload."
2. New flag on `hector check`: `--emit-semantic-payload` (long-only; this is an adapter-internal flag, not user-facing). Mutually exclusive with `--session`. The flag forces the deferred path regardless of provider value — useful for tests and explicit adapter invocations.
3. When the deferred path is active (either via the flag or implied by `provider: claude-code-subagent`):
   - Run `script` and `ast` engines as today. AST counts as deterministic.
   - For each `engine: semantic` rule that *would* dispatch (post-scope, post-skip, post-A3-diff-prefilter): instead of calling the LLM, collect it into a `deferred_semantic_rules: Vec<DeferredRule>` list.
   - For each `engine: session` rule: same treatment — session rules also need subagent dispatch under subagent mode.
   - On block from deterministic rules: emit standard verdict JSON, exit 2 (unchanged).
   - On no block AND `deferred_semantic_rules` non-empty: emit a `DeferredVerdict` JSON wrapping `passed_checks` + the bully-shaped payload (with `_evaluator_input` rendered via the existing `prompt::build_prompt_split`), exit 0.
   - On no block AND no deferred rules: emit standard verdict JSON, exit 0.
3. `DeferredVerdict` JSON shape (additive, lives next to `Verdict`):
   ```json
   {
     "schema_version": 1,
     "deferred": true,
     "hector_version": "0.2.x",
     "passed_checks": ["rule-a", "rule-b"],
     "payload": {
       "file": "src/foo.rs",
       "diff": "@@ -1,2 +1,3 @@\n …",
       "passed_checks": ["rule-a", "rule-b"],
       "evaluate": [
         {"id": "rule-c", "description": "…", "severity": "error"}
       ],
       "_evaluator_input": "<TRUSTED_POLICY>…</TRUSTED_POLICY>\n<UNTRUSTED_EVIDENCE>…</UNTRUSTED_EVIDENCE>"
     },
     "elapsed_ms": 42
   }
   ```
4. Reuse `llm::prompt::build_prompt_split` to render `_evaluator_input`. The existing function already produces a `(system, user)` tuple; the subagent path wants them concatenated into one string. Wrap that concatenation in a new `llm::prompt::build_evaluator_input(rules, primary, context)` helper to make the intent legible.

**Acceptance criteria**
- [ ] `hector check --file X --emit-semantic-payload` on a file with only deterministic rules emits the standard verdict shape (no `deferred: true` wrapper).
- [ ] Same flag on a file with surviving semantic rules emits `DeferredVerdict` with `deferred: true`, exit 0.
- [ ] `passed_checks` in the payload matches the rule IDs that ran-and-passed deterministically.
- [ ] `_evaluator_input` is byte-identical to what the live semantic engine would have sent (modulo dynamic strings like `model`); verified by a snapshot test against the existing `prompt::build_prompt_split` output.
- [ ] When a deterministic rule blocks, the deferred path is suppressed entirely (exit 2, no payload).
- [ ] The flag does not enable LLM dispatch; running with `ANTHROPIC_API_KEY` set still produces a `DeferredVerdict` (the flag is the switch, not credential presence).

**Notes**
- The `--print-prompt` flag (shipped in C4) is *similar* but single-rule and stdout-only. `--emit-semantic-payload` differs: all-rules, runs deterministic checks, emits structured JSON. They share the prompt builder; that's the only overlap.

---

## H2. `hector record-verdict` subcommand (core) 🔴 critical

**Bully reference**
- `bully --log-verdict --rule <id> --verdict <pass|violation> --file <path>` appends a `semantic_verdict` record to telemetry. Invoked by the `bully` skill after parsing subagent output so coverage reports include subagent-evaluated rules.

**Hector current state**
- `crates/hector-core/src/telemetry.rs` already defines `LogEntry::SemanticVerdict { ts, rule, verdict, file }` (shipped in D1). No CLI surface to write one from outside the runner.

**Why it matters**
- Without this, the skill can't backfill telemetry for subagent-evaluated rules. `hector coverage` (D2, shipped) would show subagent-mode users as having dead semantic rules across the board — false signal.

**Proposed design**
1. New subcommand `hector record-verdict` with required flags:
   - `--rule <id>` (single occurrence)
   - `--verdict <pass|violation>` (constrained to those two values)
   - `--file <path>` (optional; mirrors `LogEntry::SemanticVerdict.file: Option<String>`)
   - `--dir <path>` (optional; defaults to cwd, locates `.hector/log.jsonl` for tests)
2. Implementation: append a `LogEntry::SemanticVerdict { ts: now_rfc3339(), rule, verdict, file }` via the existing `telemetry::append` API. No locking gymnastics beyond what `append` already does.
3. Exit codes: `0` on success; `1` on telemetry write failure (disk full, perms). Never `2` — this command is not a gate.
4. Hidden from `--help` by default? **No.** Future contributors will want to inspect it. Document it as adapter-internal in `docs/`.

**Acceptance criteria**
- [ ] `hector record-verdict --rule r1 --verdict pass --file foo.rs` appends exactly one valid `semantic_verdict` line to `.hector/log.jsonl`.
- [ ] Invalid `--verdict` values (`fail`, `block`, etc.) error at clap parse time with exit `1`.
- [ ] `--file` omission produces a record with `file: null` (matches `Option<String>` semantics).
- [ ] Running the command in a directory with no `.hector/` initializes it (mirrors `commands/check.rs` behavior).

**Notes**
- No auth. The trust model is unchanged: an attacker who can run `hector record-verdict` can also write to `.hector/log.jsonl` directly. This subcommand is convenience, not security.

---

## H3. Claude Code adapter — subagent mode (adapter) 🔴 critical

**Bully reference**
- `~/.claude/hooks/bully` (the bully plugin's hook entry) detects PostToolUse, runs `bully` pipeline, emits stderr-blocked OR `hookSpecificOutput.additionalContext` JSON.
- `skills/bully/SKILL.md` interprets hook output; dispatches `bully-evaluator` subagent.
- `agents/bully-evaluator.md` defines the in-session subagent.

**Hector current state**
- `adapters/claude-code/hooks/hook.sh` always runs `hector check --file X`. No branch for subagent mode.
- `adapters/claude-code/skills/` ships `hector-init`, `hector-author`, `hector-review` — no equivalent of bully's `bully` interpreter skill.
- No subagent definition in the adapter.

**Why it matters**
- This is the user-visible feature. H1 and H2 are scaffolding; H3 is what subscription users actually run.

**Proposed design**

Three new files and one hook.sh change.

1. **`adapters/claude-code/hooks/hook.sh` — subagent-mode branch.**
   - Detect mode via `hector show-resolved-config --format json | jq -r '.llm.provider'` (shipped in C3 — no new core surface needed). If the value is `claude-code-subagent`, route through subagent path; otherwise current behavior.
   - Cache the detection per-invocation; `show-resolved-config` is cheap but not free.
   - Subagent path:
     ```bash
     hector check --file "$FILE" --emit-semantic-payload --format json > "$TMP" || EC=$?
     if [[ $EC -eq 2 ]]; then
       cat "$TMP" >&2          # standard block path
       exit 2
     fi
     if jq -e '.deferred == true' < "$TMP" >/dev/null; then
       # Emit Claude Code's hookSpecificOutput envelope on stdout.
       jq -n --slurpfile p "$TMP" '{
         hookSpecificOutput: {
           hookEventName: "PostToolUse",
           additionalContext: ("AGENTIC LINT SEMANTIC EVALUATION REQUIRED:\n\n" + ($p[0].payload | tojson))
         }
       }'
       exit 0
     fi
     exit 0  # nothing to defer, nothing blocked
     ```
   - Keep the existing direct-API branch (`hector check --file X` without `--emit-semantic-payload`) so users with API keys are unaffected.
2. **`adapters/claude-code/skills/hector/SKILL.md` — interpreter skill.**
   - Port `skills/bully/SKILL.md` from the bully repo: search-and-replace `bully` → `hector`, `bully-evaluator` → `hector-evaluator`, `bully --log-verdict` → `hector record-verdict`.
   - Preserve the inline-vs-dispatch heuristic (single rule + ≤15-line diff → inline; otherwise subagent).
   - Preserve the malformed-response retry path.
3. **`adapters/claude-code/agents/hector-evaluator.md` — subagent definition.**
   - Port `agents/bully-evaluator.md` from the bully repo. The system-prompt text is already framed around `<TRUSTED_POLICY>` / `<UNTRUSTED_EVIDENCE>` — Hector's prompt builder produces the same shape, so the port is near-verbatim. Update the name and any bully-specific URLs.
4. **`adapters/claude-code/.claude-plugin/plugin.json` — register the new files.**
   - Add the skill and agent paths so `/plugin install` picks them up.

**Acceptance criteria**
- [ ] A `.hector.yml` with `llm.provider: claude-code-subagent` and one `engine: semantic` rule produces a `hookSpecificOutput.additionalContext` envelope on PostToolUse (verified by running `hook.sh` with a captured event JSON).
- [ ] The `additionalContext` string starts with `AGENTIC LINT SEMANTIC EVALUATION REQUIRED:` and the JSON tail parses as the bully-compatible payload shape.
- [ ] A `.hector.yml` with `llm.provider: anthropic` (API-key mode) is unchanged — hook.sh routes through the direct-API path, no payload emitted.
- [ ] A deterministic violation under subagent mode still produces exit 2 (no payload).
- [ ] Skill file passes `hector validate` on its own frontmatter (we ship a frontmatter linter? — open question, see §6).
- [ ] Agent file follows Claude Code's `.claude/agents/*.md` schema.
- [ ] End-to-end test: `adapters/claude-code/tests/subagent_mode.sh` runs hook.sh against a fixture event JSON in subagent mode and asserts the additionalContext shape.

**Notes**
- The skill needs access to `hector` on PATH (for `record-verdict`). Document this in `adapters/claude-code/README.md` alongside the existing `hector` + `jq` + `bash` requirements.
- This adapter feature is Claude-Code-specific by design. OpenCode adapter does not change; if OpenCode users want subscription-mediated eval, that's a separate (deferred) gap.

---

## H4. Spec + docs walkback 🟡 medium

**Bully reference**
- N/A. This is a Hector-internal documentation correction.

**Hector current state**
- [`overview.md`](./overview.md) §7.1 reads: "Subagent removed: semantic evaluation goes directly through the configured LLM provider, not through a Claude Code subagent. This drops a Claude-Code-specific coupling and reduces token usage. Behavior should be observably equivalent."
- §11.5 reads as an open question: "Subagent-removal impact. Existing Bully users may benefit from the Claude Code subagent's context isolation. Direct API calls have different cost/latency characteristics. Benchmark a representative repo (10–20 rules, mixed engines) before committing to the removal."
- That benchmark never ran. The decision was effectively committed without it.

**Why it matters**
- Once H1–H3 ship, the overview is wrong on its face. The walkback explains *why* the decision changed (the `claude -p` allowance withdrawal made the subagent path the only viable subscription option, which changed the math). Without the walkback, future contributors will see the §7.1 claim and the §11.5 open question and assume the current state is "we removed it and never decided whether that was right." The walkback closes the loop.

**Proposed design**
1. Edit [`overview.md`](./overview.md) §7.1: replace the "Subagent removed" paragraph with: "Two semantic-eval paths. Direct-API (default) calls the configured LLM provider directly. Subagent (opt-in via `llm.provider: claude-code-subagent`) routes through an in-session Claude Code subagent — required for subscription-only users since headless `claude -p` is not subscription-funded. See [`specs/2026-05-14-subagent-semantic-eval.md`](./2026-05-14-subagent-semantic-eval.md)."
2. Edit [`overview.md`](./overview.md) §11.5: replace the open question with a "Resolved" footnote pointing at this spec.
3. Update `adapters/claude-code/README.md` to document the two modes and the subagent-mode requirements.
4. Update `CHANGELOG.md` with a 0.2.x entry naming the new flag, subcommand, and adapter mode.

**Acceptance criteria**
- [ ] §7.1 no longer says "Subagent removed."
- [ ] §11.5 is marked resolved with a forward link.
- [ ] `adapters/claude-code/README.md` documents how to switch modes and what each requires.
- [ ] CHANGELOG entry mentions `--emit-semantic-payload`, `record-verdict`, and `llm.provider: claude-code-subagent`.

---

## 5. Suggested sequencing

```
0.2.x  ┬─ H1 --emit-semantic-payload                 (core, foundation)
       ├─ H2 record-verdict                          (core, independent of H1)
       ├─ H3 adapter subagent mode                   (depends on H1 + H2)
       └─ H4 spec + docs walkback                    (depends on H1–H3 shipping)
```

H1 and H2 are independent and can ship in parallel. H3 depends on both. H4 is the final cleanup. Each becomes a `plans/2026-05-14-hector-h<n>-<slug>.md`.

## 6. Open questions

- **Q1 (H1).** Should `--emit-semantic-payload` also emit a payload for `engine: session` rules? Bully's session-rule path uses the same subagent. Proposal: yes — session rules join `evaluate` in the payload, distinguished by an `engine` field per rule. Confirm in the H1 plan doc.
- **Q2 (H3).** Should the skill be auto-activated by Claude Code's skill-discovery (via the description preamble), or invoked explicitly by hook output? Bully relies on the description match. Proposal: same.
- **Q3 (H3).** Does the adapter ship a frontmatter linter to keep the skill/agent files in sync with Claude Code's schema, or do we rely on `/plugin install` validation? Defer: `/plugin install` is sufficient for 0.2.x.
- **Q4 (H1).** If a user has both `llm.provider: claude-code-subagent` and `ANTHROPIC_API_KEY` set, which wins? Proposal: config wins. The flag in `.hector.yml` is the explicit signal; env var presence is incidental.

---

**Cross-links**

- Source-material for the port: `~/Documents/projects/bully/hooks/hook.sh`, `~/Documents/projects/bully/skills/bully/SKILL.md`, `~/Documents/projects/bully/agents/bully-evaluator.md`, `~/Documents/projects/bully/src/bully/pipeline/semantic.py`.
- Existing prompt builder: `crates/hector-core/src/llm/prompt.rs::build_prompt_split`.
- Existing telemetry variant: `crates/hector-core/src/telemetry.rs::LogEntry::SemanticVerdict`.
- Existing adapter hook: `adapters/claude-code/hooks/hook.sh`.
