# ZCode Adapter — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ZCode as the fifth supported harness — a `PreToolUse` pre-write gate inside a multi-file plugin tree (`.zcode-plugin/plugin.json` + `hooks/hooks.json` + `hook.sh`), wired by `ironlint init --harness zcode`.

**Architecture:** ZCode is a Claude-Code plugin-system fork with three renames (`.claude-plugin/`→`.zcode-plugin/`, `${CLAUDE_PLUGIN_ROOT}`→`${ZCODE_PROJECT_DIR}`, settings.json-hook-array→plugin-dir-`hooks/hooks.json`). It is a **hybrid of the two existing `HarnessKind` variants**: it materializes a multi-file plugin directory (like neither — `JsonHookSpec` writes scripts + patches a settings.json; `PluginSpec` writes one TS file) and registers hooks via `hooks/hooks.json` *inside* that tree (no external settings patch). So we add a third `HarnessKind::PluginTree(PluginTreeSpec)` variant. The hook script reuses claude-code's `jq`-based stdin parsing but switches to the reasonix `--file <path> --content -` model (pre-write content on stdin, no synthesized diff) since `PreToolUse` has the proposed content directly.

**Tech Stack:** Rust (workspace: `ironlint-core` lib + `ironlint-cli` bin `ironlint`), `clap`, `serde_json`, `tempfile` (tests), `assert_cmd` (CLI e2e). Bash + `jq` for the hook script. No new dependencies.

**Design spec:** `specs/2026-07-02-zcode-adapter-design.md`

---

## File Structure

### Create

| Path | Responsibility |
|---|---|
| `adapters/zcode/.zcode-plugin/plugin.json` | ZCode plugin manifest (name/version/description). |
| `adapters/zcode/hooks/hook.sh` | PreToolUse gate: parse stdin `tool_input`, run `ironlint check --file <path> --content -`, map exit code (0/2/other). |
| `adapters/zcode/hooks/hooks.json` | Registers `PreToolUse` matcher `Write\|Edit` → `hook.sh`. |
| `adapters/zcode/README.md` | Adapter doc: install, manual ZCode-registration step, exit-code table, known gaps. |
| `docs/adapters/zcode.md` | Reference doc mirroring `docs/adapters/claude-code.md`. |
| `.agents/skills/adapter-drift-audit/references/zcode.md` | Drift-audit reference (contract surface map + watermark). |

### Modify

| Path | Change |
|---|---|
| `crates/ironlint-core/src/adapter/mod.rs` | Add `HarnessKind::PluginTree` variant + `PluginTreeSpec` re-export. |
| `crates/ironlint-core/src/adapter/registry.rs` | Add `ZCODE` `PluginTreeSpec`, `ZCODE_SKILL`, register 5th `Harness`; update `is_detected`; update the `four_harnesses_registered` test to five. |
| `crates/ironlint-core/src/adapter/ops.rs` | Add `install_plugintree` / `uninstall_plugintree` / `status_plugintree` arms; thread through `install`/`uninstall`/`status`/`plan_*`. |
| `crates/ironlint-core/src/adapter/plan.rs` | Add `PluginTree` arm to `PlanStep` rendering. |
| `crates/ironlint-cli/src/commands/init/onboard.rs` | Update the "wire all four" message to "all five"; update `select_explicit_all_returns_every_harness` test. |
| `tests/e2e/init/drive.sh` | Seed `~/.zcode` so zcode is detected in the Docker e2e; assert the plugin tree materializes. |
| `tests/e2e/init/run.sh` | Add zcode assertions: `.zcode-plugin/plugin.json`, `hooks/hook.sh`, `hooks/hooks.json`, skill, sidecar; `doctor` status pass. |
| `tests/e2e/init/README.md` | Add the zcode row to the "What it asserts" table. |
| `docs/adapters/README.md` | Add the zcode row to the adapter table. |

---

## Task 1: `PluginTreeSpec` type + `HarnessKind` variant

**Files:**
- Modify: `crates/ironlint-core/src/adapter/mod.rs`
- Modify: `crates/ironlint-core/src/adapter/registry.rs` (struct definition only)

- [ ] **Step 1: Write the failing test**

Add to `crates/ironlint-core/src/adapter/mod.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn plugintree_spec_is_clone_copy() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<crate::adapter::registry::PluginTreeSpec>();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ironlint-core plugintree_spec_is_clone_copy`
Expected: FAIL — `PluginTreeSpec` not found / `HarnessKind::PluginTree` not found.

- [ ] **Step 3: Write minimal implementation**

In `crates/ironlint-core/src/adapter/mod.rs`, extend the enum:

```rust
pub enum HarnessKind {
    JsonHook(JsonHookSpec),
    Plugin(PluginSpec),
    PluginTree(PluginTreeSpec),
}
```

Add the re-export line next to the existing `pub use registry::{...}`:

```rust
pub use registry::{all_harnesses, JsonHookSpec, PluginSpec, PluginTreeSpec, SkillSpec, SKILL_NAME};
```

In `crates/ironlint-core/src/adapter/registry.rs`, define the struct (place it after `PluginSpec`):

```rust
/// A multi-file plugin directory: materializes `files` (relative paths under
/// `dir`) and writes a sidecar covering all of them. No external settings
/// patch — registration is implicit in the tree (e.g. `hooks/hooks.json`
/// inside the dir). Used by harnesses whose plugin system discovers a whole
/// directory, not a settings hook-array (zcode).
#[derive(Clone, Copy)]
pub struct PluginTreeSpec {
    /// Where to materialize the plugin tree. Returns the dir; the installer
    /// creates it. `None` from both local+global means "not installable in
    /// this scope" (caller surfaces that).
    pub dir: fn(&AdapterEnv) -> PathBuf,
    pub detect: fn(&AdapterEnv) -> bool,
    /// `(relpath, bytes)` — relpaths are forward-slash, relative to `dir`.
    pub files: &'static [(&'static str, &'static str)],
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ironlint-core plugintree_spec_is_clone_copy`
Expected: PASS.

- [ ] **Step 5: Add `PluginTree` stub arms so the crate compiles**

Adding `HarnessKind::PluginTree` makes every existing `match h.kind` non-exhaustive. Three sites must gain a temporary arm **now** (to be replaced by real logic in Tasks 4–6):

- `registry.rs` `is_detected` — add `HarnessKind::PluginTree(t) => (t.detect)(env),` (this is the final form; Task 4 step 4 confirms it).
- `registry.rs` `embedded_artifacts_are_nonempty` test — add `HarnessKind::PluginTree(t) => assert!(!t.files.is_empty(), "{} has no files", h.name),`.
- `ops.rs` `install` / `uninstall` / `status` / `plan_install` / `plan_uninstall` — add `HarnessKind::PluginTree(_) => unimplemented!("plugintree ops land in Task 5/6"),` to each. (These are unreachable until Task 4 registers the harness, but the match must be exhaustive to compile.)

Run: `cargo build -p ironlint-core`
Expected: compiles (with `dead_code` on `PluginTreeSpec` fields — those clear in Task 4).

- [ ] **Step 6: Run clippy + fmt**

Run: `cargo clippy -p ironlint-core -- -D warnings && cargo fmt`
Expected: clean. If clippy errors on `dead_code` for `PluginTreeSpec` fields, add `#[allow(dead_code)]` on the struct and remove it in Task 4 step 1.

