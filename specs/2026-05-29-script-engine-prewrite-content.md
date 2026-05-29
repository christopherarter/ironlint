# Engineering brief: script-engine content blindness under pre-write gating

**Date:** 2026-05-29
**Status:** Investigation brief — written to hand to independent max-effort agents for a fresh look. The "Recommended direction" near the end is *current thinking*, not a decision. Pressure-test it; try to break it; propose better.
**Owner:** dynamik-dev (Chris)
**Relates to / amends:** [`specs/2026-05-25-reasonix-adapter.md`](./2026-05-25-reasonix-adapter.md) §5A "known limitation", [`adapters/reasonix/README.md`](../adapters/reasonix/README.md) "Limitation: engine: script rules"

---

## 0. How to use this document

You are being asked to investigate a real engineering tension in Hector and either confirm or improve the proposed direction. This brief is self-contained: it includes the architecture you need, the exact code paths, a 30-second reproduction with real output, verification of the third-party contract involved, and the open questions. You should be able to start cold.

Bias warning: the author already has a leading hypothesis (§9). Treat it as a claim to falsify, not a conclusion. The most valuable output is a sharper framing or a failure mode the author missed.

---

## 1. TL;DR

Hector's `script` engine evaluates the **on-disk file**, never the **proposed content** a caller supplies via `hector check --file <path> --content -`. In *post-write* gating (the edit already landed on disk) that's correct and invisible. In *pre-write* gating (the gate runs **before** the edit lands) it is silently wrong: the linter inspects stale or absent content, so violations in the proposed edit pass straight through.

