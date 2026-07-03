# Drop Reasonix, Add Codex Adapter — Design

## Context

IronLint currently onboards four coding harnesses via `ironlint init`: `claude-code`,
`reasonix`, `pi`, and `opencode` (registry: `crates/ironlint-core/src/adapter/registry.rs`).
Two are `JsonHookSpec` harnesses that patch a settings-style JSON hook array
(`claude-code`, `reasonix`); two are `PluginSpec` harnesses that drop a single TS
plugin file (`pi`, `opencode`).

Reasonix is not worth the maintenance surface — negligible user share. This spec
**removes reasonix entirely** and **adds `codex`** (OpenAI's Codex CLI) as a
first-class `JsonHookSpec` harness with a shell contract test, keeping the supported
set at four: `claude-code`, `codex`, `pi`, `opencode`.

No shim, no migration — consistent with the repo's no-install-base policy (legacy
configs are rejected outright, not migrated).

## Thesis

Codex is gateable **pre-write**. Codex 0.141 ships a `PreToolUse` hook that intercepts
file edits (performed through its `apply_patch` tool) and can **deny** the tool call
before disk is touched. Critically, Codex's block contract differs from the exit-code
hooks: a `PreToolUse` hook blocks by printing a `permissionDecision: "deny"` JSON object
on **stdout** and exiting **0** — an exit code alone does nothing. This makes the codex
`hook.sh` the most defensive of the four adapters, because malformed hook output **fails
open** (the edit lands). The registry model, however, needs no change: Codex's
`hooks.json` uses exactly the `{"hooks":{"PreToolUse":[…]}}` shape that
`sync_hook_array` already writes.

---

## Part A — Remove Reasonix

### Footprint

Delete:

- `adapters/reasonix/` (whole tree: `hooks/hook.sh`, `hooks/settings.example.json`,
  `install.sh`, `README.md`)
- `crates/ironlint-cli/tests/hook_contract_reasonix.rs`

Edit out reasonix from:

- `crates/ironlint-core/src/adapter/registry.rs` — `REASONIX_HOOK` include, the
  `REASONIX` const, `reasonix_build_entry`, `REASONIX_SKILL`, the `all_harnesses`
  entry, the `is_detected` `"reasonix"` arm, and every reasonix unit test
  (`reasonix_entry_matches_write_tools`, the `claude_and_reasonix_share_pre_tool_use…`
  guard, the reasonix arm of `skill_dirs_resolve_per_harness`, the reasonix arm of
  `embedded_set_covers_on_disk_adapter_files`, and the `detect_reports_presence_per_home`
  reasonix assertions). Most of these are **retargeted to codex** (see Part B), not
  merely deleted, since codex is the replacement `JsonHook` harness.
- `crates/ironlint-cli/src/commands/doctor.rs`, `commands/init/{mod,onboard}.rs`
- `crates/ironlint-cli/tests/cli_e2e_doctor.rs`, `tests/cli_init_onboarding.rs`
- `tests/e2e/init/{run.sh,drive.sh,README.md}` (active driver references only)
- Docs: `docs/adapters/README.md`, `docs/README.md`, `docs/reference/cli.md`,
  `docs/operating/diagnostics.md`
- `AGENTS.md`, `CHANGELOG.md`, `README.md`, `adapters/claude-code/README.md`,
  `adapters/shared/ironlint-config/SKILL.md`
- Both copies of the drift-audit skill: `.claude/skills/adapter-drift-audit/SKILL.md`,
  `.agents/skills/adapter-drift-audit/SKILL.md` (retarget to `codex` as the fourth
  auditable harness)
- CI: `.github/workflows/ci.yml` (any reasonix-specific matrix/step)