- [ ] **Step 7: Commit**

```bash
git add crates/ironlint-core/src/adapter/mod.rs crates/ironlint-core/src/adapter/registry.rs crates/ironlint-core/src/adapter/ops.rs
git commit -m "feat(adapter): add HarnessKind::PluginTree + PluginTreeSpec type"
```

---

## Task 2: The ZCode plugin artifacts (hook.sh, hooks.json, plugin.json)

**Files:**
- Create: `adapters/zcode/.zcode-plugin/plugin.json`
- Create: `adapters/zcode/hooks/hook.sh`
- Create: `adapters/zcode/hooks/hooks.json`
- Test: `tests/fixtures/zcode-hook/` (sample stdin payloads)

- [ ] **Step 1: Create the plugin manifest**

`adapters/zcode/.zcode-plugin/plugin.json`:

```json
{
  "name": "ironlint",
  "version": "0.1.0",
  "description": "Pre-write policy gate for ZCode. Runs ironlint checks against proposed file content before the edit lands on disk.",
  "author": { "name": "dynamik-dev" },
  "license": "Apache-2.0",
  "homepage": "https://github.com/christopherarter/ironlint",
  "repository": "https://github.com/christopherarter/ironlint"
}
```

- [ ] **Step 2: Create the hook script**

`adapters/zcode/hooks/hook.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# ZCode adapter for ironlint.
#
# Wires ZCode's PreToolUse lifecycle event to `ironlint check --file <path>
# --content -` so file edits are BLOCKED against the project's .ironlint.yml
# policy before they land on disk. ZCode's PreToolUse is Claude-Code-shaped:
# exit 0 = allow, nonzero = block; stdin carries the tool-call JSON.
#
# ZCode PreToolUse stdin payload (Claude-Code parity):
#   {
#     "tool_name": "Write" | "Edit",
#     "tool_input": { "file_path": "...", ... }
#   }
# Per-tool tool_input:
#   Write → { file_path, content }            — content IS the post-edit text
#   Edit  → { file_path, old_string, new_string }  — apply unique substitution

EVENT=$(cat)

# Project root: the hook's cwd is the user's project (mirrors claude-code's
# pwd-based resolution). Do NOT use ZCODE_PROJECT_DIR here — that var resolves
# to the *plugin's* own directory (~/.config/ironlint/adapters/zcode/), not
# the user's project, so looking for .ironlint.yml there would always miss.
PROJECT_ROOT="$(pwd)"
CONFIG="${PROJECT_ROOT}/.ironlint.yml"

# Skip silently if ironlint isn't configured for this project.
if [[ ! -f "${CONFIG}" ]]; then
  exit 0
fi

# Parse the event JSON for the changed file. ZCode uses file_path (Claude-Code
# shape); tolerate path as an alias.
FILE=$(echo "${EVENT}" | jq -r '.tool_input.file_path // .tool_input.path // empty')
if [[ -z "${FILE}" ]]; then
  # No file in event payload — nothing to check.
  exit 0
fi

# R3: short-circuit on edits to the policy file itself. The on-disk sha will
# not match the trust store while the user is mid-edit; any ironlint
# invocation would fail the trust gate and surface a misleading "internal
# error". Match by basename so relative and absolute paths both skip.
BASENAME="${FILE##*/}"
if [[ "${BASENAME}" == ".ironlint.yml" || "${BASENAME}" == ".bully.yml" ]]; then
  exit 0
fi

# Compute the proposed post-edit content to pipe to ironlint on stdin.
#   Write → tool_input.content (the full body).
#   Edit  → read the current file, apply old_string→new_string exactly once
#           (unique match required; non-unique or missing → skip the gate,
#           fail-open, mirrors the pi adapter's simulate-failure rule).
TOOL_NAME=$(echo "${EVENT}" | jq -r '.tool_name // empty')
PROPOSED=""
case "${TOOL_NAME}" in
  Write)
    PROPOSED=$(echo "${EVENT}" | jq -r '.tool_input.content // empty')
    ;;
  Edit)
    OLD=$(echo "${EVENT}" | jq -r '.tool_input.old_string // empty')
    NEW=$(echo "${EVENT}" | jq -r '.tool_input.new_string // .tool_input.content // ""')
    if [[ -n "${OLD}" && -f "${FILE}" ]]; then
      BUF=$(cat "${FILE}")
      FIRST=$(printf '%s' "${BUF}" | grep -Fbo "${OLD}" | head -1 | cut -d: -f1 || true)
      if [[ -n "${FIRST}" ]]; then
        # Verify uniqueness: old_string occurs exactly once.
        LAST=$(printf '%s' "${BUF}" | grep -Fbo "${OLD}" | tail -1 | cut -d: -f1 || true)
        if [[ "${FIRST}" == "${LAST}" ]]; then
          # Apply substitution: head before match + NEW + tail after match.
          PROPOSED=$(printf '%s' "${BUF}" | head -c "${FIRST}"; printf '%s' "${NEW}"; printf '%s' "${BUF}" | tail -c +"$((FIRST + ${#OLD} + 1))")
        fi
      fi
    fi
    ;;
esac

# If we couldn't faithfully compute proposed content, skip the gate (fail-open
# on simulate-failure — a miscomputed buffer would risk false blocks).
if [[ -z "${PROPOSED}" ]]; then
  exit 0
fi

# Gate the edit. ironlint exit codes:
#   0 = pass/warn → allow
#   2 = block → block (verdict JSON on stderr)
#   3 = engine internal error → fail-open by default; fail-closed under
#       IRONLINT_FAIL_CLOSED_ON_INTERNAL=1
#   1/other = config/load error → log + allow.
TMP_VERDICT=""
cleanup() {
  if [[ -n "${TMP_VERDICT}" && -f "${TMP_VERDICT}" ]]; then
    rm -f "${TMP_VERDICT}"
  fi
}
trap cleanup EXIT

TMP_VERDICT=$(mktemp -t ironlint-verdict.XXXXXX)
EC=0
# Pipe proposed content on stdin (avoids the here-string trailing newline,
# which would alter content-sensitive checks). `--content -` tells ironlint
# to read the file body from stdin.
printf '%s' "${PROPOSED}" | ironlint check \
  --file "${FILE}" \
  --content - \
  --config "${CONFIG}" \
  --format json \
  > "${TMP_VERDICT}" 2>/dev/null || EC=$?