This surfaced via the Reasonix adapter (Reasonix's only blocking hook, `PreToolUse`, fires before the write). The adapter is wired correctly and the Reasonix contract is fully verified — the defect is entirely in the core `script` engine. Because many real projects' entire policy is a single `script` rule (e.g. `biome check`), the pre-write gate is effectively a no-op for them.

The deeper question is not "fix biome." It is: **what is the cleanest, most tool-agnostic, most harness-agnostic way for the core to let subprocess-based rules see proposed content — without accumulating per-tool/per-harness special cases?** Getting *consistent blocking semantics across harnesses with divergent lifecycle hooks* is the project's recurring hard problem; this is one instance of it.

---

## 2. What Hector is (just enough to investigate)

Hector is a policy-enforcement pipeline for AI coding agents (Rust rewrite of `dynamik-dev/bully`). A coding harness (Claude Code, Reasonix, OpenCode, …) calls `hector check` on each edit; Hector returns a verdict and an exit code; the harness's hook turns that into "allow" or "block."

**Engines** (`crates/hector-core/src/engine/`), selected per rule by `engine:`:

| Engine | What it does | Sees proposed content? |
|---|---|---|
| `script` | Runs an external command (`biome`, `eslint`, a shell script…) and treats non-zero exit as a violation | **No — reads the on-disk path. This is the bug.** |
| `ast` | `ast-grep` patterns over parsed source | Yes |
| `semantic` | Sends content/diff to an LLM | Yes |
| `session` | Aggregated cross-edit checks over a recorded changeset | N/A (changeset-level) |

`ast` and `semantic` honor proposed content because they consume it in-memory (see `engine/context.rs:44-50`, `file_body` prefers caller-supplied `content` over a disk read). The `script` engine is the lone exception: it shells out to a tool that wants a *path*, and it hands that tool the real on-disk path.

**`hector check` input modes:**

- `--file <path>` — read `<path>` from disk; that's the content.
- `--file <path> --content -` — read the proposed content from **stdin**; keep `<path>` for scope/baseline/AST-language purposes but **do not** read the file. (Added specifically to enable pre-write gating; see `specs/2026-05-25-reasonix-adapter.md` §5A.)
- `--diff <difffile>` — parse a unified diff; read each changed file from disk (post-edit).

**Exit-code contract** (`commands/check.rs`): `0` pass/warn · `1` config/internal error · `2` block (policy violation) · `3` engine runtime error. Adapters fail-open on `3` by default.

**Adapter model:** each harness ships a `hooks/hook.sh` under `adapters/<harness>/`. The hook parses that harness's event payload, extracts the changed file (and, for pre-write, the proposed content), invokes `hector check`, and maps the exit code to the harness's allow/block convention. **Lifecycle differences between harnesses live in the adapter; core is supposed to be harness-agnostic.**

---

## 3. The root cause, in code

`crates/hector-core/src/engine/script.rs:31-51`:

```rust
pub fn run_script_rule(
    rule_id: &str,
    rule: &Rule,
    file: &Path,
    _diff: &str,
    cwd: &Path,
) -> Result<Vec<Violation>> {
    let script = rule.script.as_ref().ok_or_else(...)?;
    // `{file}` expands to the shell parameter `"$HECTOR_FILE"`. ...
    let substituted = script.replace("{file}", "\"$HECTOR_FILE\"");
    let caps = rule.capabilities.clone().unwrap_or_default();
    let file_str = file.display().to_string();
    let outcome =
        run_with_capabilities_env(&substituted, cwd, &caps, &[("HECTOR_FILE", &file_str)])?;
    ...
}
```

`HECTOR_FILE` is set to `file` — the real on-disk path — and the proposed content is **never referenced**. The engine's `RuleContext` *does* carry the content (`engine/mod.rs:17-25`, field `content: Option<&'a str>`), and the runner *does* populate it (`runner.rs:719-735`). The `script` engine simply ignores `ctx.content`:

```rust
// runner.rs:737-744 — every engine gets the same ctx; only script ignores ctx.content
let outcome: Result<Vec<Violation>> = match rule.engine {
    EngineKind::Script   => crate::engine::script::ScriptEngine.run(&ctx), // uses ctx.file only
    EngineKind::Ast      => crate::engine::ast::AstEngine.run(&ctx),       // uses ctx.content
    EngineKind::Semantic => crate::engine::semantic::SemanticEngine.run(&ctx),
    _ => Ok(Vec::new()),
};
```

The content reaches core correctly. `commands/check.rs:89-100` (`run_file`) overrides the disk read with stdin when `--content` is given, and `runner.rs:945-953` marks file-mode content authoritative. Everything upstream of the `script` engine is correct. The `script` engine is where the chain breaks.

The limitation is **already documented** in three places (so this is a deliberately-deferred gap, not a regression):
- `crates/hector-cli/src/cli.rs:35-38` (the `--content` help text);
- `adapters/reasonix/README.md:40-46`;
- `specs/2026-05-25-reasonix-adapter.md:114-119`.

The investigation question is whether/how to close it cleanly — not whether it's "known."

---

## 4. 30-second reproduction (no biome, no Reasonix)

A minimal `script` rule that blocks any file containing `FORBIDDEN`. We hold the on-disk file and the proposed (`--content`) content *deliberately out of sync* to isolate which one the engine reads.

```bash
HX="$HOME/.cargo/bin/hector"          # hector 0.1.0
REPRO="$(mktemp -d)"; cd "$REPRO"
cat > .hector.yml <<'YAML'
schema_version: 2
rules:
  no-forbidden:
    description: "File must not contain the token FORBIDDEN."
    engine: script
    scope: ["**/*.txt"]
    severity: error
    output: passthrough
    script: "! grep -q FORBIDDEN {file}"
YAML
printf 'clean content\n' > foo.txt     # on-disk = CLEAN
hector trust >/dev/null

# CASE 1 — proposed content (stdin) CONTAINS FORBIDDEN -> should BLOCK
printf 'FORBIDDEN\n' | "$HX" check --file foo.txt --content - --config .hector.yml --format json

# CASE 2 — flip it: on-disk = FORBIDDEN, proposed = clean -> should PASS
printf 'FORBIDDEN\n' > foo.txt
printf 'clean\n'     | "$HX" check --file foo.txt --content - --config .hector.yml --format json
```

**Actual output (verbatim, hector 0.1.0):**

```
### on-disk foo.txt = 'clean content'  (no FORBIDDEN)
--- CASE 1: proposed content (stdin) CONTAINS FORBIDDEN -> should BLOCK (exit 2) ---
{ "status": "pass", "violations": [], "passed_checks": ["no-forbidden"], ... }
exit=0                       # WRONG: the forbidden proposed content passed

### on-disk foo.txt = FORBIDDEN, proposed content = clean
--- CASE 2: proposed content (stdin) is CLEAN -> should PASS (exit 0) ---
{ "status": "block",
  "violations": [ { "rule_id": "no-forbidden", "engine": "script",
                    "file": ".../foo.txt", "message": "File must not contain ... FORBIDDEN." } ],
  ... }
exit=2                       # WRONG: a clean proposed edit was blocked
```

Both directions confirm: the `script` engine reads `foo.txt` from disk and ignores `--content` entirely. Swap `engine: script` for an `ast`/`semantic` rule and the same harness call honors `--content` correctly (`engine/context.rs:44-50`).

---

## 5. The original incident (Reasonix, biome)

`rbac-node`'s entire `.hector.yml` is one `script` rule:

```yaml
schema_version: 2
rules:
  biome-check:
    engine: script
    scope: ["packages/**/src/**/*.ts", "packages/**/src/**/*.tsx", ...]
    severity: error
    output: passthrough
    script: "pnpm exec biome check --no-errors-on-unmatched {file}"
```

Observed via the Reasonix `PreToolUse` hook (reconstructed from the session transcript; all hook invocations below were manual `printf … | hook.sh` smoke tests):

| Scenario | Hook exit | Why |
|---|---|---|
| `write_file` new file with violations | **0 (pass)** | file not on disk yet → `biome check <nonexistent>` + `--no-errors-on-unmatched` → exit 0 |
| `edit_file` introducing `==`, `console.log` | **2 (block)** | **false signal** — biome flagged the *pre-edit* on-disk content, not the proposed edit |
| `edit_file` that *fixes* a violation | **2 (block)** | biome still reads the pre-edit on-disk file; the fix is invisible |
| `edit_file` clean → clean | 0 (pass) | on-disk already clean |

Direct confirmation from the transcript that biome itself *can* see proposed content via stdin (it just never gets the chance through Hector):

```
$ printf 'var x = 1;\n' | pnpm exec biome check --no-errors-on-unmatched \
      --stdin-file-path packages/core/src/foo.ts
  × ... (biome flags it, exit 1)

$ printf 'var x = 1;\n' | hector check --file packages/core/src/foo.ts \
      --content - --config .hector.yml --format json
  { "status": "pass", "passed_checks": ["biome-check"] }   # hector misses it
```

Net effect for this project: the PreToolUse gate, and all the `--content -` + Python `search/replace` synthesis machinery in the Reasonix hook, are **dead weight** — the synthesized content never reaches biome.

---

## 6. The Reasonix contract is NOT the bug (verified against installed source)

Because the symptom appeared through Reasonix, the first suspicion was adapter/harness drift. It was ruled out. Verified against the installed package `reasonix@0.53.2` (`/Users/chrisarter/.nvm/versions/node/v22.19.0/lib/node_modules/reasonix`, `dist/index.d.ts` + `dist/index.js`):

- **Gating semantics** — `dist/index.d.ts`: `type HookEvent = "PreToolUse" | "PostToolUse" | "UserPromptSubmit" | "Stop";` with the comment *"Shell-command hooks; project scope first, then global. **Exit 0=pass, 2=block on Pre***, other=warn."* → PreToolUse exit 2 really does block.
- **Settings schema** — `interface HookConfig { match?: string; command: string; description?: string; timeout?: number; cwd?: string }`. `match` is *"Anchored regex; `"*"` / omitted = every tool."* The adapter's `^(write_file|edit_file)$` is valid.
- **Payload** — `interface HookPayload { event; cwd; toolName?; toolArgs?; toolResult?; prompt?; lastAssistantText?; turn? }`. Matches the hook's `jq` extraction of `.cwd`, `.toolName`, `.toolArgs`.
- **Tool arg fields** — `dist/index.js` tool registrations: `write_file` → `{ path, content }`; `edit_file` → `{ path, search, replace }`. The hook reads exactly these keys.

Conclusion: the adapter is correct; Reasonix behaves as the adapter assumes. **Do not spend time re-checking the Reasonix wiring** unless you find a specific contradiction.

---

## 7. The real axis: tools that accept content vs. tools that demand a tree

This is the reframing that matters. "Can a `script` rule be gated pre-write?" reduces to "can the underlying tool accept the content to check from **stdin** (with a path/extension *hint* for config + language), or does it insist on reading the filesystem / the whole project?"

| Reads stdin + path hint → **pre-write is possible** | Path-/tree-bound → **post-write only (inherently)** |
|---|---|
| biome `--stdin-file-path=<p>` | tsc (types span the whole program) |
| eslint `--stdin --stdin-filename <p>` | cargo check / clippy (whole crate) |
| ruff `check --stdin-filename <p> -` | go vet / go build (whole package) |
| prettier `--stdin-filepath <p>` | pytest / any test runner |
| stylelint `--stdin --stdin-filename=<p>` | semgrep over a dir, dir-globbing scripts |
| shellcheck `-`, yamllint `-`, hadolint `-` | anything doing cross-file/import analysis |

Key observation: the right-hand column is essentially the set of tools that **should not be per-edit pre-write gates anyway** — they're whole-*program* analyzers whose answer depends on files *other than the one being edited*. No mechanism (stdin, temp file, anything short of materializing the entire tree) can give `tsc` a single proposed file and get a meaningful type-check. So that "limitation" is a *semantic boundary*, not a fixable tooling gap:

- **single-file checks** (stdin-capable) → belong in **pre-write** gates (Reasonix `PreToolUse`, OpenCode `before`);
- **whole-program / changeset checks** → belong in **post-write / `Stop` / `session` / CI**, where the tree is settled.

Biome's docs (verified via Context7, `biomejs/website`) reinforce the stdin path:
- `--stdin-file-path` *"the file path does not need to exist on the filesystem; the extension is what matters"* → the `write_file`-new-file case works.
- `--stdin-file-path` *"bypasses ignore checks."* → this is a double-edged detail that matters for option choice (§8).

---

## 8. Option space (evaluate these; add your own)

All options assume the *path* stays real (scope matching, baseline, AST language detection already key off the real path and are decided **before** the engine runs — `runner.rs` matches scope on the real path, so any materialization inside the engine does not affect scope).

### Option A — core always pipes `ctx.content` to the script subprocess's stdin
Author opts in by writing their tool's normal stdin invocation in `.hector.yml` (`biome check --stdin-file-path={file}`, `ruff check --stdin-filename {file} -`, …). `{file}` stays a path *hint*.
- **Pros:** ~10-line core change (`engine/capability.rs` spawn gets a piped stdin + a writer thread); **zero new config/flags/placeholders**; core learns nothing tool-specific; the same `content` every other engine already uses; no temp files (no source-tree pollution, no file-watcher churn); new files work; stdin *bypasses* biome ignore so it can't silently false-pass; diagnostics reference the real path. A `{file}`-only rule ignores the piped stdin and behaves exactly as today (writer must tolerate `EPIPE`).
- **Cons:** opt-in, so a naive `{file}` rule silently under-gates pre-write (discoverability burden — mitigate at the edges: scaffold stdin form in `hector init`; warn from `--explain`/a config lint). Only works for stdin-capable tools (but see §7 — the rest shouldn't be pre-write gates). Stdin bypassing the tool's own ignore means Hector's `scope:` becomes the sole filter (usually desired; occasionally surprising). Touches `capability.rs`, including the Linux namespaced-clone path (see §10).

### Option B — core materializes proposed content to a **sibling** temp file; `{file}` → temp path
- **Pros:** existing rules unchanged; works for any tool that takes a path; sibling location preserves `biome.json` walk-up + extension.
- **Cons:** writes a temp file *into the source tree* on every proposed edit → file-watcher churn (vite/nodemon HMR), `git status` noise, races with other agents; **silent false-pass risk** — if the tool's own ignore (e.g. `biome.json files.ignore`, dotfile patterns) excludes the temp name, the tool skips it and exits 0 (a security gate that looks like it's protecting but isn't); cleanup must survive `SIGKILL`. The failure mode points the *unsafe* direction.

### Option C — core materializes to a **system-tmp** file
Rejected in `specs/2026-05-25-reasonix-adapter.md` §5-B: a `/tmp/xxx.ts` path breaks the tool's config resolution (no `biome.json` found) and is strictly worse. Listed for completeness.

### Option D — don't change core; improve adapter UX only
Keep the limitation; make pre-write adapters detect a `script`-only `.hector.yml` and warn that script rules won't gate new-file writes; steer authors to `ast`/`semantic`/`session`.
- **Pros:** least work; honest. **Cons:** biome-style gating still never works pre-write, which is the user-visible promise being broken.

---

## 9. Recommended direction (current thinking — falsify this)

**Adopt Option A as a single agnostic primitive: "core always offers proposed content on the script's stdin."** Then push every kind of variability to the layer that owns it:

- **Lifecycle variability → the adapter.** Each harness's `hook.sh` is the only place that knows pre-vs-post, payload shape, and exit-code meaning. Its one job: produce `(path, content)` for the proposed/landed state and call `hector check --file <path> --content -`.
- **Tool variability → `.hector.yml`.** The script string holds the tool's stdin incantation. Author-owned; not core surface.
- **Core → one content model.** `(path, content)` in; content exposed on stdin **and** in `RuleContext`; verdict out. Identical across Claude Code, Reasonix, OpenCode, pi, aider, pre-commit.

Why this resolves the recurring cross-harness problem: a stdin-style `script` rule then yields the **same verdict in every harness** — pre-write the piped bytes are the proposal, post-write they are the on-disk file, the rule and engine behave identically. Today `script` rules are *inconsistent* (work post-write, silently no-op pre-write), which is exactly the transcript bug.

Explicit non-goals (to avoid the per-situation-flag code smell the owner flagged):
- No temp-file materialization in core (Option B/C) — *where* to place the file to satisfy each tool's config/ignore rules is itself tool-specific.
- No `stdin: true` key, no `{content_file}` placeholder. The opt-in is "your command reads stdin," nothing more.

What stays unsolved **by design:** whole-program tools (`tsc`, `cargo`, test runners) cannot be per-edit pre-write gates. Route them to post-write/`Stop`/`session`/CI and document the boundary once.

## 9.1 Investigator's amendment — independent review (2026-05-29)

An independent max-effort review verified this brief against the code and empirically, and **confirms Option A as the mechanism**, with grounding and three refinements.

**Verified.** Every load-bearing claim holds: root cause is `script.rs:13-20` discarding `ctx.content` (content is intact at `runner.rs:727` and handed identically to all engines at `737-744`); AST/semantic honor content via `context.rs:44-50`; biome's `--stdin-file-path` behavior (path need not exist, extension decides language, **stdin bypasses ignore**) reconfirmed against current biome docs. Empirical anchor (hector 0.1.0): the §4 repro reproduces both directions, and a stdin-form rule (`! grep -q FORBIDDEN`, no `{file}`) **passes** even with `FORBIDDEN` piped via `--content -` — because the child inherits hector's fd 0, which is at EOF after `--content` is consumed. The bare predicate `printf FORBIDDEN | sh -c '! grep -q FORBIDDEN'` exits 1. The gap between those two is exactly the content Option A delivers — proving A both necessary and a sufficient mechanism.

**Option A is deadlock-safe to build (answers §11 Q1).** The spawn already drains stdout/stderr on dedicated reader threads *before* waiting (`capability.rs:468-469` fast path, `334-335` clone path), so a concurrent stdin *writer* thread cannot deadlock. SIGPIPE is `SIG_IGN` (no signal-disposition code in the repo), so a writer hitting a closed pipe gets `BrokenPipe`, not a fatal signal. Blast radius is one production caller (`script.rs:51`). Required correctness details: (1) the writer must own `ChildStdin` and **drop it after writing** to deliver EOF — without it, `--stdin-file-path` tools block until the 5s timeout → exit 124 → engine-error exit 3; (2) swallow `BrokenPipe`; (3) on timeout **detach** the writer (symmetric with reader detach at `485-486`/`349-350`). The Linux clone path needs an `O_CLOEXEC` stdin pipe + one async-signal-safe `dup2(stdin_r, 0)` in `child_exec` (~`296`); it is CI-only verifiable and runs only when `network: false` (non-default), so the default path is the easy one.

**Refinement 1 — the boundary must be *detectable*, not merely documented.** Option A makes only the *stdin-capable* subset trustworthy pre-write. A tree-bound tool (`tsc`, `cargo`, test runners) run pre-write does not fail — it **silently lies** (reads the stale on-disk tree; under-gates if clean, false-positive-blocks if the tree has an unrelated error). That is isomorphic to the §5 biome `edit_file` incident — the same silent-wrongness this brief exists to kill. §9's "document the boundary once" is too soft for a silent failure. Core knows the exact under-gating moment (`engine: script` + authoritative `--content` + script references `{file}`/`HECTOR_FILE`) and should surface a note via **`--explain` + `hector init` scaffolding + docs** — *not* an always-on stderr line (`capability.rs:393-398` records why per-process stderr leaks: ~3 hector processes per edit). Defer any `stage: pre|post|any` annotation (flag-smell) until a warning proves insufficient.

**Refinement 2 — §7's "inherently post-write only" is a priced choice, not physics.** You don't need the whole tree, only the one edited file *in place*. An **Option H** (adapter writes proposed content to the real path → runs hector post-write → restores) closes the tree-bound gap and is rejected *for cause*: unapproved bytes briefly live on disk (a crash window defeats pre-write gating), watcher/HMR/multi-agent races, and per-adapter transactional restore complexity. Stating it as a declined option is more defensible than "impossible," and justifies why the boundary exists. The honest support axis is **per-tool (stdin-capable vs tree-bound), not per-harness**: any harness producing `(path, content)` is uniformly supported.

**Refinement 3 — Option A activates two dormant adapter bugs; ship adapter content-fidelity tests with it.** The Reasonix hook's synthesis is currently dead weight (script ignores `--content`) but byte-wrong: `write_file` (`adapters/reasonix/hooks/hook.sh:106`, `jq -r`) **appends** a trailing newline; `edit_file` (`hook.sh:119-144`, `$(python3 …)`) **strips** trailing newlines. Both go live under Option A → spurious "missing/extra final newline" diagnostics. A false-newline *block* erodes trust faster than the current silent miss, so Option A must land with adapter golden-content tests asserting byte-exact piped content. (`multi_edit` remains an unchecked-edit no-op at `hook.sh:146-152`.) Relatedly, the edit→content transform is harness-agnostic and is a candidate to hoist into core (one tested implementation) rather than reimplement per adapter hook.

**Net:** adopt Option A; pair it with a regression test of §4 plus the writer correctness details, a detectable tree-bound boundary surfaced via `--explain`, and adapter content-fidelity tests in the same change.

---

## 10. Constraints an implementer must respect (repo rules)

- **Coverage gate:** Rust files under `crates/*/src/` must hit ≥80% **region** coverage, enforced per-file in CI (`scripts/ci-coverage.sh`, cargo-llvm-cov). New code must bring its file to the gate.
- **Local coverage caveat:** the maintainer's box (Homebrew rustc, no `llvm-tools-preview`) can't run `ci-coverage.sh` locally, and the Linux `cfg` paths in `capability.rs` can't be cross-compiled locally — so any `capability.rs` stdin change to the **Linux namespaced-clone** path is CI-verified only. Account for this; don't assume local proof.
- **Cognitive complexity** capped at 15 per function (clippy). Refactor over `#[allow]`.
- **Bug fixes start with a failing test** (the §4 repro is the natural seed: a `script` rule + `--content` that disagrees with disk).
- **Verdict shape is a stability surface** (`verdict.rs`); this change should need **no** verdict/telemetry schema change — it's behavior-only inside the `script` engine + spawn.
- **Working-tree state (uncommitted, unrelated):** `commands/check.rs`, `adapters/claude-code/hooks/hook.sh` + README, and two test files have in-flight edits for a *different* feature (claude-code switching to `--diff` gating + `--emit-semantic-payload` over diffs). The fix here is orthogonal (engine/spawn) — don't clobber that WIP. Untracked: `docs/superpowers/plans/2026-05-28-pi-adapter.md`.

---

## 11. Open questions for investigators

1. **Is "always pipe stdin" truly side-effect-free for existing `{file}` rules?** Enumerate tools that behave differently when fd 0 is a pipe with bytes vs. inherited/closed. (Linters: expected none. Interactive or `isatty`-sensitive scripts: maybe.) Is the `EPIPE`-tolerant writer-thread the right shape, and does it interact safely with the existing bounded-output reader threads + timeout in `capability.rs:452-499`?
2. **Opt-in vs. discoverability.** Is the silent under-gating of naive `{file}` rules acceptable if mitigated by `hector init` scaffolding + an `--explain`/config-lint warning? Or is implicit "do the right thing" worth more core complexity? Could a lint *detect* "script rule reads `{file}` but not stdin, and a pre-write context is in play" reliably?
3. **Should the single-file/whole-program boundary be enforced or merely documented?** e.g. a rule annotation (`lifecycle: pre|post|any`) so a `tsc` rule is *refused* in a pre-write call instead of silently reading a stale tree? Does that reintroduce the flag smell, or is it a legitimate semantic, not a tool special-case?
4. **`session` engine overlap.** Whole-program checks belong post-write; the `session` engine already aggregates a changeset. Is the right long-term answer to route "tree-bound" tools through `session` rather than `script`, making this boundary first-class?
5. **Generalization to other pre-write harnesses.** OpenCode `tool.execute.before`, a future Aider pre-edit hook — does the §9 layering hold for them with no further core change? Find a harness whose payload can't produce `(path, content)` cheaply and you've found a hole.
6. **Did the author misframe anything?** Is there a failure in the transcript beyond the documented `script` limitation that points to a *second* bug (e.g. in the Reasonix hook's `edit_file` synthesis, the `multi_edit` no-op, or path relativization)? The author concluded "single root cause"; verify.

---

## 12. Reference index (paths are repo-relative)

- Root cause: `crates/hector-core/src/engine/script.rs:31-51`
- Engine context / why AST/semantic differ: `crates/hector-core/src/engine/context.rs:16-50`; `engine/mod.rs:17-25`
- Runner content plumbing: `crates/hector-core/src/runner.rs:719-744` (dispatch), `945-1000` (`resolve_check_input`, `content_authoritative`)
- Subprocess spawn (where stdin would be piped): `crates/hector-core/src/engine/capability.rs:445-499`
- `--content` CLI flag + its help text documenting the limitation: `crates/hector-cli/src/cli.rs:17-47`; `commands/check.rs:89-120,446-464`
- Reasonix adapter: `adapters/reasonix/hooks/hook.sh`, `adapters/reasonix/README.md`
- Reasonix design + the deferred-limitation note: `specs/2026-05-25-reasonix-adapter.md` (esp. §4, §5, §5A note at lines 114-119)
- Reference adapter (post-write): `adapters/claude-code/hooks/hook.sh`
- Existing tests to extend: `crates/hector-cli/tests/adapter_reasonix.rs`, `cli_check_content.rs`
- Verified third-party source: `reasonix@0.53.2` at `~/.nvm/versions/node/v22.19.0/lib/node_modules/reasonix/dist/{index.d.ts,index.js}`
- The incident project: `/Users/chrisarter/Documents/projects/rbac-node` (`.hector.yml`, `hector-install-session.md`)
