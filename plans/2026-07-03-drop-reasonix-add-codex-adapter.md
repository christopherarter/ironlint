# Drop Reasonix, Add Codex Adapter — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the reasonix harness entirely and add Codex as a first-class `JsonHookSpec` harness whose PreToolUse `hook.sh` gates `apply_patch` edits by translating ironlint exit codes into Codex's `permissionDecision:"deny"` JSON.

**Architecture:** Codex's `hooks.json` uses the same `{"hooks":{"PreToolUse":[…]}}` shape that `sync_hook_array` already writes, so Codex slots into the existing `JsonHookSpec` model with **no new `HarnessKind`**. All novelty lives in `adapters/codex/hooks/hook.sh`: it parses the apply_patch envelope, synthesizes each touched file's post-edit content, runs `ironlint check --file <path> --content -` per file, and emits a well-formed deny JSON on the first block. Reasonix is deleted; most of its tests are *retargeted* to codex (codex takes its slot as the second hook harness).

**Tech Stack:** Rust (cargo workspace, `ironlint-core` + `ironlint-cli`), Bash + embedded `python3` for the hook, `jq` for JSON, `assert_cmd`/`predicates`/`serde_json` for CLI tests.

## Global Constraints

- **Design authority:** `specs/2026-07-02-drop-reasonix-add-codex-adapter-design.md`. Every task implicitly inherits it.
- **Codex block contract (verified against `codex-rs/hooks/src/events/pre_tool_use.rs`):** ALLOW = exit 0 + **empty** stdout; BLOCK = exit 0 + `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"…"}}` on stdout. Exit codes never block. **Malformed stdout fails OPEN** (the edit lands) — so every block path must emit well-formed deny JSON before exiting.
- **Exit-code → verdict mapping** (in the hook): ironlint `0`→allow; `2`→deny(message); `4`→deny(trust message, fail-closed); `3`→allow by default, deny under `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`; `1`→allow + stderr log.
- **Fail-closed on un-synthesizable patch** (unparseable envelope / non-applying hunk / missing file) → deny.
- **Supported harness order everywhere:** `["claude-code", "codex", "pi", "opencode"]`.
- **`timeout` in the codex hook entry is seconds (30), not ms.**
- **Repo gates (AGENTS.md):** each touched `crates/*/src/*.rs` holds ≥80% region coverage (`scripts/ci-coverage.sh` — CI-only locally), `cargo clippy --all-targets -- -D warnings`, `cargo fmt`, cognitive complexity ≤15 per fn. `Cargo.lock` committed; build/test with `--locked` where CI does.
- **Leave historical artifacts intact:** `specs/2026-05-25-reasonix-adapter.md`, `tests/e2e/init/runs/*`, `docs/audits/*`, `.superpowers/sdd/*`.

---

### Task 1: Capture a real Codex `apply_patch` payload → test fixtures

**Why:** The one contract detail not fully pinned from docs is the exact byte shape of `tool_input.command` for `apply_patch`. Codex 0.141 is installed locally. Capture ground truth so the Task 2 parser is validated against a real sample, not a guess.

**Files:**
- Create: `tests/fixtures/codex/apply_patch_add.json`
- Create: `tests/fixtures/codex/apply_patch_update.json`
- Create: `tests/fixtures/codex/README.md` (provenance note)

- [ ] **Step 1: Register a throwaway logging hook.** Add to `~/.codex/hooks.json` (merge into existing `hooks` object; do not clobber the repo's `.codex/hooks.json`):

```json
{ "hooks": { "PreToolUse": [ { "matcher": "apply_patch|Edit|Write",
  "hooks": [ { "type": "command",
    "command": "cat > /tmp/codex-payload-$(date +%s%N).json; exit 0" } ] } ] } }
```

- [ ] **Step 2: Drive one add and one update.** In a scratch dir, run Codex and ask it to (a) create a new file `foo.py` with `print('hi')`, then (b) change that line to `print('bye')`. Approve the edits. Two payload files land in `/tmp`.

- [ ] **Step 3: Sanitize + save.** Copy the add-file payload → `tests/fixtures/codex/apply_patch_add.json` and the update payload → `apply_patch_update.json`. Replace any absolute/user paths in `cwd` with the literal `__CWD__` (tests substitute the temp project path). Confirm each is one JSON object with `.tool_name == "apply_patch"` and `.tool_input.command` containing `*** Begin Patch`.

Run: `jq -e '.tool_name=="apply_patch" and (.tool_input.command|contains("*** Begin Patch"))' tests/fixtures/codex/apply_patch_add.json`
Expected: `true`

- [ ] **Step 4: Note the exact Update hunk shape** in `tests/fixtures/codex/README.md` — record whether Update sections carry `@@` markers, whether context lines are space-prefixed, and whether `tool_input.command` is a bare envelope or heredoc-wrapped. Task 2's parser must match this sample.

- [ ] **Step 5: Remove the logging hook** from `~/.codex/hooks.json` and delete `/tmp/codex-payload-*.json`.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/codex/
git commit -m "test(codex): capture real apply_patch PreToolUse payloads as fixtures"
```

> If Codex 0.141 is unavailable to the implementer, fall back to the documented format below (Task 2's payloads) and mark `README.md` "synthetic — reconcile against a live capture before release." The parser is defensive (fails closed on a shape it can't read), so a synthetic fixture is safe but not a substitute for the capture.

---

### Task 2: The codex `hook.sh` + its contract test

**Files:**
- Create: `adapters/codex/hooks/hook.sh` (executable)
- Create: `crates/ironlint-cli/tests/hook_contract_codex.rs`

**Interfaces:**
- Consumes: `crates/ironlint-cli/tests/common/mod.rs` — `HookFixture` (fields `project`; methods `new(hook_rel)`, `file(name)`, `stub(code, stdout)`, `run(hook_arg, stdin, extra_env)`), `hook_tools_available()`.
- Produces: the shell contract every codex `run` relies on (ALLOW=empty stdout, BLOCK=deny JSON). No Rust symbols.

- [ ] **Step 1: Write the failing contract test.** Create `crates/ironlint-cli/tests/hook_contract_codex.rs`:

```rust
//! Contract tests for `adapters/codex/hooks/hook.sh` — the PreToolUse hook
//! that gates Codex `apply_patch` edits through `ironlint check`.
//!
//! Unlike the claude-code hook (exit-code block), Codex blocks ONLY via a
//! `permissionDecision:"deny"` JSON on stdout (exit 0); malformed stdout fails
//! OPEN. These tests therefore assert the JSON shape, not the exit code, and
//! parse stdout with serde_json so a future edit that emits garbage-on-block
//! (→ fail-open) is caught. Same harness as `hook_contract_claude_code.rs`:
//! real `hook.sh` under `bash`, a stub `ironlint` on PATH, temp HOME/project.
#![cfg(unix)]

mod common;

use common::HookFixture;
use predicates::prelude::*;

const HOOK: &str = "adapters/codex/hooks/hook.sh";

/// A one-file Add-File apply_patch payload touching `foo.py` (in `.ironlint.yml`
/// scope). `cwd` is the temp project so the hook resolves the config + root.
fn add_payload(cwd: &std::path::Path) -> String {
    let patch = "*** Begin Patch\n*** Add File: foo.py\n+print('hi')\n*** End Patch\n";
    serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": cwd.display().to_string(),
        "tool_input": { "command": patch },
    })
    .to_string()
}