case "${EC}" in
  0) exit 0 ;;
  2)
    cat "${TMP_VERDICT}" >&2
    exit 2
    ;;
  3)
    if [[ "${IRONLINT_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
      echo "ironlint: internal error — failing closed (IRONLINT_FAIL_CLOSED_ON_INTERNAL=1)" >&2
      [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
      exit 2
    fi
    echo "ironlint: internal error during check — allowing edit; see .ironlint/log.jsonl" >&2
    exit 0
    ;;
  *)
    echo "ironlint: internal error checking ${FILE} (exit ${EC})" >&2
    [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
    exit 0
    ;;
esac
```

- [ ] **Step 3: Create the hooks.json registration**

`adapters/zcode/hooks/hooks.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "${ZCODE_PROJECT_DIR}/hooks/hook.sh",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

> `${ZCODE_PROJECT_DIR}` is ZCode's rename of Claude Code's `${CLAUDE_PLUGIN_ROOT}` — it resolves to the plugin's own root directory at hook-fire time. Since `hook.sh` lives at `<plugin-root>/hooks/hook.sh`, this path is correct and needs no installer rewriting. The var is injected by ZCode when it loads the plugin; if it's ever unset, the hook command would fail to resolve and ZCode would surface a clear error (not a silent miss).

- [ ] **Step 4: Create fixture payloads for the hook test**

`tests/fixtures/zcode-hook/write-event.json`:

```json
{
  "tool_name": "Write",
  "tool_input": {
    "file_path": "REPLACED_AT_RUNTIME/src/example.txt",
    "content": "hello world\n"
  }
}
```

`tests/fixtures/zcode-hook/edit-event.json`:

```json
{
  "tool_name": "Edit",
  "tool_input": {
    "file_path": "REPLACED_AT_RUNTIME/src/example.txt",
    "old_string": "hello",
    "new_string": "goodbye"
  }
}
```

- [ ] **Step 5: Make the hook executable + syntax-check**

Run: `chmod +x adapters/zcode/hooks/hook.sh && bash -n adapters/zcode/hooks/hook.sh`
Expected: exit 0, no syntax errors.

- [ ] **Step 6: Commit**

```bash
git add adapters/zcode/ tests/fixtures/zcode-hook/
git commit -m "feat(zcode): add plugin manifest, PreToolUse hook script, hooks.json"
```

---

## Task 3: Failing registry test for the fifth harness

**Files:**
- Modify: `crates/ironlint-core/src/adapter/registry.rs` (test only)

- [ ] **Step 1: Write the failing tests**

In `crates/ironlint-core/src/adapter/registry.rs` `#[cfg(test)] mod tests`, update the existing `four_harnesses_registered` test and add a new one:

```rust
#[test]
fn five_harnesses_registered() {
    let names: Vec<_> = all_harnesses().iter().map(|h| h.name).collect();
    assert_eq!(
        names,
        vec!["claude-code", "reasonix", "pi", "opencode", "zcode"]
    );
}

#[test]
fn zcode_is_a_plugintree_harness() {
    let z = all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    assert!(matches!(z.kind, HarnessKind::PluginTree(_)));
}

#[test]
fn zcode_detects_via_home_zcode_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_str().unwrap();
    std::fs::create_dir_all(format!("{home}/.zcode")).unwrap();
    let env = env_with(home, home);
    let found: std::collections::BTreeMap<_, _> =
        crate::adapter::detect(&env).into_iter().collect();
    assert!(found["zcode"]);
}

#[test]
fn zcode_not_detected_when_home_zcode_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_str().unwrap();
    let env = env_with(home, home);
    let found: std::collections::BTreeMap<_, _> =
        crate::adapter::detect(&env).into_iter().collect();
    assert!(!found["zcode"]);
}
```

Also **delete** the old `four_harnesses_registered` test (replaced by `five_harnesses_registered`).

Also **extend** the existing `detect_reports_presence_per_home` test to assert zcode detection. The test currently creates `.claude` and `.pi` dirs and asserts `found["claude-code"]` and `found["pi"]` but doesn't touch zcode. Add a `.zcode` dir and assert both directions:

```rust
std::fs::create_dir_all(format!("{home}/.zcode")).unwrap();
// ... after the existing assertions:
assert!(found["zcode"]);
```

Also **update** the existing `embedded_set_covers_on_disk_adapter_files` drift guard to include zcode. The test currently iterates over `[("claude-code", "hooks"), ("reasonix", "hooks")]` and asserts every `.sh` file on disk is embedded. Extend the tuple list to add `("zcode", "hooks")` so a future commit that adds a file to `adapters/zcode/hooks/` without embedding it is caught:

```rust
for (harness, subdir) in [
    ("claude-code", "hooks"),
    ("reasonix", "hooks"),
    ("zcode", "hooks"),
] {
```

The test's inner match currently extracts `JsonHookSpec` to read its `.files` list. Generalize it to also handle `PluginTree` (zcode) by extracting the embedded-files list from either variant:

```rust
let embedded_files: Vec<&'static str> = match &all_harnesses()
    .into_iter()
    .find(|h| h.name == harness)
    .unwrap()
    .kind
{
    HarnessKind::JsonHook(s) => s.files.iter().map(|(f, _)| *f).collect(),
    HarnessKind::Plugin(t) => unreachable!("{} has no hooks/ dir", harness),
    HarnessKind::PluginTree(t) => t.files.iter().map(|(f, _)| *f).collect(),
};
// ... then use embedded_files in the assertion below:
for entry in std::fs::read_dir(&dir).unwrap() {
    let name = entry.unwrap().file_name().into_string().unwrap();
    if std::path::Path::new(&name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("sh")) {
        assert!(
            embedded_files.iter().any(|f| *f == name),
            "adapters/{harness}/{subdir}/{name} is not embedded in the registry"
        );
    }
}
```

> The `Plugin` arm is `unreachable!` because no `Plugin` harness (pi, opencode) has a `hooks/` dir — they ship a single TS file. The `PluginTree` arm covers zcode.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core five_harnesses_registered zcode_`
Expected: FAIL — `zcode` not in the registry / no `PluginTree` harness.

- [ ] **Step 3: Commit (red)**

```bash
git add crates/ironlint-core/src/adapter/registry.rs
git commit -m "test(adapter): add failing zcode harness registry tests"
```

---

## Task 4: Register the ZCode harness

**Files:**
- Modify: `crates/ironlint-core/src/adapter/registry.rs`

- [ ] **Step 1: Add the embedded-artifact includes + ZCODE spec**

At the top of `crates/ironlint-core/src/adapter/registry.rs`, add the includes (after the existing `include_str!` block):

```rust
const ZCODE_HOOK: &str = include_str!("../../../../adapters/zcode/hooks/hook.sh");
const ZCODE_HOOKS_JSON: &str = include_str!("../../../../adapters/zcode/hooks/hooks.json");
const ZCODE_PLUGIN_JSON: &str =
    include_str!("../../../../adapters/zcode/.zcode-plugin/plugin.json");
```

> `IRONLINT_CONFIG_SKILL` (the shared `ironlint-config/SKILL.md` bytes) is already defined at the top of `registry.rs` — reuse it for the skill entry in `ZCODE.files`.

Add the `ZCODE` spec (after the `OPENCODE` `PluginSpec` const):

```rust
const ZCODE: PluginTreeSpec = PluginTreeSpec {
    dir: |e| adapters_dir(e).join("zcode"),
    detect: |e| e.home.join(".zcode").is_dir(),
    files: &[
        (".zcode-plugin/plugin.json", ZCODE_PLUGIN_JSON),
        ("hooks/hook.sh", ZCODE_HOOK),
        ("hooks/hooks.json", ZCODE_HOOKS_JSON),
        // The skill is materialized *inside* the plugin tree (D6: ZCode
        // discovers skills under <plugin-dir>/skills/). install_skill is a
        // no-op for PluginTree harnesses (Task 7) — this is the only write.
        ("skills/ironlint-config/SKILL.md", IRONLINT_CONFIG_SKILL),
    ],
};
```

Add `use crate::adapter::adapters_dir;` to the imports at the top of the file if not already present.

- [ ] **Step 2: Add the ZCODE skill spec**

After `OPENCODE_SKILL`:

```rust
const ZCODE_SKILL: SkillSpec = SkillSpec {
    // For a PluginTree harness the skill is materialized *inside* the plugin
    // tree by install_plugintree; these dirs are only used by the standalone
    // install_skill/uninstall_skill path, which is a no-op for PluginTree
    // harnesses. Point them at the conventional zcode locations for safety.
    dir_local: |e| e.project_root.join(".zcode").join("skills"),
    dir_global: |e| e.home.join(".zcode").join("skills"),
    source: IRONLINT_CONFIG_SKILL,
};
```

- [ ] **Step 3: Register the 5th Harness in `all_harnesses`**

In the `all_harnesses()` vec, append after the `opencode` entry:

```rust
Harness {
    name: "zcode",
    kind: HarnessKind::PluginTree(ZCODE),
    restart_hint: "Restart ZCode (or reload plugins) so it picks up the new plugin.",
    skill: ZCODE_SKILL,
},
```

- [ ] **Step 4: Update `is_detected` to handle `PluginTree`**

Replace the `is_detected` function body's match to add the `PluginTree` arm:

```rust
pub(crate) fn is_detected(harness: &Harness, env: &AdapterEnv) -> bool {
    match &harness.kind {
        HarnessKind::JsonHook(s) => match s.array_key {
            "PostToolUse" => env.home.join(".claude").is_dir(),
            _ => env.home.join(".reasonix").is_dir(),
        },
        HarnessKind::Plugin(p) => (p.detect)(env),
        HarnessKind::PluginTree(t) => (t.detect)(env),
    }
}
```

- [ ] **Step 5: Run the registry tests to verify they pass**

Run: `cargo test -p ironlint-core five_harnesses_registered zcode_ detect_reports_presence_per_home`
Expected: PASS.

- [ ] **Step 6: Run clippy + fmt**

Run: `cargo clippy -p ironlint-core --all-targets -- -D warnings && cargo fmt`
Expected: clean. (There will be unused-function warnings for `install_plugintree` etc. — those come in Task 5. If clippy errors on `dead_code`, add `#[allow(dead_code)]` temporarily on `PluginTreeSpec` fields and remove it in Task 5. Prefer: leave as-is and let Task 5 land before re-running clippy if it errors.)

- [ ] **Step 7: Commit**

```bash
git add crates/ironlint-core/src/adapter/registry.rs
git commit -m "feat(adapter): register zcode as the fifth harness (PluginTree)"
```

---

## Task 5: `install` / `uninstall` / `status` for `PluginTree`

**Files:**
- Modify: `crates/ironlint-core/src/adapter/ops.rs`
- Test: `crates/ironlint-core/src/adapter/ops.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing install test**

In `crates/ironlint-core/src/adapter/ops.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn install_zcode_materializes_plugintree_and_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let env = AdapterEnv {
        home: tmp.path().join("home"),
        config_home: tmp.path().join("home").join(".config"),
        project_root: tmp.path().join("proj"),
    };
    std::fs::create_dir_all(&env.config_home).unwrap();
    let h = crate::adapter::all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    let out = install(&h, &env, Scope::Global).unwrap();
    assert_eq!(out.harness, "zcode");
    assert!(matches!(out.result, InstallResult::Installed));

    let dir = crate::adapter::adapters_dir(&env).join("zcode");
    assert!(dir.join(".zcode-plugin/plugin.json").exists());
    assert!(dir.join("hooks/hook.sh").exists());
    assert!(dir.join("hooks/hooks.json").exists());
    assert!(dir.join("skills/ironlint-config/SKILL.md").exists());
    // sidecar present
    assert!(crate::adapter::sidecar_path(&dir).exists());
    // hook script is executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("hooks/hook.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "hook.sh not executable");
    }
}

#[test]
fn install_zcode_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let env = AdapterEnv {
        home: tmp.path().join("home"),
        config_home: tmp.path().join("home").join(".config"),
        project_root: tmp.path().join("proj"),
    };
    std::fs::create_dir_all(&env.config_home).unwrap();
    let h = crate::adapter::all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    install(&h, &env, Scope::Global).unwrap();
    let out2 = install(&h, &env, Scope::Global).unwrap();
    assert!(matches!(out2.result, InstallResult::AlreadyPresent));
}

