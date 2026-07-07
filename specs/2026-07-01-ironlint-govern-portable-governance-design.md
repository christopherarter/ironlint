# ironlint-govern — a portable governance layer for agentic tool use

**Status:** design, exploratory (2026-07-01). Not a commitment to build; captures a brainstorm.
**Revised 2026-07-01 (same day):** MVP narrowed after a market pass (Microsoft's Agent Governance Toolkit, the MCP-gateway category, the ACS standard). Kernel extraction proceeds exactly as specified — it pays for itself in the linter regardless and is fully de-risked by the characterization suite. The govern MVP ships **one** PEP (Claude Code `PreToolUse`), dogfooded on the author's own agents, plus an example policy pack mapped to the OWASP Agentic Top 10. Framework/cloud PEPs and the two-runtime demo move to deferred (§12).
**Builds on:** the 0.4 checks-pipeline substrate (`specs/2026-06-28-ironlint-checks-pipeline-design.md`) — same veto-by-exit-code primitive, generalized off of files.
**Breaking:** introduces a shared `ironlint-kernel` crate and refactors the shipped linter onto it. The linter's locked ABI, exit-code contract, and verdict JSON are preserved byte-for-byte (§8); the change is internal.
**Evolved 2026-07-07:** reviewed against six days of drift (§1.1). Path committed as **Approach A** — kernel extraction first, govern as the second `Product`, with `ironlint-bash-gate` absorbed as govern's built-in baseline policy (§3.1). The controlling design constraint is the **composable-spine principle** (§3.1): baked-in checks traverse the same `evaluate` pipeline as user policies — no bypass path. A baseline policy is just a policy whose `run` happens to invoke a built-in binary, not bespoke code beside the pipeline.

## 1.1 Reality delta — the drift since 2026-07-01 (logged 2026-07-07)

Six days of post-spec work. The thesis is **strengthened**, not invalidated — bash-gate is a second hand-built governor proving the pattern-repeat the kernel predicts. But the migration surgery is bigger than the original spec assumed. Concretely:

- **`ironlint-bash-gate` shipped (2026-07-06, v0.9.1)** — a whole new crate + `ironlint gate-bash` subcommand. NOT a `check`, NOT trust-gated, runs with no `.ironlint.yml`, fires before the config-existence check in every adapter's Bash branch. This is a **hand-built `pre-tool` lifecycle for Bash only**, built outside the policy mechanism — exactly the abstraction gap the kernel extraction predicted. **Resolved (§3.1):** absorbed as govern's built-in baseline policy, preserving its three properties (config-less, untrust-gated, runs-first).
- **Exit code 4 landed** (Task 3.2 — untrusted config → block loudly). The spec's "preserve the linter's exit-code contract byte-for-byte (§8)" now covers a **5-code** contract (0/1/2/3/4), not four. **Resolved (§8):** the kernel models trust-gating as a *pre-eval gate* (resolves to allow/deny before the verdict fold), so exit-4's "untrusted → block loudly" survives in govern as "untrusted → deny" — they agree because govern is already fail-closed by default.
- **File growth under the kernel lift surface** — every file the kernel would move has grown, several dramatically:

  | File | At spec | 2026-07-07 | Δ |
  | --- | --- | --- | --- |
  | `runner.rs` | 1354 | 1935 | +43% |
  | `engine/gate.rs` | 387 | 848 | +119% |
  | `trust.rs` | 499 | 1517 | **+204%** |
  | `telemetry.rs` | 277 | 645 | +133% |
  | `config/scope.rs` | 34 | 174 (proptests) | 5× |
  | `verdict.rs` | 179 | 219 | +22% |

  The "lift these unchanged" claim (§8) now lifts a much bigger surface. `trust.rs` in particular hardened dramatically — process locking, unique temp, corruption recovery, symlink refusal, **in-repo script hashing** (closes a post-bless script-swap RCE, Task 3.3). "Unchanged" becomes "behavior-identical, hardening preserved" — a riskier claim, but the characterization suite is bigger and catches more, so the *proof* is stronger even as the *surface* is larger.
- **`gate.rs` now scrubs the environment** (allowlisted env, no secret inheritance, 3.4), kills the process group on timeout (2.1), bounds drain wait. The render layer's `{stdin, env}` is the seam these flow through; the kernel keeps them.
- **A repo-sweep dispatch path landed in `runner.rs`** (2026-07-05, v0.9.0) — walker + lifecycle classifier + batched pre-commit. The spec's "only write/pre-commit dispatch stays in the linter" now also has to account for the sweep path — it stays in `ironlint-core` (it's linter-product dispatch), not the kernel.
- **`verdict` SCHEMA_VERSION bumped 5→6** once (InternalError detail), and `serde_yaml` → `serde_yaml_ng`. The kernel's generalized `Verdict` and the linter's preserved byte-shape diverge at the render layer, not the schema.