/// An Update-File payload changing `print('old')` → `print('new')` in foo.py.
fn update_payload(cwd: &std::path::Path) -> String {
    let patch = "*** Begin Patch\n*** Update File: foo.py\n@@\n-print('old')\n+print('new')\n*** End Patch\n";
    serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": cwd.display().to_string(),
        "tool_input": { "command": patch },
    })
    .to_string()
}

/// Assert stdout is a well-formed Codex deny verdict whose reason contains `needle`.
fn assert_deny(stdout: &[u8], needle: &str) {
    let v: serde_json::Value =
        serde_json::from_slice(stdout).expect("hook stdout must be well-formed JSON on block");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap_or_default();
    assert!(reason.contains(needle), "reason {reason:?} lacks {needle:?}");
}

#[test]
fn add_allow_on_exit_0_emits_empty_stdout() {
    if !common::hook_tools_available() {
        eprintln!("skipping: jq/python3 not available");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    fx.run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn add_block_on_exit_2_emits_deny_json() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, r#"{"blocks":[{"check":"g","message":"no print statements"}]}"#);
    let out = fx
        .run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "no print statements");
}

#[test]
fn update_block_on_exit_2_emits_deny_json() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    std::fs::write(fx.file("foo.py"), "print('old')\n").unwrap();
    fx.stub(2, r#"{"blocks":[{"check":"g","message":"blocked update"}]}"#);
    let out = fx
        .run("pre-tool-use", &update_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "blocked update");
}

#[test]
fn untrusted_config_emits_trust_deny() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(4, "");
    let out = fx
        .run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "not trusted");
}

#[test]
fn internal_error_allows_by_default() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(3, "");
    fx.run("pre-tool-use", &add_payload(fx.project.path()), &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn internal_error_denies_when_fail_closed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(3, "");
    let out = fx
        .run(
            "pre-tool-use",
            &add_payload(fx.project.path()),
            &[("IRONLINT_FAIL_CLOSED_ON_INTERNAL", "1")],
        )
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "fail-closed");
}

#[test]
fn unparseable_patch_fails_closed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, ""); // ironlint must never even be consulted
    let bad = serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": "this is not a patch envelope" },
    })
    .to_string();
    let out = fx
        .run("pre-tool-use", &bad, &[])
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert_deny(&out, "apply_patch");
}

#[test]
fn delete_only_patch_allows() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, ""); // even if ironlint would block, a delete has nothing to gate
    let patch = "*** Begin Patch\n*** Delete File: foo.py\n*** End Patch\n";
    let payload = serde_json::json!({
        "tool_name": "apply_patch",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": patch },
    })
    .to_string();
    fx.run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}

#[test]
fn malformed_stdin_is_allowed_gracefully() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(0, "");
    fx.run("pre-tool-use", "{not json", &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::contains("malformed"));
}