#[test]
fn uninstall_zcode_removes_tree_and_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let env = AdapterEnv {
        home: tmp.path().join("home"),
        config_home: tmp.path().join("home").join(".config"),
        project_root: tmp.path().join("proj"),
    };
    std::fs::create_dir_all(&env.config_home).unwrap();
    let h = crate::adapter::all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    install(&h, &env, Scope::Global).unwrap();
    uninstall(&h, &env, Scope::Global).unwrap();
    let dir = crate::adapter::adapters_dir(&env).join("zcode");
    assert!(!dir.exists());
    assert!(!crate::adapter::sidecar_path(&dir).exists());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core install_zcode_ uninstall_zcode_removes`
Expected: FAIL — `PluginTree` arm not handled in `install`/`uninstall` (non-exhaustive match or panic).

- [ ] **Step 3: Implement `install_plugintree`**

In `crates/ironlint-core/src/adapter/ops.rs`, add this function (after `install_plugin`):

```rust
fn install_plugintree(
    spec: &PluginTreeSpec,
    env: &AdapterEnv,
) -> Result<InstallResult> {
    let dir = (spec.dir)(env);
    // Read the prior sidecar *before* writing so idempotency is detected
    // against the pre-install state (atomic_write overwrites).
    let prev_sidecar = read_sidecar(&dir)?;
    let mut files = BTreeMap::new();
    for (relpath, bytes) in spec.files {
        let p = dir.join(relpath);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(&p, bytes.as_bytes())?;
        // Executable bit for shell scripts.
        if let Some(ext) = p.extension() {
            if ext == "sh" {
                set_executable(&p)?;
            }
        }
        files.insert((*relpath).to_string(), sha256_hex(bytes.as_bytes()));
    }
    let already = match &prev_sidecar {
        Some(prev) => files == prev.files,
        None => false,
    };
    write_sidecar(
        &dir,
        &AdapterSidecar {
            version: CURRENT_ADAPTER_VERSION,
            files,
        },
    )?;
    Ok(if already {
        InstallResult::AlreadyPresent
    } else {
        InstallResult::Installed
    })
}
```

> `name` and `scope` are not needed — the plugin tree always materializes to a single dir via `spec.dir(env)`. The `install` arm (Step 5) passes only `spec` and `env`.

- [ ] **Step 4: Implement `uninstall_plugintree`**

Add after `install_plugintree`:

```rust
fn uninstall_plugintree(spec: &PluginTreeSpec, env: &AdapterEnv) -> Result<InstallResult> {
    let dir = (spec.dir)(env);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(InstallResult::Installed)
}
```

> Returns `InstallResult::Installed` to match the `uninstall` match's `InstallResult` binding (mirrors `uninstall_plugin` / `uninstall_jsonhook`). `scope` is unused for the same reason as install.

- [ ] **Step 5: Thread `PluginTree` through `install` / `uninstall` / `status`**

In `install`, add the arm:

```rust
pub fn install(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome> {
    let result = match &h.kind {
        HarnessKind::JsonHook(spec) => install_jsonhook(h.name, spec, env, scope)?,
        HarnessKind::Plugin(spec) => install_plugin(spec, env, scope)?,
        HarnessKind::PluginTree(spec) => install_plugintree(spec, env)?,
    };
    // ... rest unchanged
}
```

In `uninstall` (find the existing match), add:

```rust
HarnessKind::PluginTree(spec) => uninstall_plugintree(spec, env)?,
```

In `status` (find the existing `match &h.kind`), add an arm mirroring the `Plugin` arm's shape — `status` returns a `(bool, bool, Option<bool>, Option<bool>)` tuple (installed, registered, intact, current), not a `HarnessStatus`:

```rust
HarnessKind::PluginTree(spec) => {
    let dir = (spec.dir)(env);
    let installed = dir.join(".zcode-plugin/plugin.json").exists();
    let (intact, current) = sidecar_integrity(&dir)?;
    (installed, installed, intact, current)
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ironlint-core install_zcode_ uninstall_zcode_removes`
Expected: PASS.

- [ ] **Step 7: Run the full adapter test suite + clippy**

Run: `cargo test -p ironlint-core adapter && cargo clippy -p ironlint-core --all-targets -- -D warnings`
Expected: all pass, clean.

- [ ] **Step 8: Commit**

```bash
git add crates/ironlint-core/src/adapter/ops.rs
git commit -m "feat(adapter): install/uninstall/status for PluginTree (zcode)"
```

---

## Task 6: `plan_install` / `plan_uninstall` rendering for `PluginTree`

**Files:**
- Modify: `crates/ironlint-core/src/adapter/ops.rs` (this is where `plan_install`/`plan_uninstall` live — confirmed at `ops.rs:212` and `ops.rs:240`)

The `PlanStep` enum (defined in `crates/ironlint-core/src/adapter/plan.rs`) has exactly these variants: `Hook { path }`, `Plugin { path }`, `Patch { path, key }`, `Skill { path }`. There is no `Write`/`WriteSidecar`/`RemoveDir` variant — use `Hook { path }` for each materialized file (it's the variant for hook artifacts) and a single `Hook { path: dir }` for uninstall (mirrors how `JsonHook` uninstall lists the whole adapter dir).

- [ ] **Step 1: Write the failing test**

In `crates/ironlint-core/src/adapter/ops.rs` `#[cfg(test)] mod tests` (next to the existing `plan_install_plugin_lists_plugin_and_skill` at ~line 659):

```rust
#[test]
fn plan_install_zcode_lists_plugintree_files() {
    let tmp = tempfile::tempdir().unwrap();
    let env = AdapterEnv {
        home: tmp.path().join("home"),
        config_home: tmp.path().join("home").join(".config"),
        project_root: tmp.path().join("proj"),
    };
    let h = all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    let steps = plan_install(&h, &env, Scope::Global);
    // plan_install returns Vec<PlanStep> (not Result) — each materialized file
    // becomes a Hook step. PluginTree has 4 files (plugin.json, hook.sh,
    // hooks.json, skills/ironlint-config/SKILL.md). The trailing Skill step
    // is suppressed for PluginTree (the skill is in the tree).
    let hook_paths: Vec<String> = steps
        .iter()
        .filter_map(|s| match s {
            PlanStep::Hook { path } => Some(path.display().to_string()),
            _ => None,
        })
        .collect();
    assert!(hook_paths.iter().any(|p| p.contains("zcode/.zcode-plugin/plugin.json")));
    assert!(hook_paths.iter().any(|p| p.contains("zcode/hooks/hook.sh")));
    assert!(hook_paths.iter().any(|p| p.contains("zcode/hooks/hooks.json")));
    assert!(hook_paths.iter().any(|p| p.contains("zcode/skills/ironlint-config/SKILL.md")));
    // No Patch step for a PluginTree harness (no settings.json to patch).
    assert!(!steps.iter().any(|s| matches!(s, PlanStep::Patch { .. })));
    // No separate Skill step — the skill is a Hook step in the tree.
    assert!(!steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
}

#[test]
fn plan_uninstall_zcode_lists_adapter_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let env = AdapterEnv {
        home: tmp.path().join("home"),
        config_home: tmp.path().join("home").join(".config"),
        project_root: tmp.path().join("proj"),
    };
    let h = all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    let steps = plan_uninstall(&h, &env, Scope::Global);
    // Uninstall removes the whole adapter dir (which includes the skill,
    // since the skill lives inside the plugin tree). No separate Skill step.
    let any_hook_dir = steps.iter().any(|s| match s {
        PlanStep::Hook { path } => path.display().to_string().ends_with("adapters/zcode"),
        _ => false,
    });
    assert!(any_hook_dir);
    assert!(!steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
}
```

Add `use crate::adapter::PlanStep;` to the test module's imports if not already present.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core plan_install_zcode plan_uninstall_zcode`
Expected: FAIL — `PluginTree` arm not handled in `plan_install`/`plan_uninstall` (non-exhaustive match).

- [ ] **Step 3: Add the `PluginTree` arms**

In `plan_install` (ops.rs:212), add the arm to the `match &h.kind` (mirroring how `Plugin` builds its vec):

```rust
HarnessKind::PluginTree(spec) => spec
    .files
    .iter()
    .map(|(relpath, _)| PlanStep::Hook {
        path: (spec.dir)(env).join(relpath),
    })
    .collect(),
```

**Then suppress the trailing `Skill` step for `PluginTree`.** `plan_install` always appends a `PlanStep::Skill` after the match (for the other harnesses, the skill is installed separately). For `PluginTree`, the skill is one of the materialized tree files (`skills/ironlint-config/SKILL.md`), so it's already covered by a `Hook` step — the trailing `Skill` step would be redundant and point to a path that `install_skill` never writes (it's a no-op per Task 7). Guard the append:

```rust
// After the match, before the existing `steps.push(PlanStep::Skill { ... })`:
if !matches!(h.kind, HarnessKind::PluginTree(_)) {
    let skill_dir = skill_base(&h.skill, env, scope).join(SKILL_NAME);
    steps.push(PlanStep::Skill {
        path: skill_dir.join("SKILL.md"),
    });
}
```

In `plan_uninstall` (ops.rs:240), add the arm (mirrors `JsonHook`'s single-`Hook`-for-the-dir pattern):

```rust
HarnessKind::PluginTree(spec) => vec![PlanStep::Hook {
    path: (spec.dir)(env),
}],
```

**Also suppress the trailing `Skill` step in `plan_uninstall` for `PluginTree`** — the skill dir is inside the tree, which the `Hook { path: dir }` step already removes. Apply the same `if !matches!(h.kind, HarnessKind::PluginTree(_))` guard to the `plan_uninstall` skill append.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-core plan_install_zcode plan_uninstall_zcode`
Expected: PASS.

- [ ] **Step 5: Run clippy + fmt**

Run: `cargo clippy -p ironlint-core --all-targets -- -D warnings && cargo fmt`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-core/src/adapter/ops.rs
git commit -m "feat(adapter): plan_install/plan_uninstall for PluginTree (zcode)"
```

---

## Task 7: `install_skill` is a no-op for `PluginTree` harnesses

**Files:**
- Modify: `crates/ironlint-core/src/adapter/ops.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn install_skill_for_zcode_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let env = AdapterEnv {
        home: tmp.path().join("home"),
        config_home: tmp.path().join("home").join(".config"),
        project_root: tmp.path().join("proj"),
    };
    std::fs::create_dir_all(&env.config_home).unwrap();
    let h = crate::adapter::all_harnesses()
        .into_iter()
        .find(|h| h.name == "zcode")
        .unwrap();
    let out = install_skill(&h, &env, Scope::Global).unwrap();
    // The skill is materialized inside the plugin tree by install(); the
    // standalone install_skill path for a PluginTree harness is a no-op so it
    // doesn't duplicate the skill outside the tree.
    assert!(matches!(out.result, InstallResult::Skipped(_)));
    assert!(!env.home.join(".zcode/skills").exists());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ironlint-core install_skill_for_zcode_is_noop`
Expected: FAIL — `install_skill` writes to `~/.zcode/skills/ironlint-config/SKILL.md` instead of skipping.

- [ ] **Step 3: Add the no-op arm to `install_skill`**

In `install_skill`, before the existing body, branch on the kind:

```rust
pub fn install_skill(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome> {
    // For a PluginTree harness, the skill is part of the materialized plugin
    // tree (written by install_plugintree). The standalone skill path would
    // duplicate it outside the tree, so it's a no-op.
    if matches!(h.kind, HarnessKind::PluginTree(_)) {
        return Ok(InstallOutcome {
            harness: h.name,
            result: InstallResult::Skipped("skill is part of the plugin tree".to_string()),
            hint: h.restart_hint,
        });
    }
    // ... existing body unchanged
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ironlint-core install_skill_for_zcode_is_noop`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/adapter/ops.rs
git commit -m "feat(adapter): install_skill is a no-op for PluginTree harnesses"
```

---

## Task 8: End-to-end CLI test — `ironlint init --harness zcode`

**Files:**
- Test: `crates/ironlint-cli/tests/` (find the existing init integration test)

- [ ] **Step 1: Locate the existing init e2e test**

Run: `ls crates/ironlint-cli/tests/ && grep -rln "init --harness\|harness.*pi\|harness.*opencode" crates/ironlint-cli/tests/`
Expected: shows the integration test file that asserts `ironlint init --harness <name>` for the existing four.

- [ ] **Step 2: Write the failing test**

Add to that file (mirror the closest existing harness test, e.g. the opencode one):

```rust
#[test]
fn init_zcode_materializes_plugin_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let config_home = home.join(".config");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(home.join(".zcode")).unwrap(); // detect zcode
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    let bin = assert_cmd::cargo::cargo_bin("ironlint");
    let output = std::process::Command::new(&bin)
        .args(["init", "--harness", "zcode", "--yes", "--global"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .current_dir(&proj)
        .output()
        .expect("ironlint ran");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let dir = config_home.join("ironlint/adapters/zcode");
    assert!(dir.join(".zcode-plugin/plugin.json").exists());
    assert!(dir.join("hooks/hook.sh").exists());
    assert!(dir.join("hooks/hooks.json").exists());
    assert!(dir.join("skills/ironlint-config/SKILL.md").exists());
}
```

- [ ] **Step 3: Run test to verify it passes (should already pass after Tasks 4–7)**

Run: `cargo test -p ironlint-cli init_zcode_materializes_plugin_tree`
Expected: PASS. (If it fails on `--global` flag semantics, check `onboard.rs` for the exact flag name and adjust.)

- [ ] **Step 4: Run the full CLI test suite**

Run: `cargo test -p ironlint-cli`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/tests/
git commit -m "test(cli): e2e ironlint init --harness zcode materializes plugin tree"
```

---

## Task 8.5: Docker e2e — extend `tests/e2e/init/` for zcode

**Files:**
- Modify: `tests/e2e/init/drive.sh`
- Modify: `tests/e2e/init/run.sh`
- Modify: `tests/e2e/init/README.md`

The existing Docker e2e (`tests/e2e/init/`) is an opt-in, clean-room test: it builds a Linux `ironlint` in Docker, seeds harness homes, runs `ironlint init --yes` in a container with a real `$HOME`, and asserts the materialized artifacts landed on the bind-mounted filesystem. ZCode needs the same treatment — seed `~/.zcode`, then assert the plugin tree materialized.

- [ ] **Step 1: Seed `~/.zcode` in `drive.sh`**

In `tests/e2e/init/drive.sh`, add `~/.zcode` to the `mkdir -p` line so detection finds zcode:

```bash
mkdir -p "$HOME/.reasonix" "$HOME/.pi" "$HOME/.config/opencode" "$HOME/.zcode"
```

- [ ] **Step 2: Add zcode assertions to `run.sh`**

In `tests/e2e/init/run.sh`, after the opencode assertions and before the authoring-skill block, add zcode assertions:

```bash
# zcode: plugin tree materialized under the ironlint adapters dir.
ZCODE_DIR="$HOME_DIR/.config/ironlint/adapters/zcode"
exists "$ZCODE_DIR/.zcode-plugin/plugin.json" "zcode plugin.json materialized"
exists "$ZCODE_DIR/hooks/hook.sh" "zcode hook.sh materialized"
executable "$ZCODE_DIR/hooks/hook.sh" "zcode hook.sh is executable"
exists "$ZCODE_DIR/hooks/hooks.json" "zcode hooks.json materialized"
exists "$ZCODE_DIR/skills/ironlint-config/SKILL.md" "zcode skill materialized"
exists "$ZCODE_DIR/.ironlint-adapter.json" "zcode sidecar present"
contains "$ZCODE_DIR/.ironlint-adapter.json" "sha256:" "zcode sidecar has sha256"
```

Also add `zcode` to the `doctor` status-pass loop:

```bash
for h in reasonix pi opencode zcode; do status_pass "$h"; done
```

- [ ] **Step 3: Update `tests/e2e/init/README.md`**

In the "What it asserts" table, add a zcode row:

```markdown
| zcode | `~/.config/ironlint/adapters/zcode/.zcode-plugin/plugin.json` + `hooks/hook.sh` + `hooks/hooks.json` + `skills/ironlint-config/SKILL.md` + `.ironlint-adapter.json` (sha256) |
```

Also update the "Seeds harness homes" bullet in "What it does" to include `~/.zcode`.

- [ ] **Step 4: Run the Docker e2e (opt-in, requires Docker)**

Run: `bash tests/e2e/init/run.sh`
Expected: PASS — all assertions hold including the new zcode ones.

> This test is **not** part of `cargo test` or PR CI (first run compiles ironlint inside the image — slow). It's a manual acceptance check for the onboarding install path against a real filesystem, consistent with the existing harnesses.

- [ ] **Step 5: Commit**

```bash
git add tests/e2e/init/drive.sh tests/e2e/init/run.sh tests/e2e/init/README.md
git commit -m "test(e2e): extend Docker onboarding test for zcode plugin tree"
```

---

## Task 9: Update the "wire all four" message + docs

**Files:**
- Modify: `crates/ironlint-cli/src/commands/init/onboard.rs`
- Modify: `docs/adapters/README.md`
- Create: `docs/adapters/zcode.md`
- Create: `adapters/zcode/README.md`

- [ ] **Step 1: Update the harness-count message**

In `crates/ironlint-cli/src/commands/init/onboard.rs:19`, change:

```rust
"no supported harnesses detected; run `ironlint init --harness all` to wire all four"
```
to:
```rust
"no supported harnesses detected; run `ironlint init --harness all` to wire all five"
```

- [ ] **Step 2: Update the `select_explicit_all_returns_every_harness` test**

In `crates/ironlint-cli/src/commands/init/onboard.rs` (in the `#[cfg(test)] mod tests` block), the existing test asserts `select_harness_names(&["all".to_string()])` returns the four-harness list. Update it to include `zcode`:

```rust
#[test]
fn select_explicit_all_returns_every_harness() {
    let names = select_harness_names(&["all".to_string()]).unwrap();
    assert_eq!(
        names,
        vec!["claude-code", "reasonix", "pi", "opencode", "zcode"]
    );
}
```

Run: `cargo test -p ironlint-cli select_explicit_all_returns_every_harness`
Expected: PASS.

- [ ] **Step 3: Run the render test that asserts the count**

Run: `cargo test -p ironlint-cli render`
Expected: if there's a snapshot asserting "four", it fails. Update with `cargo insta review` if so.

- [ ] **Step 4: Add the zcode row to `docs/adapters/README.md`**

In the adapter table, add a row:

```markdown
| [ZCode](zcode.md) | ZCode (Z.AI) | `ironlint init --harness zcode` | `PreToolUse` hook in a `.zcode-plugin/` plugin tree |
```

- [ ] **Step 5: Create `docs/adapters/zcode.md`**

Mirror `docs/adapters/claude-code.md` structure. Key content:

```markdown
# ZCode adapter

The ZCode adapter runs your IronLint checks every time ZCode edits a file. When an edit breaks a check, ZCode rejects it on the spot, hands ZCode the verdict, and ZCode rewrites the change to comply.

The adapter ships in this repo at `adapters/zcode/`. It wires ZCode's `PreToolUse` lifecycle event to `ironlint check --file <path> --content -`, blocking edits *before* they land on disk (a strictly better posture than post-hoc checking).

## Install

With the `ironlint` binary and `jq` on your `PATH`:

```bash
ironlint init --harness zcode
```

This materializes the plugin tree to `~/.config/ironlint/adapters/zcode/` (`.zcode-plugin/plugin.json` + `hooks/hook.sh` + `hooks/hooks.json` + skill) with a `.ironlint-adapter.json` sidecar. Re-runs are idempotent.

**Then register the plugin in ZCode:** open ZCode → Settings → Plugins → add the directory `~/.config/ironlint/adapters/zcode/` as a local plugin source. (ZCode's marketplace owns its own install cache and sqlite registry; `ironlint init` cannot write there directly.) Restart ZCode so it loads the plugin, then verify:

```bash
ironlint doctor
```

To remove:

```bash
ironlint init --uninstall --harness zcode
```

This removes the materialized tree and sidecar. Unregister the plugin in ZCode's plugin manager separately.

## Exit-code contract

| `ironlint` exit | Behaviour |
|------|-----------|
| `0` (pass) | Allow. |
| `2` (block) | ZCode blocks the tool call; the verdict message is fed back. |
| `3` (internal error) | Fail-open (log + allow) by default; set `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1` to fail closed. |
| `1` / other (config error) | Log to stderr, allow. |

## Known gaps (v1)

- **Marketplace registration is manual.** ZCode's plugin install cache (`~/.zcode/cli/plugins/cache/`) and sqlite registry are owned by ZCode's marketplace flow; `ironlint init` materializes the tree but you must point ZCode at it in Settings → Plugins.
- **`Edit` fuzzy-match fallback** can't be faithfully simulated, so non-unique `old_string` matches skip the check (fail-open on simulate-failure). Exact + unique `old_string` edits check normally.
- **`bash`-tool shell-out** (`cat > foo`, redirections) bypasses the check — universal across all adapters.
- **`${ZCODE_PROJECT_DIR}` in hook commands** is observed in ZCode plugin `mcpServers` config but not yet confirmed for hook `command` strings; the hook falls back to `pwd` if it's unset, so a wrong variable degrades to correct-cwd behavior, not a hard failure.

## Diagnostic

If the check isn't firing:

1. `ironlint --version` runs on `PATH`.
2. `.ironlint.yml` is present in the project root and trusted (`ironlint trust`).
3. The plugin is registered in ZCode (Settings → Plugins shows `ironlint` enabled).
4. `jq` is on `PATH` (the hook script parses stdin with it).
5. Run `ironlint doctor` for a structured health report.
```

- [ ] **Step 6: Create `adapters/zcode/README.md`**

Mirror `adapters/pi/README.md` structure, adapted for ZCode (install command, manual-registration step, exit-code table, known gaps — same content as `docs/adapters/zcode.md` but scoped to the adapter directory).

- [ ] **Step 7: Run fmt + the full test suite**

Run: `cargo fmt && cargo test`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/ironlint-cli/src/commands/init/onboard.rs docs/adapters/README.md docs/adapters/zcode.md adapters/zcode/README.md
git commit -m "docs(zcode): adapter reference + README; update harness count to five"
```

---

## Task 10: Adapter-drift-audit reference

**Files:**
- Create: `.agents/skills/adapter-drift-audit/references/zcode.md`

- [ ] **Step 1: Create the reference file**

Mirror `.agents/skills/adapter-drift-audit/references/claude-code.md` structure. Content:

```markdown
# ZCode — harness intel

Reference for `adapter-drift-audit zcode`. Audits `adapters/zcode/` against ZCode's current contract.

## Thesis

ZCode is Z.AI's agentic terminal coding tool ("Official Harness for GLM-5.2"), a Claude-Code plugin-system fork. Its integration surface, as the adapter uses it:

- **Lifecycle hooks** fire on named events; a hook is a shell command registered in `hooks/hooks.json` *inside a plugin directory*. The adapter wires `PreToolUse` (gate each edit before disk).
- **Hooks communicate by exit code.** Exit `0` = allow, nonzero = block (PreToolUse). The hook receives the tool-call JSON on stdin.
- **Plugins** bundle hooks, skills, commands, agents, MCP servers behind a `.zcode-plugin/plugin.json` manifest (the ZCode rename of Claude Code's `.claude-plugin/`). `${ZCODE_PROJECT_DIR}` (rename of `${CLAUDE_PLUGIN_ROOT}`) is injected at hook-fire time.
- **Plugin discovery** is via the marketplace (`~/.zcode/cli/plugins/cache/<marketplace>/<plugin>/<version>/`) + a sqlite registry (`~/.zcode/cli/db/db.sqlite`). The adapter materializes to `~/.config/ironlint/adapters/zcode/` and the user registers that dir in ZCode's plugin manager.

Use this to judge a drift's *impact*: a renamed hook-payload field silently breaks gating (high impact); a new optional manifest key the adapter ignores is cosmetic.

## Doc sources

| Source | Use for | Pointer |
|---|---|---|
| Web (primary) | Hook events, matcher, exit-code contract, plugin manifest | `https://zcode.z.ai/en/newdocs/hook` and `https://zcode.z.ai/en/docs/plugin` |
| Live install (ground truth) | `.zcode-plugin/plugin.json` shape, `hooks/hooks.json` format, `${ZCODE_PROJECT_DIR}` var, install paths | `~/.zcode/cli/plugins/cache/zcode-plugins-official/*/0.1.0/` |
| Claude Code parity | stdin payload field names (`tool_input.file_path` / `content` / `old_string`+`new_string`), event semantics | `https://code.claude.com/docs/en/hooks` (ZCode consumes `claude-plugins-official`) |

Authority: the **live install** is the most reliable source (the SPA docs render only via JS). For *what changed*, watch the ZCode release notes / changelog if one is published; otherwise re-derive from a fresh `~/.zcode/` inspection.

## Contract surface map

Grounded in `adapters/zcode/` as of 2026-07-02. Line numbers are anchors — re-read the file; they speed location but may shift.

| # | Harness contract | Adapter consumer | Verify against |
|---|---|---|---|
| 1 | Hook event name `PreToolUse` | `adapters/zcode/hooks/hooks.json` | ZCode Hook docs |
| 2 | Matcher syntax + file-mutating tool set (`Write\|Edit`) | `adapters/zcode/hooks/hooks.json` | ZCode Hook docs (watch for new mutating tools) |
| 3 | `${ZCODE_PROJECT_DIR}` injection at hook-fire time | `adapters/zcode/hooks/hook.sh` (project-root resolution) | live install mcpServers config; hook docs |
| 4 | **Hook stdin payload** — `tool_input.file_path` / `.path` / `.old_string` / `.new_string` / `.content` | `adapters/zcode/hooks/hook.sh` (jq extraction) | Claude-Code parity; ZCode Hook docs — **most drift-prone** |
| 5 | Hook decision contract: exit `0` = allow, nonzero = block | `adapters/zcode/hooks/hook.sh` (all case arms) | ZCode Hook docs |
| 6 | Plugin manifest schema + location `.zcode-plugin/plugin.json` | `adapters/zcode/.zcode-plugin/plugin.json` | live install; ZCode Plugin docs |
| 7 | `hooks/hooks.json` wrapper format `{ "hooks": { "<Event>": [{ "matcher", "hooks": [{ "type":"command", "command", "timeout" }] }] } }` | `adapters/zcode/hooks/hooks.json` | live install (android-emulator plugin) |
| 8 | Skill `SKILL.md` (shared `ironlint-config`) | `adapters/zcode/skills/ironlint-config/SKILL.md` | ZCode Skill docs |

## Known-fragile spots

Scrutinize these every run — most likely to have moved:

- **Hook stdin field names (#4).** `hook.sh` reads `.tool_input.file_path // .tool_input.path` and `.old_string // .new_string // .content`. A rename or restructure breaks file extraction with no error — the hook exits 0 and gates nothing.
- **`${ZCODE_PROJECT_DIR}` for hook commands (#3).** Confirmed only for `mcpServers` config, not for hook `command` strings. If ZCode doesn't inject it at hook-fire time, the hook falls back to `pwd` (correct cwd), so this degrades gracefully — but verify the variable is actually set.
- **File-mutating tool set (#2).** The matcher is `Write|Edit`. A new mutating tool means edits via it bypass the gate.
- **Marketplace registration UX.** If ZCode adds a way to register a plugin dir programmatically (filesystem or API), the adapter could automate the manual step in `docs/adapters/zcode.md` — flag as ✨ new-capability-not-adopted.

## Watermark

Last verified: 2026-07-02 against ZCode (initial baseline — live install inspected; SPA docs page not rendered, contract derived from plugin manifests + search-cache of Hook docs; **not yet audited**)
```

- [ ] **Step 2: Commit**

```bash
git add .agents/skills/adapter-drift-audit/references/zcode.md
git commit -m "docs(adapter-audit): add zcode harness reference"
```

---

## Task 11: Coverage gate + final verification

**Files:**
- Verify only (no edits unless coverage < 80%).

- [ ] **Step 1: Run the per-file coverage gate**

Run: `bash scripts/ci-coverage.sh`
Expected: every `crates/*/src/` file ≥ 80% region coverage. The new `PluginTree` arms in `ops.rs`/`plan.rs`/`registry.rs` are exercised by Tasks 5–8's tests.

- [ ] **Step 2: If any file is under 80%, add tests**

For each file below the gate, identify the uncovered region (cargo-llvm-cov report) and add a test that exercises it. Re-run `bash scripts/ci-coverage.sh`.

- [ ] **Step 3: Run the full suite + clippy + fmt**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 4: Manual smoke test against the live ZCode install**

Run: `cargo build --release`
Then: `./target/release/ironlint init --harness zcode --yes --global`
Verify: `ls ~/.config/ironlint/adapters/zcode/` shows `.zcode-plugin/`, `hooks/`, `.ironlint-adapter.json`.
Then open ZCode → Settings → Plugins → add `~/.config/ironlint/adapters/zcode/` → enable → restart. Edit a file in a project with `.ironlint.yml`; confirm a blocking check rejects the edit.

- [ ] **Step 5: Clean up build artifacts**

Run: `cargo clean -p ironlint-cli` (drops the one-off release binary; the working `target/` stays).

- [ ] **Step 6: Request code review**

Per repo rules (`AGENTS.md`): request code review from a separate agent before declaring done. The review is then reviewed by the principal engineer.

- [ ] **Step 7: Final commit (if review found fixes)**

```bash
git add -A
git commit -m "fix(zcode): address code review feedback"
```

---

## Self-Review

**1. Spec coverage:**

- D1 (new `HarnessKind::PluginTree` variant) → Task 1. ✓
- D2 (`PreToolUse`, not `PostToolUse`) → Task 2 step 2 (hook script), Task 2 step 3 (hooks.json). ✓
- D3 (reuse claude-code logic, `--content` not `--diff`) → Task 2 step 2. ✓
- D4 (install to `~/.config/ironlint/adapters/zcode/`) → Task 4 step 1 (`dir` closure), Task 5 (materialize). ✓
- D5 (detection via `~/.zcode`) → Task 4 step 4, Task 3 (tests). ✓
- D6 (shared skill, no-op standalone install) → Task 4 step 1 (skill in `ZCODE.files`), Task 4 step 2 (`ZCODE_SKILL`), Task 7 (no-op `install_skill`). ✓
- Risks (SPA docs unverifiable, marketplace registration manual) → Task 10 (watermark notes), Task 9 (docs surface the manual step). ✓

**2. Placeholder scan:** No "TBD"/"implement later"/"add error handling" red flags remain. The `install_plugintree` idempotency check reads the sidecar *before* writing (corrected in-place; the buggy first draft was struck). All code blocks are concrete and copy-pasteable. ✓

**3. Type consistency:** `PluginTreeSpec` fields (`dir`, `detect`, `files`) are consistent across Task 1 (def), Task 4 (use), Task 5 (install reads `spec.files`/`spec.dir`/`spec.detect`), Task 6 (plan reads `spec.dir`/`spec.files`). `install_plugintree` and `uninstall_plugintree` both return `Result<InstallResult>` (matching the `install`/`uninstall` match bindings). The `status` `PluginTree` arm returns the `(bool, bool, Option<bool>, Option<bool>)` tuple (matching `status_plugin`'s shape, not `HarnessStatus`). `HarnessKind::PluginTree(spec)` match arm consistent throughout. ✓

**4. Compilation order:** Task 1 adds the variant + stub arms in `ops.rs`/`registry.rs` so the crate compiles before any test runs. Tasks 4–6 replace the stubs with real logic. No task leaves the crate in a non-compiling state. ✓

**5. Skill consistency:** The `ironlint-config` skill is materialized as a file in `ZCODE.files` (Task 4 step 1), asserted in the install test (Task 5 step 1), and the `plan_install`/`plan_uninstall` previews suppress the redundant `Skill` step for `PluginTree` (Task 6) since the skill is covered by a `Hook` step in the tree. `install_skill` is a no-op (Task 7). ✓

**Gaps:** None — every spec decision maps to a task.