**Net:** Approach A committed. The drift makes step 1 (kernel extraction) *longer*, not *riskier* — the characterization suite's job is to catch exactly the byte-churn the grown surfaces would introduce.

## 1. Thesis — a file-write is just one kind of action

IronLint already governs one class of agent action: a **file write**. It watches a proposed edit, runs a check, and vetoes by exit code. Nothing in the *decision* machinery is actually about files — `engine::gate::run_gate` spawns `sh -c`, feeds stdin, and classifies an exit code with zero knowledge of what it's judging. The file-ness lives entirely in two thin places: the glob-on-path matcher (`config/scope.rs`) and the ABI env vars (`$IRONLINT_FILE`, content-on-stdin).

**Generalize the thing being judged from *a file write* to *a canonical action* — `{ kind, tool, args, content, context }` — and the same primitive governs anything an agent tries to do:** run this command, hit this URL, call this MCP tool, invoke this framework tool. That is `ironlint-govern`: a portable policy layer that sits between an agent and the tools it invokes, and can refuse.

The positioning is **middleware-in-a-box for agentic tool use**: a consistent, composable policy config plus a runtime-agnostic decision engine, portable across *any* agentic runtime — a coding harness (Claude Code, Cursor, opencode, reasonix), a framework where tool calls happen inside orchestration code (LangGraph, Hermes), or a cloud orchestrator — via swappable adapters. Write your agent policy once; enforce it everywhere.

The differentiator versus a harness's built-in permission system is exactly this portability: built-in permission rules are per-harness and don't transfer, and a hand-rolled `PreToolUse` hook is bespoke per project. `ironlint-govern` is **one policy spec, one decision engine, enforced identically across every runtime**, reusing ironlint's hardest-to-copy asset — the multi-harness adapter layer — and its composable-config engine (`extends`).

**Beachhead (2026-07-01 revision):** the long-term portability story stands, but the MVP targets the one enforcement point the mid-2026 incumbents (in-process framework middleware, server-side MCP gateways) structurally can't reach — **coding agents on the developer's machine**, where `Bash` and `Write` traverse nobody's gateway and where ironlint's adapters, trust store, and `init` machinery already live. Frameworks and cloud come later, likely via a single ACS adapter once that standard stabilizes, rather than N bespoke PEPs.

## 2. Decisions log (the forks this design settled)