#[test]
fn non_apply_patch_tool_is_allowed() {
    if !common::hook_tools_available() {
        eprintln!("skipping");
        return;
    }
    let fx = HookFixture::new(HOOK);
    fx.stub(2, "");
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "cwd": fx.project.path().display().to_string(),
        "tool_input": { "command": "echo hi" },
    })
    .to_string();
    fx.run("pre-tool-use", &payload, &[])
        .success()
        .code(0)
        .stdout(predicates::str::is_empty());
}
```

- [ ] **Step 2: Confirm `serde_json` is a dev-dependency of `ironlint-cli`.**

Run: `grep -A6 '\[dev-dependencies\]' crates/ironlint-cli/Cargo.toml | grep -c serde_json`
Expected: `1`. If `0`, add `serde_json = "1"` under `[dev-dependencies]` in `crates/ironlint-cli/Cargo.toml`.

- [ ] **Step 3: Run the test to verify it fails** (hook script does not exist yet)

Run: `cargo test -p ironlint-cli --test hook_contract_codex`
Expected: FAIL — the tests run `bash adapters/codex/hooks/hook.sh` which is missing (`bash: … No such file`), so assertions fail.

- [ ] **Step 4: Write `adapters/codex/hooks/hook.sh`:**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Codex adapter for ironlint. Gates `apply_patch` edits via Codex's PreToolUse
# hook — see specs/2026-07-02-drop-reasonix-add-codex-adapter-design.md.
#
# Codex's block contract is unlike the exit-code hooks:
#   ALLOW = exit 0 with EMPTY stdout.
#   BLOCK = exit 0 with a permissionDecision:"deny" JSON on stdout.
# An exit code NEVER blocks; malformed stdout FAILS OPEN (the edit lands). So
# every block path emits well-formed deny JSON (via jq, with a static fallback)
# before exiting, and the script never dies between "decided to block" and
# "emitted".
#
# stdin: {"tool_name":"apply_patch","cwd":"...",
#         "tool_input":{"command":"*** Begin Patch ... *** End Patch"}}
# arg1 (the hook event name) is accepted and ignored.

WORKDIR=""
cleanup() {
  if [[ -n "${WORKDIR}" && -d "${WORKDIR}" ]]; then
    rm -rf "${WORKDIR}"
  fi
}
trap cleanup EXIT

# Emit a Codex deny verdict and exit 0. Blocking rides on this JSON, never the
# exit code. jq builds it from an arbitrary reason; if jq fails, a static,
# guaranteed-valid deny is printed so a block is NEVER silently dropped.
deny() {
  local reason=$1
  local out
  if out=$(jq -cn --arg r "${reason}" \
      '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}' \
      2>/dev/null); then
    printf '%s\n' "${out}"
  else
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"ironlint blocked this edit"}}'
  fi
  exit 0
}

EVENT=$(cat)

# Malformed stdin must never brick the agent: allow gracefully.
if ! printf '%s' "${EVENT}" | jq empty >/dev/null 2>&1; then
  echo "ironlint: malformed event JSON on stdin — skipping (allow)" >&2
  exit 0
fi

CWD=$(printf '%s' "${EVENT}" | jq -r '.cwd // empty')
PROJECT_ROOT="${CWD:-$(pwd)}"
CONFIG="${PROJECT_ROOT}/.ironlint.yml"

# Skip silently if ironlint isn't configured for this project.
if [[ ! -f "${CONFIG}" ]]; then
  exit 0
fi

TOOL_NAME=$(printf '%s' "${EVENT}" | jq -r '.tool_name // empty')

# Only file edits (apply_patch) are gated. Codex reports tool_name:"apply_patch"
# for file edits even when the matcher aliased Edit/Write. Anything else that
# reaches here (matcher over-broad) is allowed — ironlint gates file edits only.
if [[ "${TOOL_NAME}" != "apply_patch" ]]; then
  exit 0
fi

PATCH=$(printf '%s' "${EVENT}" | jq -r '.tool_input.command // empty')
if [[ -z "${PATCH}" ]]; then
  exit 0
fi

WORKDIR=$(mktemp -d -t ironlint-codex.XXXXXX)

# Parse the apply_patch envelope → synthesize each touched file's post-edit
# content into WORKDIR, printing a "<abs-path>\t<content-tmpfile>" manifest.
# Fail CLOSED (python exit != 0) on an unparseable / non-applying patch: an
# un-gated edit must never be mistaken for an allowed one.
PY_EC=0
IRONLINT_EVENT_JSON="${EVENT}" \
IRONLINT_PROJECT_ROOT="${PROJECT_ROOT}" \
IRONLINT_WORKDIR="${WORKDIR}" \
python3 -c '
import json, os, sys

root = os.environ["IRONLINT_PROJECT_ROOT"]
workdir = os.environ["IRONLINT_WORKDIR"]
ev = json.loads(os.environ["IRONLINT_EVENT_JSON"])
cmd = (ev.get("tool_input") or {}).get("command") or ""

def fail(msg):
    print(msg, file=sys.stderr)
    sys.exit(2)

b = cmd.find("*** Begin Patch")
e = cmd.find("*** End Patch")
if b == -1 or e == -1 or e < b:
    fail("no apply_patch envelope in tool_input.command")
lines = cmd[b:e].splitlines()[1:]  # drop the "*** Begin Patch" marker line

manifest = []
_n = [0]
def emit(relpath, content):
    _n[0] += 1
    tf = os.path.join(workdir, "content-%d" % _n[0])
    with open(tf, "w", encoding="utf-8") as f:
        f.write(content)
    ap = os.path.normpath(os.path.join(root, relpath))
    manifest.append("%s\t%s" % (ap, tf))

def find_block(hay, needle):
    if not needle:
        return None
    for i in range(0, len(hay) - len(needle) + 1):
        if hay[i:i+len(needle)] == needle:
            return i
    return None

def apply_update(relpath, hunk):
    """Splice apply_patch context sub-hunks into the on-disk file. None on miss."""
    try:
        with open(os.path.join(root, relpath), "r", encoding="utf-8") as f:
            cur = f.read()
    except OSError as ex:
        fail("cannot read %s for update: %s" % (relpath, ex))
    final_nl = cur.endswith("\n")
    cur_lines = cur.split("\n")
    if final_nl:
        cur_lines = cur_lines[:-1]
    subs, buf = [], []
    for hl in hunk:
        if hl.startswith("@@"):
            if buf:
                subs.append(buf); buf = []
        else:
            buf.append(hl)
    if buf:
        subs.append(buf)
    for sub in subs:
        before, after = [], []
        for hl in sub:
            if hl[:1] in (" ", "+", "-"):
                tag, text = hl[0], hl[1:]
            else:
                tag, text = " ", hl
            if tag in (" ", "-"):
                before.append(text)
            if tag in (" ", "+"):
                after.append(text)
        idx = find_block(cur_lines, before)
        if idx is None:
            return None
        cur_lines = cur_lines[:idx] + after + cur_lines[idx+len(before):]
    out = "\n".join(cur_lines)
    if final_nl:
        out += "\n"
    return out

i, N = 0, len(lines)
while i < N:
    ln = lines[i]
    if ln.startswith("*** Add File: "):
        rel = ln[len("*** Add File: "):].strip(); i += 1
        added = []
        while i < N and not lines[i].startswith("*** "):
            s = lines[i]
            added.append(s[1:] if s.startswith("+") else s)
            i += 1
        emit(rel, ("\n".join(added) + "\n") if added else "")
    elif ln.startswith("*** Update File: "):
        rel = ln[len("*** Update File: "):].strip(); i += 1
        dest = rel
        if i < N and lines[i].startswith("*** Move to: "):
            dest = lines[i][len("*** Move to: "):].strip(); i += 1
        hunk = []
        while i < N and not lines[i].startswith("*** "):
            hunk.append(lines[i]); i += 1
        content = apply_update(rel, hunk)
        if content is None:
            fail("apply_patch hunk did not apply cleanly to %s" % rel)
        emit(dest, content)
    elif ln.startswith("*** Delete File: "):
        i += 1  # nothing to gate on a deletion
    else:
        i += 1

sys.stdout.write("\n".join(manifest))
if manifest:
    sys.stdout.write("\n")
sys.exit(0)
' > "${WORKDIR}/manifest" 2>"${WORKDIR}/pyerr" || PY_EC=$?

if [[ "${PY_EC}" -ne 0 ]]; then
  deny "ironlint: could not gate apply_patch — $(tr '\n' ' ' < "${WORKDIR}/pyerr")"
fi

# Run ironlint per touched file; first block wins. No manifest lines (e.g. a
# delete-only patch) → nothing to gate → allow. Reading from a file (not a
# pipe) keeps the loop in this shell, so `deny`'s exit ends the whole script.
while IFS=$'\t' read -r ABSPATH CONTENTFILE; do
  if [[ -z "${ABSPATH}" ]]; then
    continue
  fi
  # Skip edits to the policy file itself: its on-disk sha won't match trust
  # mid-edit, so any check would fail the trust gate misleadingly.
  BASENAME="${ABSPATH##*/}"
  if [[ "${BASENAME}" == ".ironlint.yml" || "${BASENAME}" == ".bully.yml" ]]; then
    continue
  fi
  EC=0
  ironlint check --file "${ABSPATH}" --content - --config "${CONFIG}" --format json \
    > "${WORKDIR}/verdict" 2>/dev/null < "${CONTENTFILE}" || EC=$?
  case "${EC}" in
    0) : ;;
    2) deny "$(cat "${WORKDIR}/verdict")" ;;
    4) deny "ironlint is configured here but not trusted — run 'ironlint trust' to enable checks" ;;
    3)
      if [[ "${IRONLINT_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
        deny "ironlint: check errored (exit 3) for ${ABSPATH} — blocking (fail-closed)"
      fi
      echo "ironlint: check errored (exit 3) for ${ABSPATH} — allowing (fail-open default)" >&2
      ;;
    1) echo "ironlint: config/load error (exit 1) — see 'ironlint doctor'" >&2 ;;
    *) echo "ironlint: unexpected ironlint exit ${EC} for ${ABSPATH}" >&2 ;;
  esac
done < "${WORKDIR}/manifest"

exit 0   # every file passed (or fail-open) → allow, empty stdout
```