**Left intact (repo convention: don't rewrite history):**

- `specs/2026-05-25-reasonix-adapter.md` — historical design record
- `tests/e2e/init/runs/*` — captured run artifacts
- `docs/audits/*`, `.superpowers/sdd/*` — historical reports

---

## Part B — Add Codex

### Ground truth: the Codex hook contract (verified)

Verified three ways: OpenAI's hooks reference (`developers.openai.com/codex/hooks`), the
Codex Rust source (`codex-rs/hooks/src/…`), and the **installed 0.141 binary** (symbol
grep confirms `permissionDecision`, `hookSpecificOutput`, `permissionDecisionReason` are
present in the shipped `codex-darwin-arm64` binary).

| Contract | Value | Source |
|---|---|---|
| Discovery | `hooks.json` or inline `[hooks]` in `config.toml`, at `~/.codex/` (user) or `<repo>/.codex/` (project) | docs |
| `hooks.json` shape | `{"hooks":{"<Event>":[{"matcher":"…","hooks":[{"type":"command","command":"…","timeout":N,"statusMessage":"…"}]}]}}` | docs + live `<repo>/.codex/hooks.json` |
| Enabled by default | yes; `[features].hooks = false` disables (`codex_hooks` = deprecated alias) | docs |
| PreToolUse scope | Bash, **`apply_patch`** (file edits), MCP tools. Not WebSearch, not all shell (`unified_exec` interception incomplete) | docs |
| matcher for file edits | `apply_patch`, `Edit`, or `Write` — all alias to `apply_patch`; hook input always reports `tool_name:"apply_patch"` | docs |
| `timeout` unit | **seconds** (`ConfiguredHandler.timeout_sec: u64`) — contrast reasonix's ms | source |
| stdin payload | one JSON object: common fields `session_id, transcript_path, cwd, hook_event_name, model, permission_mode` + PreToolUse `turn_id, tool_name, tool_use_id, tool_input`. For `apply_patch`, the patch text is in `tool_input.command` | docs |
| **Block** | stdout `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"…"}}`, **exit 0** | source test `permission_decision_deny_blocks_processing` (feeds exit `Some(0)` + deny JSON → blocks) |
| **Allow** | exit 0, **empty** stdout. `permissionDecision:"allow"` is *unsupported* (→ run `Failed`, `should_block:false`) | source tests |
| **Fail-open trap** | invalid/garbage JSON on stdout → run `Failed`, `should_block:false` → **the edit lands** | source test `invalid_json_like_stdout_fails_instead_of_becoming_noop` |
| Trust | non-managed hooks must be reviewed+trusted in Codex before they run; project-local `.codex/` hooks load only when that layer is trusted | docs |

### D1 — Codex is a `JsonHookSpec` harness (no new `HarnessKind`)

`sync_hook_array` already writes `settings["hooks"][key]` as an array — i.e.
`{"hooks":{"PreToolUse":[…]}}` — which is precisely Codex's `hooks.json` shape. So codex
reuses the existing machinery with a new spec:

```rust
const CODEX: JsonHookSpec = JsonHookSpec {
    settings_local:  |e| Some(e.project_root.join(".codex").join("hooks.json")),
    settings_global: |e| e.home.join(".codex").join("hooks.json"),
    array_key: "PreToolUse",
    entry_arg: "pre-tool-use",
    primary: "hook.sh",
    files: &[("hook.sh", CODEX_HOOK)],
    build_entry: codex_build_entry,
};

pub(crate) fn codex_build_entry(command: &str) -> Value {
    json!({"matcher": "apply_patch|Edit|Write",
           "hooks": [{"type": "command", "command": command,
                      "timeout": 30, "statusMessage": "ironlint check"}]})
}
```

`matcher` is `apply_patch|Edit|Write` (belt-and-suspenders — matches whether Codex
reports the canonical tool name or an alias). `timeout` is **30 seconds** (not `30000`).
The `hook.sh` bytes are embedded from `adapters/codex/hooks/hook.sh` via `include_str!`,
same as claude-code.

### D2 — Detection, skill, adapter version

- **Detection** (`is_detected`): the `JsonHook` arm keys on harness name (claude-code and
  codex both register `PreToolUse`, so `array_key` can't disambiguate). Add
  `"codex" => env.home.join(".codex").is_dir()`.
- **Skill**: shared `ironlint-config` `SKILL.md` → `CODEX_SKILL` with
  `dir_local: <repo>/.codex/skills`, `dir_global: ~/.codex/skills`.
- **Adapter version**: bump `CURRENT_ADAPTER_VERSION` `1 → 2` (the embedded harness set
  changed; this drives doctor's "outdated — re-run `ironlint init`" check).

### D3 — The codex `hook.sh`: block-by-JSON

A new script at `adapters/codex/hooks/hook.sh`, structurally mirroring
`adapters/claude-code/hooks/hook.sh` (same `set -euo pipefail`, `.ironlint.yml`
short-circuit, `.ironlint.yml`/`.bully.yml` self-edit skip, malformed-stdin guard, the
`IRONLINT_FILE`-as-env-not-interpolated safety, the embedded `python3` synthesis block),
but with a **different verdict translation** and **different content synthesis**:

**Verdict translation** — a `deny_json` emitter replaces the exit-2 path. Every block
routes through it:

- ironlint exit `0` → `exit 0` (empty stdout = allow)
- ironlint exit `2` (policy block) → emit deny JSON with the ironlint message as
  `permissionDecisionReason`; `exit 0`
- ironlint exit `4` (untrusted) → emit deny JSON with the "run `ironlint trust`" message;
  `exit 0` (**fail closed** — an untrusted config is never silently allowed)
- ironlint exit `3` (internal) → allow (empty stdout) by default; under
  `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`, emit deny JSON
- ironlint exit `1` (config/load error) → allow, log to stderr (Codex surfaces stderr in
  its event stream, not to the model as a block reason)

`permissionDecisionReason` is built with `jq` (never string-concatenated) so arbitrary
ironlint messages can't produce invalid JSON. This is the crux of the fail-open trap
(D5).

### D4 — `apply_patch` → per-file proposed content

`tool_input.command` carries the OpenAI apply_patch envelope:

```
*** Begin Patch
*** Add File: path/to/new.py
+<line>
*** Update File: path/to/existing.py
[*** Move to: path/to/renamed.py]
@@ <context>
 <ctx>
-<removed>
+<added>
*** Delete File: path/to/gone.py
*** End Patch
```

The embedded `python3` parser locates the `*** Begin Patch … *** End Patch` block within
`tool_input.command` (robust to a bare envelope or a heredoc-wrapped invocation) and, per
touched file, synthesizes the **post-edit content**:

- **Add File** → the added (`+`) lines, `+` stripped
- **Update File** → apply the hunks to the current on-disk file; if `Move to` is present,
  the checked path (and thus extension) is the destination
- **Delete File** → skip (no content to gate)

Each synthesized file is run through `ironlint check --file <path> --content -`. Because
one `apply_patch` call may touch **multiple** files (unlike the single-file claude-code
hook), the hook **loops**: the first file whose check blocks (exit 2/4) produces the deny
JSON and stops; the reason names the offending file. This is the one structural
divergence from claude-code's dispatch.

### D5 — Fail-closed & fail-open discipline (the hard part)

Codex's parser fails **open** on malformed hook output (D3 ground-truth table). So the
script must guarantee: *if it decides to block, a well-formed deny JSON reaches stdout
before the process exits.* Rules:

1. **Never rely on exit code to block** — only the deny JSON blocks.
2. **Unparseable / non-applying patch → fail closed.** If the envelope can't be found, a
   hunk doesn't apply cleanly, or the structure is unrecognized, emit deny JSON naming
   the reason. (Mirrors the claude-code hook's stance on un-synthesizable edits, and the
   project's "fail loud, not silent" thesis.) An un-gated edit is never mistaken for an
   allowed one.
3. **Build → validate → emit → exit.** The deny JSON is assembled and validated
   (`jq empty`) before it's printed; the script must not let `set -e`/`pipefail` kill it
   between "decided to block" and "emitted." Trap-based cleanup only removes temp files.
4. Malformed **stdin** payload → allow gracefully (exit 0, empty stdout), log to stderr —
   a broken event must not brick the agent (matches claude-code).

---

## Testing

**New `crates/ironlint-cli/tests/hook_contract_codex.rs`** (mirrors the reasonix suite's
harness — real `hook.sh` under `bash`, stub `ironlint` on `PATH`, temp `HOME`/project),
asserting the **JSON** contract, not exit codes:

- exit 0 → success, **empty stdout** (allow)
- ironlint exit 2 → exit 0, stdout is valid JSON with
  `.hookSpecificOutput.permissionDecision == "deny"` and the message in
  `permissionDecisionReason` (assert via `jq`, not substring on raw bytes)
- ironlint exit 4 → deny JSON carrying the trust message
- ironlint exit 3 → allow by default; deny JSON under `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`
- malformed stdin → exit 0, empty stdout, stderr notes "malformed"
- **multi-file apply_patch** where the second file blocks → deny JSON naming that file
- **unparseable patch envelope** → deny JSON (fail closed), not empty-stdout allow
- **regression guard**: a block must be delivered as *well-formed* JSON — assert
  `jq empty` passes on stdout — so a future edit that crashes the script mid-block (→
  fail-open) is caught

**Registry unit tests** (`registry.rs`): `four_harnesses_registered` →
`["claude-code","codex","pi","opencode"]`; the shared-`PreToolUse` detection guard
retargeted to **claude-code vs codex**; `skill_dirs_resolve_per_harness`,
`embedded_set_covers_on_disk_adapter_files`, and `detect_reports_presence_per_home`
re-armed for codex; new `codex_entry_matches_apply_patch` asserting `matcher` and
`timeout: 30`.

**Gates** (per `AGENTS.md`): touched `.rs` files hold ≥80% region coverage
(`scripts/ci-coverage.sh`), `cargo clippy --all-targets -- -D warnings`, `cargo fmt`,
cognitive-complexity ≤15. New CLI test wired into the same layer as
`hook_contract_claude_code.rs`.

## De-risking step (before finalizing the parser)

The one contract detail not fully pinned from docs is the exact byte shape of
`tool_input.command` for `apply_patch` (bare envelope vs heredoc-wrapped). Codex 0.141 is
installed locally. **Capture a real PreToolUse payload** by registering a throwaway
logging hook (`command: 'cat >> /tmp/codex-payload.json'`) and driving one `apply_patch`
edit, then lock the parser against that ground-truth sample. This removes the last
assumption from D4.

## Non-goals

- No `PostToolUse` / `Stop` / `SessionStart` codex hooks — only the `PreToolUse` gate.
- No inline-`[hooks]`-in-`config.toml` writer — ironlint writes `hooks.json` (Codex
  supports both; one representation per layer is cleaner and avoids the merge-warning).
- No Codex plugin packaging (`.codex-plugin/plugin.json` + bundled hooks) — the
  user/project `hooks.json` install is sufficient.
- No gating of Bash or MCP tool calls — ironlint's model is file-content checks (matches
  claude-code's `Edit|Write`-only scope).
- No `updated_input` / content rewriting — ironlint blocks, it doesn't mutate edits.

## Risks & mitigations

- **PreToolUse is a guardrail, not a hard boundary.** Codex's own docs: it "can often
  perform equivalent work through another supported tool path"; shell interception
  (`unified_exec`) is incomplete. A determined model can route around the gate. This is a
  Codex limitation, not an implementation gap — documented here so expectations match
  claude-code's stronger posture. *Mitigation: none available; state it in the codex
  adapter README.*
- **Fail-open on malformed output.** Central risk; mitigated by D5 discipline + the
  well-formed-JSON regression test.
- **apply_patch `tool_input.command` shape.** Mitigated by the live-capture de-risking
  step; parser fails closed if the envelope isn't found.
- **Codex hook trust review.** After `ironlint init`, Codex will not run the hook until
  the user reviews+trusts it (first-run trust flow). Unlike the other harnesses' silent
  wiring, this is a required manual step — surface it in the init restart hint
  (`restart_hint` for codex) and the adapter README.