| Fork | Decision |
| --- | --- |
| Core driver | Product/TAM: reposition ironlint as *the governance layer for agentic tool use*. |
| Product shape | **Separate product** (`ironlint-govern`) on a **shared core**, with security-grade defaults. |
| The wedge | Long-term: portable across any agentic runtime. **Revised beachhead:** the coding-agent endpoint (dev laptop) — the enforcement point framework middleware and MCP gateways can't reach. Frameworks/cloud deferred (§12). |
| Decision architecture | **CLI subprocess** (`ironlint-govern eval`, Action JSON on stdin, verdict via exit code). Daemon/PDP is a documented later upgrade. |
| Kernel scope | **Extract `ironlint-kernel`** generic over `Action`; refactor **both** the linter and govern onto it. |
| Input contract | A canonical **`Action`** schema; per-product `render(Action) → {stdin, env}` preserves the linter's locked ABI. |
| Matcher | Coarse **`tools:`** glob over tool *names*; the policy body makes the fine-grained decision (parity with the linter's files/run division). |
| Fail posture | **Fail-closed** by default for govern (`on_error: closed`); untrusted policy → deny-all. |
| Bypass model (MVP) | Repo-owned, not tamper-proof against the repo owner. Non-bypassable signed policy is a later enterprise capability. |
| MVP proof | **One PEP, dogfooded** — Claude Code `PreToolUse` governing the author's own daily agent use, plus an OWASP-Agentic-Top-10-mapped policy pack. (Two-runtime demo deferred with the framework PEP.) |

## 3. Layered architecture — the box and its plugs

```
┌─────────────────────────────────────────────────────────────┐
│  ADAPTERS / PEPs   (per-runtime, NOT portable — by design)  │
│                                                              │
│  claude-code   cursor   opencode   reasonix   ← largely have │
│  langgraph(py)   hermes   cloud-orchestrator  ← new plugs    │
│                                                              │
│  each: (1) installs the interception point                  │
│        (2) translates native tool-call → canonical Action   │
│        (3) shells out: `ironlint-govern eval` < action.json   │
└───────────────────────────┬─────────────────────────────────┘
                            │  Action (stdin) → verdict (exit code)
┌───────────────────────────▼─────────────────────────────────┐
│  PRODUCT LAYER   ironlint (lint)  │  ironlint-govern (actions)   │
│  thin CLIs: schema + matcher axis + defaults (fail posture) │
└───────────────────────────┬─────────────────────────────────┘
                            │
┌───────────────────────────▼─────────────────────────────────┐
│  ironlint-kernel   (runtime-agnostic — the portable box)      │
│  evaluate(Action, Policy) -> Verdict                         │
│  gate exec · trust · telemetry · verdict · extends-compose  │
└─────────────────────────────────────────────────────────────┘
```

Three layers, three portability verdicts:

- **Kernel** — fully portable. Input is an `Action`; output is a `Verdict`. Knows nothing about files, harnesses, or frameworks. This is `gate` + `trust` + `telemetry` + `verdict` + `extends`-composition, lifted out of today's `runner.rs`. The crate boundary makes "runtime-agnostic" a compile-time fact: the kernel cannot depend on file- or harness-specific code.
- **Product layer** — two thin CLIs over the kernel. `ironlint` instantiates it with `kind: write` + glob matcher + fail-*open*. `ironlint-govern` instantiates it with `kind: tool_call` + tool matcher + fail-*closed*. This is the only place the two products diverge, and it is deliberately small.
- **Adapters (PEPs)** — per-runtime plugs, never portable, by design. Each is a small shim: install the interception point, translate the runtime's tool call into a canonical `Action`, invoke the CLI. Coding-harness plugs largely exist; frameworks/cloud are new but each is small.

**Data flow, one action:** agent attempts a tool call → the runtime's PEP intercepts → PEP serializes it to a canonical `Action` → `ironlint-govern eval` reads it on stdin, the kernel loads + composes policy (trust-checked), runs matching policies via `sh -c`, folds exit codes into a `Verdict` → the CLI exit code carries the verdict back → PEP allows or blocks the call → the kernel appends a telemetry record.

### 3.1 The composable-spine principle + the bash-gate absorb

**The controlling constraint.** A baked-in check must traverse the **same** `evaluate` pipeline as a user policy — never a bypass path. If a built-in needs bespoke code beside the pipeline to run, the abstraction is hollow and govern has failed to eat its own dogfood. This is non-negotiable: the baseline policy is "just a policy," distinguished only by being built-in code rather than config.

**Why bash-gate absorbs cleanly.** bash-gate already ships as `ironlint gate-bash` — a pure-Rust binary that reads a Bash command on stdin and exits `0` (allow) or `2` (block, reason on stdout). It is *already* a `run`-able gate. So the baseline policy for `tool: Bash` is:

```yaml
# ironlint-govern built-in baseline (code, not user config — never trust-gated, fires on a config-less project)
baseline-bash-self-trust:
  tools: ["Bash"]
  run: "ironlint gate-bash"      # reads the Action's args.command on stdin, exit 0/2
  on: [pre-tool]
  trust_gated: false             # baseline only — see below
```

That `run` goes through the **identical** `gate::run_gate(run, env, stdin, timeout)` every user policy uses. The baseline is not special-cased in `evaluate`; it is selected first and runs first, but through the same code path. The composable-spine principle holds because the baseline is structurally indistinguishable from a user policy at the pipeline level.

**The three bash-gate properties, preserved:**

| Property today | After absorb |
| --- | --- |
| Runs with **no `.ironlint.yml`**, no trust store | Baseline policy is **built-in code**, not config — fires on a config-less project before the user-policy load. |
| **Not trust-gated** | Baseline policies are never trust-gated (`trust_gated: false` is a baseline-only field; user policies cannot set it). User policies are trust-gated as in §7. |
| Runs **before** the config-existence check | Govern `eval` order: **baseline policy → (if config exists) trust-gate → user policies**. Baseline is the non-bypassable floor; user `policies:` can only *add* blocks, never relax it. |

**`ironlint-bash-gate` crate fate.** Its surface is unchanged — still a pure-Rust Bash classifier exposed as `ironlint gate-bash`. What changes is its dependency home: today `ironlint-cli` depends on it directly (the `gate-bash` subcommand); after absorb, `ironlint-govern` depends on it (the baseline policy invokes it), and `ironlint-cli` exposes `gate-bash` as a passthrough for backward compat. The crate's code does not move.

**One interception point per adapter.** Today each adapter has a Bash branch (→ `gate-bash`) and a Write branch (→ `ironlint check`). After absorb, the Claude Code `PreToolUse` PEP shells out to **`ironlint-govern eval`** with the canonical `Action` on stdin for *every* tool call. Govern's baseline handles Bash self-trust; govern's user policies handle everything else (egress, MCP writes, destructive shell beyond the baseline); the linter (still wired, still a separate product) handles `kind: write`. The adapter's Bash-specific branch collapses into the general PEP.

## 4. The canonical `Action` (input contract / new stability surface)

```json
{
  "kind": "tool_call",
  "tool": "Bash",
  "args": { "command": "git push --force origin main" },
  "content": null,
  "context": {
    "runtime": "langgraph",
    "session_id": "…", "agent_id": "…",
    "root": "/repo", "event": "pre-tool", "ts": "…"
  }
}
```

- `kind` — generalizes the kernel: `write | tool_call` (extensible enum).
- `tool` — the tool name (`Bash`, `WebFetch`, `mcp__github__create_issue`).
- `args` — the tool-specific argument object; raw, unbounded, tool-defined.
- `content` — proposed bytes for the `write` kind; `null` for tool calls.
- `context` — provenance/audit envelope (runtime, session, agent, root, event, timestamp). This is what makes a unified cross-agent telemetry trail possible.

**How a policy reads an Action** — mirror the existing ABI philosophy (curated env scalars + stdin):

- **Env** = curated scalars only: `$IRONLINT_ACTION_KIND`, `$IRONLINT_TOOL`, `$IRONLINT_RUNTIME`, `$IRONLINT_ROOT`, `$IRONLINT_EVENT`.
- **Stdin** = the full `Action` JSON; the policy parses `args` itself (jq / python / whatever).
- **`args` is never spliced into env or the command string.** Arguments are attacker-influenced, so they only ever arrive as inert JSON on stdin — preserving ironlint's anti-templating stance ("the path travels only as an env value, never spliced into `run`"). This is a security property, not tidiness.

**Constraint — the linter's ABI is locked.** The linter renders `content` on stdin with `$IRONLINT_FILE` in env; that is a stability surface and must not change. So the kernel is generic over `Action`, but *how* an `Action` is rendered to process-I/O is a small per-product step: `render(Action) → {stdin, env}`. The linter renders the legacy way; govern renders the new way. One kernel, two locked ABIs, no breakage. The kernel just calls `gate::run_gate(run, env, stdin, timeout)` with whatever the product rendered.

## 5. The policy config (composable)

```yaml
extends:
  - ironlint-govern:baseline-egress     # composable: pull a shared policy library

execution:
  timeout_secs: 10
  on_error: closed                    # fail-CLOSED default (govern) vs open (linter)

policies:
  block-destructive-shell:
    tools: ["Bash"]                   # matcher axis: glob over tool NAMES (not paths)
    run: |
      jq -r '.args.command' | grep -Eq 'rm -rf|mkfs|:\(\)\{' && exit 1 || exit 0
    on: [pre-tool]                    # new lifecycle: preventive, before execution

  egress-allowlist:
    tools: ["WebFetch", "Bash"]
    run: govern-egress-check          # policy-as-code: a real script, not a list

  mcp-prod-writes:
    tools: ["mcp__*"]                 # glob across MCP tool names
    run: block-if-prod
```

**Reused verbatim** from the linter: `extends` (the composability engine — cycle-detected DFS, local-wins-on-collision; this *is* "consistent composable config", already built), `execution`, `steps`/`run`, `name`.

**New/different, deliberately tiny:**

- **`tools:` replaces `files:`** — a glob over *tool names* (`Bash`, `mcp__*`). Same coarse-scope-then-body-decides division the linter uses (files = coarse scope, `run` = fine decision). The matcher is actually *simpler* than the linter's: tool names are not paths, so it drops `scope.rs`'s path-flavored `**/` bare-pattern quirk.
- **`on: [pre-tool]`** — a new, preventive lifecycle (veto before the tool runs), which the linter's reactive `write`/`pre-commit` never needed. The kernel treats lifecycle as an opaque tag; each product supplies its own valid set.
- **`on_error: closed`** — fail posture as a first-class field, defaulting to *closed* for govern (a broken policy must not silently allow `rm -rf`), the opposite of the linter's fail-open default. Per-policy overridable.

**`policies:` not `checks:`** — distinct product vocabulary over the same underlying struct, so each product reads as what it is.

Decision: **no declarative `args_match:`/`where:` predicate in the MVP** — coarse tool matcher + body-decides, mirroring the linter; every policy parses stdin. A declarative arg-predicate is a plausible *ergonomic* follow-up, not MVP.

## 6. The `eval` flow and PEP exit contract

```
Action JSON (stdin)
  → load + extends-compose policy        (trust-checked)
  → select policies where tools: matches Action.tool
                     AND on: matches Action.context.event
  → for each: render(Action) → {stdin, env}; run via sh -c; classify exit
  → fold outcomes → Verdict
  → resolve fail posture → emit
       • exit code  = final allow/deny  (PEP reads this)
       • stdout     = Verdict JSON      (rich PEPs read this)
  → append telemetry record             (the audit trail)
```

**PEP-facing exit contract — deliberately binary:**

- `0` = **allow** the action to proceed.
- non-zero = **do NOT allow** (block).

That is the whole contract a PEP must know. This is a deliberate divergence from the linter's contract — five codes as of 2026-07-02 (`0/1/2/3/4`, the govern-era four-code `0/1/2/3` plus exit-4 untrusted; see §1.1) — justified by the portability goal: **govern resolves fail posture internally and hands the PEP an already-decided answer.** A PEP in Python for LangGraph, in Go for a cloud orchestrator, in a shell hook for Claude Code all implement the same trivial rule: `exit 0 → proceed, else → block`. Nuance (explicit veto vs. error-resolved-to-deny, which policy fired, the message to show the agent) lives in the **Verdict JSON on stdout** for PEPs that want a good rejection reason, and always lands in telemetry regardless. Keeping the exit code binary is what keeps the plug layer cheap in any language.

## 7. Fail-closed posture, trust, and bypass

**What each outcome resolves to:**

- **No policy configured** → nothing to govern → **allow** (exit 0). Govern is opt-in; absence is not denial.
- **All pass / no match** → **allow**.
- **Explicit veto** (policy exits 1–125) → **deny**. Message = the policy's trimmed stdout+stderr (reuses gate's Block-message construction), fed back to the agent by the PEP.
- **Policy crashes** (127 / timeout / signal) **or won't load / parse** → resolved by **`on_error`, default `closed` → deny.**