- [ ] **Step 5: Make it executable.**

Run: `chmod +x adapters/codex/hooks/hook.sh`

- [ ] **Step 6: Reconcile against the real fixture (from Task 1).** Read `tests/fixtures/codex/apply_patch_update.json`'s `.tool_input.command`; confirm the parser's Update handling (context `@@`, ` `/`+`/`-` prefixes) matches. If the live sample omits `@@` or wraps the envelope in a heredoc, adjust the parser — the `*** Begin/End Patch` scan already tolerates a heredoc wrapper.

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p ironlint-cli --test hook_contract_codex`
Expected: PASS (all cases; skips only if `jq`/`python3` absent).

- [ ] **Step 8: Commit**

```bash
git add adapters/codex/hooks/hook.sh crates/ironlint-cli/tests/hook_contract_codex.rs crates/ironlint-cli/Cargo.toml
git commit -m "feat(codex): PreToolUse hook.sh gating apply_patch via deny-JSON, with contract tests"
```

---

### Task 3: Register codex in the core registry (swap out reasonix)

**Files:**
- Modify: `crates/ironlint-core/src/adapter/registry.rs`
- Modify: `crates/ironlint-core/src/adapter/mod.rs:25` (version bump)

**Interfaces:**
- Consumes: `JsonHookSpec`, `SkillSpec`, `HarnessKind::JsonHook`, `sync_hook_array` (unchanged).
- Produces: `all_harnesses()` returns `["claude-code","codex","pi","opencode"]`; `codex_build_entry(command: &str) -> Value`; `is_detected` recognizes `"codex"` via `~/.codex`.

- [ ] **Step 1: Update the failing registry unit tests first (TDD).** In `registry.rs` `#[cfg(test)] mod tests`, retarget reasonix→codex:
  - `four_harnesses_registered`: `assert_eq!(names, vec!["claude-code", "codex", "pi", "opencode"]);`
  - Rename `reasonix_entry_matches_write_tools` → `codex_entry_matches_apply_patch` and rewrite:

```rust
#[test]
fn codex_entry_matches_apply_patch() {
    let e = codex_build_entry("\"/x/hook.sh\" pre-tool-use");
    assert_eq!(e["matcher"], "apply_patch|Edit|Write");
    assert_eq!(e["hooks"][0]["command"], "\"/x/hook.sh\" pre-tool-use");
    assert_eq!(e["hooks"][0]["timeout"], 30);
}
```

  - `detect_reports_presence_per_home`: replace the reasonix assertion with `assert!(!found["codex"]);`
  - Rename `claude_and_reasonix_share_pre_tool_use_but_detect_independently` → `claude_and_codex_share_pre_tool_use_but_detect_independently`; replace every `reasonix` binding/dir with `codex` / `.codex` (both register `PreToolUse`, so the name-keyed guard still applies):

```rust
#[test]
fn claude_and_codex_share_pre_tool_use_but_detect_independently() {
    let harnesses = all_harnesses();
    let claude = harnesses.iter().find(|h| h.name == "claude-code").unwrap();
    let codex = harnesses.iter().find(|h| h.name == "codex").unwrap();
    match (&claude.kind, &codex.kind) {
        (HarnessKind::JsonHook(c), HarnessKind::JsonHook(r)) => {
            assert_eq!(c.array_key, "PreToolUse");
            assert_eq!(r.array_key, "PreToolUse");
        }
        _ => panic!("expected both to be JsonHook"),
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_str().unwrap();
    std::fs::create_dir_all(format!("{home}/.codex")).unwrap();
    let env = env_with(home, home);
    assert!(!is_detected(claude, &env), "claude-code false-positive via ~/.codex");
    assert!(is_detected(codex, &env));

    let tmp2 = tempfile::tempdir().unwrap();
    let home2 = tmp2.path().to_str().unwrap();
    std::fs::create_dir_all(format!("{home2}/.claude")).unwrap();
    let env2 = env_with(home2, home2);
    assert!(is_detected(claude, &env2));
    assert!(!is_detected(codex, &env2), "codex false-positive via ~/.claude");
}
```

  - `skill_dirs_resolve_per_harness`: replace the `// reasonix` block with codex:

```rust
    // codex
    let codex = by("codex");
    assert_eq!(
        (codex.dir_local)(&env),
        PathBuf::from("/home/u/proj/.codex/skills")
    );
    assert_eq!(
        (codex.dir_global)(&env),
        PathBuf::from("/home/u/.codex/skills")
    );
```

  - `embedded_set_covers_on_disk_adapter_files`: change the loop array to `[("claude-code", "hooks"), ("codex", "hooks")]`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ironlint-core adapter::registry`
Expected: FAIL to compile (`codex_build_entry` undefined, no `codex` harness).

- [ ] **Step 3: Implement the registry changes.** In `registry.rs`:
  - Replace the `REASONIX_HOOK` include with:

```rust
const CODEX_HOOK: &str = include_str!("../../../../adapters/codex/hooks/hook.sh");
```

  - Replace `reasonix_build_entry` with:

```rust
pub(crate) fn codex_build_entry(command: &str) -> Value {
    json!({"matcher": "apply_patch|Edit|Write",
           "hooks": [{"type": "command", "command": command,
                      "timeout": 30, "statusMessage": "ironlint check"}]})
}
```

  - Replace the `REASONIX` const with:

```rust
const CODEX: JsonHookSpec = JsonHookSpec {
    settings_local: |e| Some(e.project_root.join(".codex").join("hooks.json")),
    settings_global: |e| e.home.join(".codex").join("hooks.json"),
    array_key: "PreToolUse",
    entry_arg: "pre-tool-use",
    primary: "hook.sh",
    files: &[("hook.sh", CODEX_HOOK)],
    build_entry: codex_build_entry,
};
```

  - Replace the `REASONIX_SKILL` const with:

```rust
const CODEX_SKILL: SkillSpec = SkillSpec {
    dir_local: |e| e.project_root.join(".codex").join("skills"),
    dir_global: |e| e.home.join(".codex").join("skills"),
    source: IRONLINT_CONFIG_SKILL,
};
```

  - In `all_harnesses()`, replace the reasonix `Harness { … }` with:

```rust
        Harness {
            name: "codex",
            kind: HarnessKind::JsonHook(CODEX),
            restart_hint: "Restart Codex, then review+trust the ironlint hook when Codex prompts (non-managed hooks require trust).",
            skill: CODEX_SKILL,
        },
```

  - In `is_detected`, replace the reasonix arm:

```rust
            "codex" => env.home.join(".codex").is_dir(),
```

  - Update the doc comment above `is_detected` (line ~149): replace "claude-code and reasonix" with "claude-code and codex" and "~/.reasonix" with "~/.codex".

- [ ] **Step 4: Bump the adapter version.** In `mod.rs:25`:

```rust
pub const CURRENT_ADAPTER_VERSION: u32 = 2;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ironlint-core adapter::registry`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/adapter/registry.rs crates/ironlint-core/src/adapter/mod.rs
git commit -m "feat(codex): register codex JsonHook harness; drop reasonix from registry; bump adapter version"
```

---

### Task 4: Retarget `ops.rs` and `json_settings.rs` unit tests

**Files:**
- Modify: `crates/ironlint-core/src/adapter/ops.rs` (tests only)
- Modify: `crates/ironlint-core/src/adapter/json_settings.rs` (tests only)

- [ ] **Step 1: Retarget `ops.rs` tests.** These use a `harness("reasonix")` helper that resolves any registered harness by name and are otherwise harness-agnostic. Replace every `"reasonix"` → `"codex"` and every `.reasonix/settings.json` → `.codex/hooks.json` and every `ironlint/adapters/reasonix/` → `ironlint/adapters/codex/` in the test module (lines ~431–685). Rename the test fns accordingly (e.g. `install_reasonix_writes_artifact_sidecar_and_patches_settings` → `install_codex_writes_artifact_sidecar_and_patches_settings`). The `install_reasonix…` assertion `assert!(cmd.contains("adapters/reasonix/hook.sh"))` → `adapters/codex/hook.sh`.

Run: `grep -c reasonix crates/ironlint-core/src/adapter/ops.rs`
Expected after edits: `0`.

- [ ] **Step 2: Retarget `json_settings.rs` tests.** In `sync_is_idempotent_for_identical_entry` and `sync_strips_stale_ironlint_entry_and_keeps_foreign`, replace the reasonix marker/command strings with codex equivalents (`/h/adapters/codex/hook.sh`, marker `/h/adapters/codex/`, and the `match` value `"^(write_file|edit_file|multi_edit)$"` → `"apply_patch|Edit|Write"`). These are sample markers; the assertions don't depend on the specific harness.

Run: `grep -c reasonix crates/ironlint-core/src/adapter/json_settings.rs`
Expected after edits: `0`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p ironlint-core adapter::ops adapter::json_settings`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/ironlint-core/src/adapter/ops.rs crates/ironlint-core/src/adapter/json_settings.rs
git commit -m "test(codex): retarget core adapter ops + json_settings tests reasonix→codex"
```

---

### Task 5: Retarget `doctor.rs` unit tests

**Files:**
- Modify: `crates/ironlint-cli/src/commands/doctor.rs` (tests only, lines ~544–700)

- [ ] **Step 1: Retarget.** Replace every `"reasonix"` → `"codex"` in the `#[cfg(test)] mod tests`:
  - `check_adapters_reports_installed_reasonix_as_pass` → `…_codex_as_pass`; the `find(|h| h.name == "reasonix")` → `"codex"`; the `find(|c| c.name == "reasonix")` → `"codex"`; `.expect("codex reported")`.
  - `check_adapters_reports_broken_adapter_as_fail`: `find(|h| h.name == "reasonix")` → `"codex"`; `remove_dir_all(env.config_home.join("ironlint/adapters/reasonix"))` → `…/adapters/codex`; the result `find` → `"codex"`.
  - `harness_status(...)` helper: `harness: "reasonix"` → `"codex"`.
  - `adapter_check_reports_registered_but_absent_as_fail`: `harness: "reasonix"` → `"codex"`.
  - `adapter_check_warns_when_detected_but_not_installed`: the remediation assertion `contains("ironlint init --harness reasonix")` → `"ironlint init --harness codex"`.
  - `hooks_row_*` tests: `row("reasonix", …)` → `row("codex", …)`.

Run: `grep -c reasonix crates/ironlint-cli/src/commands/doctor.rs`
Expected: `0`.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p ironlint-cli --lib commands::doctor`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/ironlint-cli/src/commands/doctor.rs
git commit -m "test(codex): retarget doctor adapter tests reasonix→codex"
```

---

### Task 6: Retarget `onboard.rs` tests + `init/mod.rs` doc

**Files:**
- Modify: `crates/ironlint-cli/src/commands/init/onboard.rs` (tests, lines ~271, 286–287)
- Modify: `crates/ironlint-cli/src/commands/init/mod.rs:6-7` (doc comment)

- [ ] **Step 1: Retarget onboard tests.**
  - `select_explicit_all_returns_every_harness`: `assert_eq!(names, vec!["claude-code", "codex", "pi", "opencode"]);`
  - `format_outcome_covers_every_variant`: change the two `format_outcome("reasonix", …)` calls to `format_outcome("codex", …)`.

- [ ] **Step 2: Update the `init/mod.rs` doc comment.** Line 6–7: replace `claude-code, pi, opencode, reasonix` with `claude-code, codex, pi, opencode`.

- [ ] **Step 3: Also fix the `run_hook_phase` copy.** In `onboard.rs:19`, the message says "wire all four" — still four harnesses, so no change needed; confirm it still reads correctly.

Run: `grep -rn reasonix crates/ironlint-cli/src/commands/init/`
Expected: no matches.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ironlint-cli --lib commands::init`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/src/commands/init/
git commit -m "test(codex): retarget init onboarding tests + docs reasonix→codex"
```

---

### Task 7: Rewrite CLI integration tests; delete reasonix contract test