**Trust — reused, with one strict consequence.** Govern reuses the existing trust store (hash over config + referenced policy scripts). Because govern is fail-closed, **untrusted or tampered policy resolves to deny-all**, not a soft warning — if you cannot trust the policy on this machine, you cannot trust the actions it gates. This is the correct posture and also the sharpest adoption edge: a fresh clone denies every governed action until `ironlint-govern trust` runs. `on_error: open` (or a dev env override) is the documented escape hatch. **Decided:** keep deny-all as the default.

**Bypass model — honest MVP scope.** Governance is **repo-owned, not tamper-proof against the repo owner**: a dev who controls the repo can edit policy, decline to trust it, or set `on_error: open`. That is fine for self-imposed rails and framework use — the wedge (portability, composability, policy-as-code) never depended on non-bypassability. True **non-bypassable, centrally-signed policy** (fetched from an authority, dev can't edit) is a later enterprise capability, and the trust-hash machinery is its foundation. The linter's inline `ironlint-disable:` directives **do not carry over** — an action has no body to hold them, and an inline per-action bypass would be a governance hole.

## 8. The kernel refactor and migrating the shipped linter

**Target workspace (crate boundaries enforce portability):**

```
ironlint-kernel      lib   Action · Verdict · evaluate() · gate · trust ·
                         telemetry · extends-compose · the Product trait.
                         Depends on NOTHING file- or harness-specific.
ironlint-core        lib   LINTER product: write/pre-commit lifecycles,
                         file-glob matcher (scope.rs), diff, disable,
                         render(write→legacy ABI). Depends on kernel.
ironlint-govern      lib   GOVERN product: pre-tool lifecycle, tool-name
                         matcher, render(tool_call→Action ABI). Depends on kernel.
ironlint-cli         bin   `ironlint`         (thin, over core + kernel)
ironlint-govern-cli  bin   `ironlint-govern`  (thin, over govern + kernel)
```

**Kernel public API — generic over a `Product`:**

```rust
// ironlint-kernel
pub struct Action { kind, tool, args, content, context }
pub struct Verdict { /* generalized; SCHEMA_VERSION bumped */ }

pub trait Product {
    fn matches(&self, policy: &Policy, action: &Action) -> bool; // scope test
    fn render(&self, action: &Action) -> ProcessIo;             // {stdin, env} — the ABI
    fn valid_lifecycles(&self) -> &[&str];                      // which `on:` tags exist
}

pub fn evaluate(actions: &[Action], policies: &PolicySet, product: &impl Product) -> Verdict;
```

- The **linter is `Product` with `kind: write`** (glob-on-path matcher, legacy render). **Govern is `Product` with `kind: tool_call`** (tool-name matcher, Action-JSON render, + a baseline-policy host — §3.1). "A file-write is just one action kind" becomes a concrete, testable claim.
- **`extends`-compose goes generic** — `resolve<T>(named_map, …)` composes any named map of `extends`-participating entries (checks *or* policies). The composability engine is shared verbatim.
- `gate::run_gate`, `trust`, and `telemetry` move up **behavior-identical** (the 2026-07-07 drift — §1.1 — hardened `trust.rs` with process locking, in-repo script hashing, exit-4 enforcement, and grew `gate.rs` with env-scrubbing + process-group kill; all lift intact). "Unchanged" in the original spec meant behavior-identical; the grown surface makes the characterization suite's no-diff bar the decisive proof.
- **Trust-gating as a pre-eval gate (exit-4 reconciliation).** The linter's 5-code contract (0/1/2/3/4) is preserved for the linter product. In the kernel, trust-gating resolves to allow/deny *before* the verdict fold — so exit-4's "untrusted → block loudly" surfaces in govern as "untrusted → deny," consistent with govern's fail-closed default. The kernel's `evaluate` is: `trust_gate(action, policy_set) → Allow | Deny` then `run → fold`. The baseline policy bypasses the trust gate (`trust_gated: false`), which is why it fires on a config-less project (§3.1).
- **Composable-spine invariant (carried from §3.1).** The baseline policy goes through the *same* `gate::run_gate` as user policies. The migration's acceptance test for the absorb: a fixture where a Bash action triggers the baseline on a config-less project must produce the *same* verdict (exit 0/2) as today's `ironlint gate-bash` invocation — i.e., the absorb is provably a no-op for existing bash-gate behavior.

**Migration sequence — order is non-negotiable:**

1. **Extract the kernel *underneath* the linter, behavior byte-identical.** Pure internal refactor: gate/trust/telemetry/verdict/extends move to `ironlint-kernel`; the linter is re-expressed as `Product<write>`. The full existing suite — `cli_e2e_gates`, the `insta` snapshots, the unit tests, the ≥80% region gate — is the **characterization harness**. It must stay green **with zero snapshot churn**. Green-with-no-diff *is* the proof that the locked ABI, the 5-code exit contract (`0/1/2/3/4` — code 4 added 2026-07-02, see §1.1), and the verdict JSON are untouched. The v0.9.0 repo-sweep dispatch path stays in `ironlint-core` — it is write/pre-commit dispatch, the linter product's job, not kernel territory.
2. **Only then** build `ironlint-govern` as a *second* `Product` on the now-proven kernel — purely additive, touching nothing in the linter's dispatch. Govern ships the baseline-policy host (§3.1) and absorbs the bash-gate invocation into it as its first built-in.

**Side benefits:** `runner.rs` (50K, flagged as carrying too much) shrinks — its generic load/resolve/fold/log spine leaves for the kernel; only write/pre-commit dispatch stays. The kernel inherits the same coverage (≥80% region) and cognitive-complexity (≤15) gates, so `evaluate` is decomposed (select → run → fold) rather than growing into one function.

## 9. Enforcement points (the PEP model)

An adapter/PEP does three things: install the interception point, translate the native tool call into a canonical `Action`, and shell out to `ironlint-govern eval` (allow on exit 0, block otherwise, surfacing the Verdict JSON message to the agent).

- **Claude Code (the MVP PEP)** — a `PreToolUse` hook via the existing `adapter`/`json_settings` machinery. The plug is small; the machinery exists; the author runs it daily, which is the real-usage proof.
- **Other harnesses** (Cursor, opencode, reasonix) — each a small follow-up plug on the same seam (`adapter/registry.rs`); the natural second PEP when one is wanted.
- **Framework (Python) / cloud** — deferred (§12): a thin callback / tool-wrapper that subprocess-calls the CLI. If ACS stabilizes, one ACS adapter may cover several of these at once.

## 10. MVP scope and phasing

**MVP = the smallest thing with real usage** (revised 2026-07-01): the kernel extraction (which pays for itself in the linter regardless), a govern CLI, **one** enforcement point the author uses daily, and a policy pack legible to security reviewers. The earlier two-runtime proof is deferred — it invited a head-on latency/feature comparison with in-process framework middleware (Microsoft's Agent Governance Toolkit) that a subprocess CLI loses, in a segment that is not the beachhead. Real daily usage on one harness proves more than a demo on two.

**Deliverables:**

- `ironlint-kernel` extracted; linter migrated (§8 step 1); suite green, no snapshot diff. This lands even if govern stalls — it shrinks `runner.rs` and is fully de-risked by the characterization suite.
- `ironlint-govern` crate + CLI: `eval` (core), `validate`, `trust` (reused). Canonical `Action` schema and `policies:` schema, documented and versioned.
- **One enforcement point, dogfooded:** the Claude Code `PreToolUse` hook, installed on the author's machine and governing daily agent use. The second `Product` on the kernel is what earns the abstraction; the daily dogfood is what makes it credible.
- **OWASP-mapped example policy pack:** each policy annotated with the OWASP Agentic Top 10 item it addresses. Note (2026-07-07): the destructive-shell veto is now the **baseline policy** (absorbed bash-gate, §3.1) — it ships as a built-in, not in the user pack. The user pack layers *above* the baseline: egress allowlist, `git push --force` block, `mcp__*` prod-write guard. Every user policy ships with fixture tests proving what it blocks *and* what it deliberately allows. The baseline's real-shell-parsing requirement (not regex) is inherited from bash-gate's existing pure-Rust classifier.
- **The headline demo:** a coding agent attempting a destructive action and being vetoed end-to-end — the baseline policy firing, the verdict JSON the agent sees, and the telemetry record, in one short walkthrough. A second demo: a user policy (egress allowlist) blocking a call the baseline would allow, proving the superset layer works.

**Phasing** (maps to the `plans/` convention; each phase independently shippable/reviewable):

1. Kernel extraction + linter migration (green, no diff). Suite green with zero snapshot churn is the gate.
2. `ironlint-govern` core — schema, `eval`, trust, fail-posture, telemetry, **baseline-policy host** (§3.1). The baseline `ironlint gate-bash` invocation must produce byte-identical verdicts to today's bash-gate — the absorb's no-op proof.
3. The Claude Code PEP + the OWASP-mapped user policy pack + the dogfood install = the demo.

## 11. Testing strategy

- **Kernel:** the migrated linter suite as characterization net, plus new tests for `evaluate`, matcher, `render`, fail-posture resolution, and trust→deny-closed. ≥80% region, complexity ≤15.
- **Govern CLI:** `assert_cmd` e2e mirroring `cli_e2e_gates` — feed `Action` JSON on stdin, assert exit code + verdict JSON across allow / deny / error-closed / error-open / untrusted-deny / no-policy-allow / **baseline-fires-on-config-less-project**.
- **Bash-gate absorb no-op test (acceptance gate for the absorb):** the exact Bash commands today's `ironlint gate-bash` allows/blocks, fed as `Action` JSON to `ironlint-govern eval` on a config-less project, must produce the same exit code (0/2) and message. The baseline policy must be provably a no-op for existing bash-gate behavior. This is the proof the composable-spine invariant (§3.1) holds.
- **Composable-spine test:** a fixture proving the baseline and a user policy for the same `tool: Bash` action both go through `gate::run_gate` — i.e., no special-case code path for the baseline. Enforced by a mutation test: a mutant that bypasses `gate::run_gate` for the baseline must be caught.
- **Stability surfaces:** `insta` snapshots pinning the `Action` schema and the exit contract.
- **PEP conformance:** the same `Action` fed to `eval` directly must yield the verdict the Claude Code PEP acts on — hook-in-the-loop e2e, verified in CI. (The cross-language pytest shim moves to §12 with the framework PEP; it returns when a second-language PEP does.)
- **Policy-pack fixtures:** every shipped example policy has tests for what it blocks and what it deliberately allows, including the known bypass shapes for the destructive-shell policy (`rm -r -f`, flag reordering, encoded commands).

## 12. Out of scope (deferred, each a clean follow-up)

- **Daemon/PDP** (`ironlint-govern serve`) — the perf upgrade for tight tool-loops.
- **Declarative `args_match:` predicates** — coarse tool matcher + body-decides for now.
- **Non-bypassable / centrally-signed policy** — the enterprise story; trust-hash is its foundation.
- **FFI/embedded bindings** (PyO3/napi) — frameworks use the subprocess shim first.
- **`post-tool` audit-only lifecycle** — MVP is preventive `pre-tool`.
- **All PEPs beyond Claude Code** (moved here in the 2026-07-01 revision) — other harnesses (Cursor, opencode, reasonix) are small plugs on the existing seam and the natural next step; framework (LangGraph/Hermes) and cloud PEPs are further out and may be subsumed by a single ACS adapter once that standard stabilizes. The two-runtime byte-identical-policy demo and the cross-language pytest conformance shim move here with them.
- **Rich TUI / `watch` for govern.**

## 13. Open questions

1. **Second PEP, when wanted** — another coding harness (Cursor or opencode: small plugs on the existing `adapter/registry.rs` seam) vs. waiting for ACS to stabilize and shipping one standard adapter instead. No longer blocks any MVP phase.
2. **Telemetry record shape** — how much of `context` (session/agent/runtime) to persist, and whether the govern audit log is the same `.ironlint/log.jsonl` or a distinct store.
3. **`ironlint-govern init`** — does the MVP ship an onboarding/install path for the Claude Code PEP (the dogfood install argues yes, and the `init` machinery exists), or is manual wiring acceptable initially?
4. **Verdict JSON** — a govern-specific schema vs. extending the linter's `Verdict`; likely a new `SCHEMA_VERSION` under a govern namespace.
5. **ACS verdict vocabulary** — ACS defines allow/deny/**modify**; the binary exit-code contract cannot express *modify*. Decide explicitly whether modify is out of scope (likely yes — ironlint's primitive is a veto, and a modify verdict would break the "PEP reads one bit" portability property) and document the rationale where the exit contract is specified (§6).