**Files:**
- Modify: `crates/ironlint-cli/tests/cli_init_onboarding.rs`
- Modify: `crates/ironlint-cli/tests/cli_e2e_doctor.rs`
- Modify: `crates/ironlint-cli/tests/hook_contract_claude_code.rs` (doc comment only, lines 8–9)
- Delete: `crates/ironlint-cli/tests/hook_contract_reasonix.rs`

- [ ] **Step 1: `cli_init_onboarding.rs` — swap reasonix→codex.** This suite installs/uninstalls a harness via the real `ironlint init` path and asserts on-disk artifacts. Replace throughout:
  - `.args(["init", "--harness", "reasonix", …])` → `"codex"`
  - `home.join(".reasonix")` (the detected-harness dir) → `home.join(".codex")`
  - `home.join(".config/ironlint/adapters/reasonix/hook.sh")` → `…/adapters/codex/hook.sh`
  - The settings-file assertion `home.join(".reasonix/settings.json")` → `home.join(".codex/hooks.json")`
  - The command-substring assertion `.contains("adapters/reasonix/hook.sh")` → `"adapters/codex/hook.sh"`
  - `assert!(s.contains("reasonix"), …)` → `s.contains("codex")`
  - Rename test fns (`init_installs_reasonix_hook_with_yes` → `init_installs_codex_hook_with_yes`, etc.)

  Note codex has BOTH local and global settings targets (unlike reasonix, which was global-only). The `--harness codex` install with a detected `~/.codex` writes to the **global** `~/.codex/hooks.json` when `--global`, or the project `.codex/hooks.json` by default. Keep each existing test's scope flag; assert against whichever path that scope writes (default local → `<project>/.codex/hooks.json`; the fixtures use a temp project as cwd). Verify by reading the emitted plan text.

- [ ] **Step 2: `cli_e2e_doctor.rs` — swap reasonix→codex.**
  - Line 110: `for harness in ["claude-code", "codex", "pi", "opencode"]`
  - `doctor_reports_installed_reasonix_adapter` → `doctor_reports_installed_codex_adapter`; the `.args([…, "reasonix", …])` install → `"codex"`; the assertions `s.contains("reasonix")` → `s.contains("codex")`.

- [ ] **Step 3: `hook_contract_claude_code.rs` doc comment.** Lines 8–9: replace the reference to `adapters/reasonix/hooks/hook.sh` / `hook_contract_reasonix.rs` with `adapters/codex/hooks/hook.sh` / `hook_contract_codex.rs`.

- [ ] **Step 4: Delete the reasonix contract test.**

Run: `git rm crates/ironlint-cli/tests/hook_contract_reasonix.rs`

- [ ] **Step 5: Run the CLI tests**

Run: `cargo test -p ironlint-cli`
Expected: PASS. Then confirm no stray refs in tests:
Run: `grep -rn reasonix crates/` → Expected: no matches.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-cli/tests/
git commit -m "test(codex): swap CLI integration + e2e doctor tests to codex; drop reasonix contract test"
```

---

### Task 8: Delete the `adapters/reasonix/` tree

**Files:**
- Delete: `adapters/reasonix/` (hooks/hook.sh, hooks/settings.example.json, install.sh, README.md)

- [ ] **Step 1: Confirm nothing in code still references it.**

Run: `grep -rn "adapters/reasonix" crates/ && echo "STILL REFERENCED" || echo "clear"`
Expected: `clear` (the `include_str!` was removed in Task 3).

- [ ] **Step 2: Delete the tree.**

Run: `git rm -r adapters/reasonix`

- [ ] **Step 3: Full build + test to prove the embed still resolves.**

Run: `cargo build --locked && cargo test`
Expected: PASS (the `embedded_set_covers_on_disk_adapter_files` drift test now walks `adapters/codex/hooks/`, which exists from Task 2).

- [ ] **Step 4: Commit**

```bash
git add -A adapters/
git commit -m "chore(reasonix): remove adapters/reasonix tree"
```

---

### Task 9: Docs, skill, CI, e2e-driver sweep + codex README

**Files:**
- Create: `adapters/codex/README.md`
- Modify: `docs/adapters/README.md`, `docs/README.md`, `docs/reference/cli.md`, `docs/operating/diagnostics.md`
- Modify: `AGENTS.md`, `CHANGELOG.md`, `README.md`, `adapters/claude-code/README.md`, `adapters/shared/ironlint-config/SKILL.md`
- Modify: `.claude/skills/adapter-drift-audit/SKILL.md`, `.agents/skills/adapter-drift-audit/SKILL.md`
- Modify: `.github/workflows/ci.yml`
- Modify: `tests/e2e/init/run.sh`, `tests/e2e/init/drive.sh`, `tests/e2e/init/README.md`

- [ ] **Step 1: Write `adapters/codex/README.md`** documenting: the PreToolUse deny-JSON gate; that after `ironlint init` Codex won't run the hook until the user **reviews+trusts** it (non-managed hook trust flow); the **guardrail-not-boundary** caveat (Codex may route around PreToolUse via un-intercepted tool paths / `unified_exec`); and that ironlint writes `~/.codex/hooks.json` (or `<repo>/.codex/hooks.json`), never `config.toml`. Mirror the structure of `adapters/claude-code/README.md`.

- [ ] **Step 2: Sweep the docs.** In each doc file, replace reasonix mentions with codex where the file lists supported harnesses, and update any reasonix-specific contract prose (`~/.reasonix/settings.json`, the `write_file|edit_file|multi_edit` matcher) with codex's (`~/.codex/hooks.json`, `apply_patch|Edit|Write`, deny-JSON block). Where a doc describes the *generic* exit-code hook contract, add a one-line note that codex blocks via `permissionDecision` JSON instead.

- [ ] **Step 3: Update both `adapter-drift-audit/SKILL.md` copies.** The skill takes a harness name arg `(claude-code, pi, opencode, reasonix)` — change to `(claude-code, codex, pi, opencode)`. Add a codex watermark section (source of truth: `developers.openai.com/codex/hooks` + `codex-rs/hooks/`), and note the payload-shape risk (`tool_input.command` apply_patch envelope) as the first thing a codex audit re-verifies.

- [ ] **Step 4: `CHANGELOG.md`** — add an entry under the next version: "Removed the reasonix adapter; added Codex (`apply_patch` PreToolUse gate)."

- [ ] **Step 5: `.github/workflows/ci.yml`** — remove any reasonix-specific matrix entry/step; if the workflow enumerates adapters for a shellcheck/e2e pass, add `codex`.

- [ ] **Step 6: `tests/e2e/init/*`** — in `run.sh`/`drive.sh`/`README.md`, swap reasonix for codex in the harness list the driver exercises (the `runs/*` captured artifacts stay untouched).

- [ ] **Step 7: Verify no live reasonix references remain.**

Run: `grep -rln reasonix . --include='*.md' --include='*.sh' --include='*.yml' --include='*.rs' | grep -vE '(/runs/|/audits/|\.superpowers/|specs/2026-05-25-reasonix-adapter\.md)'`
Expected: no output (only historical files may still match, and they are intentionally excluded).

- [ ] **Step 8: Commit**

```bash
git add adapters/codex/README.md docs/ AGENTS.md CHANGELOG.md README.md adapters/claude-code/README.md adapters/shared/ .claude/skills/ .agents/skills/ .github/ tests/e2e/init/
git commit -m "docs(codex): document codex adapter; sweep reasonix from live docs, skills, CI, e2e driver"
```

---

### Task 10: Full gate pass + code review

**Files:** none new — verification + any coverage top-ups.

- [ ] **Step 1: Format + lint.**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings`
Expected: no diff from fmt, no clippy warnings (watch cognitive-complexity on any touched fn — refactor, don't `#[allow]`).

- [ ] **Step 2: Full test suite.**

Run: `cargo test --locked`
Expected: PASS.

- [ ] **Step 3: Coverage on touched core files.** `registry.rs` gained `CODEX`, `codex_build_entry`, and the codex `is_detected` arm; confirm the retargeted tests exercise them (the swap preserves the reasonix suite's coverage 1:1). If CI's `scripts/ci-coverage.sh` can't run locally (no `llvm-tools-preview` on Homebrew rustc — known), rely on CI; otherwise:

Run: `bash scripts/ci-coverage.sh`
Expected: every touched `crates/*/src/*.rs` ≥80% region coverage.

- [ ] **Step 4: Cleanup build artifacts** this task produced (per AGENTS.md / cleanup-build-artifacts skill): drop any `target/llvm-cov*` scratch, one-off binaries, or `pr.diff`.

- [ ] **Step 5: Request code review** from a separate agent (adversarial-review skill / requesting-code-review), focused on: the hook's fail-open discipline (does every block path emit valid JSON? does `set -e` ever kill it pre-emit?), the apply_patch parser vs. the real captured fixture, and that no reasonix reference survives in live code.

- [ ] **Step 6: Final commit (if review produced fixes).**

```bash
git add -A
git commit -m "chore(codex): address review — <summary>"
```

---

## Self-Review

**Spec coverage** (checked against `specs/2026-07-02-drop-reasonix-add-codex-adapter-design.md`):
- Part A removal footprint → Tasks 3 (registry), 4–7 (tests), 8 (tree), 9 (docs/CI/e2e). ✅
- D1 JsonHookSpec harness (no new variant) → Task 3. ✅
- D2 detection/skill/version bump → Task 3 (is_detected, CODEX_SKILL, `CURRENT_ADAPTER_VERSION=2`). ✅
- D3 block-by-JSON verdict translation → Task 2 hook `deny()` + exit-code case. ✅
- D4 apply_patch → per-file content (Add/Update/Delete/Move, multi-file loop) → Task 2 python parser. ✅
- D5 fail-closed/fail-open discipline → Task 2 (`deny` static fallback, python exit-2 fail-closed, file-redirect loop) + Task 2 well-formed-JSON regression test. ✅
- Testing (JSON contract, multi-file, unparseable, malformed stdin, registry re-arm) → Tasks 2, 3. ✅
- De-risk capture step → Task 1. ✅
- Codex trust-review + guardrail caveat → Task 3 `restart_hint`, Task 9 README. ✅
- Non-goals (no PostToolUse, no config.toml writer, no Bash/MCP gating) → honored; hook only handles apply_patch, registry writes hooks.json. ✅

**Placeholder scan:** no TBD/TODO; every code step shows complete code; retarget steps give exact old→new strings. ✅

**Type consistency:** `codex_build_entry` signature/`matcher`/`timeout:30` identical across Task 2 test, Task 3 impl, Task 3 test. `deny()` shell fn name consistent. `CODEX`/`CODEX_HOOK`/`CODEX_SKILL` names consistent. Harness order `["claude-code","codex","pi","opencode"]` identical in every task that asserts it. ✅
