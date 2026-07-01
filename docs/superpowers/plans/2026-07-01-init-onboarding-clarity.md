# `hector init` Onboarding Clarity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `hector init` harness onboarding show a per-file plan (tagged `detected`/`requested`) and confirm before installing, for both the explicit `--harness` and auto-detect paths.

**Architecture:** Add a structured `PlanStep` model + `plan_install`/`plan_uninstall` planners in `hector-core::adapter` (data only). Add a pure pretty-printer in a new `hector-cli` `render.rs` (pixels only). Rewire `onboard.rs` into one pipeline — resolve harness set → build plans → render → (dry-run stop) → confirm → install — replacing the current fork where explicit installs silently and only auto-detect prompts. Remove the now-obsolete `InstallResult::DryRun` variant and the `dry_run` parameter threaded through the install/uninstall functions.

**Tech Stack:** Rust (Cargo workspace, two crates: `hector-core`, `hector-cli`), `assert_cmd` for CLI integration tests, `tempfile` for isolation. No new dependencies — color is a ~15-line internal ANSI helper gated on `std::io::stdout().is_terminal()`.

## Global Constraints

- No new crate dependencies. Color uses raw ANSI via a tiny internal helper, gated on `stdout().is_terminal()`; non-TTY output contains **no** escape bytes (`\x1b`).
- Rust files under `crates/*/src/` must meet ≥80% **region** coverage per file (branches/short-circuits/match arms). New match arms and the color on/off branch must be exercised by tests.
- Cognitive complexity per function is capped at **15** (clippy). Keep the flow decomposed (`resolve_harnesses`, `build_plans`, `render_plan`, `render_harness`, `confirm_gate`, `apply`) — do not inline into one function.
- `cargo clippy --all-targets -- -D warnings` must pass. `cargo fmt` clean.
- Binary is `hector`. `Cargo.lock` is gitignored — do not commit it.
- The plan describes **what the install touches**, not a diff; post-install result lines (`installed — <hint>`, `already present`, `updated`) are unchanged.
- Home-relative (`~/…`) and project-relative (`./…`) path display; absolute fallback when neither prefix applies. Never splice absolute `$HOME` into displayed paths when a prefix matches.

---

## Task 1: Core `PlanStep` model + planners

Add the structured plan type and the two planner functions. Additive only — nothing is removed yet, everything still compiles.

**Files:**
- Create: `crates/hector-core/src/adapter/plan.rs`
- Modify: `crates/hector-core/src/adapter/ops.rs` (add planners near the install fns; add tests to the existing `tests` module)
- Modify: `crates/hector-core/src/adapter/mod.rs:2-16` (declare `mod plan;`, re-export)

**Interfaces:**
- Consumes: existing private helpers in `ops.rs` — `adapters_dir(env)`, `settings_path(spec, env, scope)`, `plugin_dir(spec, env, scope)`, `skill_base(&h.skill, env, scope)`, const `SKILL_NAME`; registry types `Harness`, `HarnessKind`, `JsonHookSpec` (field `array_key: &'static str`, `files: &'static [(&'static str, &'static str)]`, `primary`), `PluginSpec` (field `filename`), `SkillSpec`.
- Produces:
  - `pub enum PlanStep { Hook { path: PathBuf }, Plugin { path: PathBuf }, Patch { path: PathBuf, key: &'static str }, Skill { path: PathBuf } }` (derives `Debug, Clone, PartialEq, Eq`)
  - `pub fn plan_install(h: &Harness, env: &AdapterEnv, scope: Scope) -> Vec<PlanStep>` — full onboarding footprint: hook/plugin file(s) + settings patch + the `SKILL.md` skill step.
  - `pub fn plan_uninstall(h: &Harness, env: &AdapterEnv, scope: Scope) -> Vec<PlanStep>` — removal footprint: adapter dir (JsonHook) / plugin file (Plugin) + settings patch + skill dir.

- [ ] **Step 1: Create the `PlanStep` type**

Create `crates/hector-core/src/adapter/plan.rs`:

```rust
//! Structured preview of what a harness install/uninstall touches. Core emits
//! this data; the CLI owns all formatting.
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStep {
    /// A hook artifact (`hook.sh`, `synthesize_diff.sh`) — or, for uninstall,
    /// the adapter directory that holds them.
    Hook { path: PathBuf },
    /// A plugin file (`hector.ts`).
    Plugin { path: PathBuf },
    /// A JSON settings patch: the file plus the hook-array key it lands in.
    Patch { path: PathBuf, key: &'static str },
    /// The `SKILL.md` authoring skill — or, for uninstall, its directory.
    Skill { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn patch_step_carries_key() {
        let s = PlanStep::Patch {
            path: PathBuf::from("/x/settings.json"),
            key: "PostToolUse",
        };
        match s {
            PlanStep::Patch { key, .. } => assert_eq!(key, "PostToolUse"),
            _ => panic!("wrong variant"),
        }
    }
}
```

- [ ] **Step 2: Declare and re-export from `mod.rs`**

In `crates/hector-core/src/adapter/mod.rs`, add `mod plan;` alongside the other `mod` lines (after `mod ops;`), and extend the `pub use` block. Change:

```rust
mod json_settings;
mod materialize;
mod ops;
mod registry;
```
to:
```rust
mod json_settings;
mod materialize;
mod ops;
mod plan;
mod registry;
```

And add a re-export line after the existing `pub use ops::{...}` block:

```rust
pub use plan::PlanStep;
pub use ops::{plan_install, plan_uninstall};
```

(The `ops::{...}` re-export list already exports `install, install_skill, status, uninstall, uninstall_skill, HarnessStatus, InstallOutcome, InstallResult` — add `plan_install, plan_uninstall` to that same brace list rather than a second `pub use ops::` line if you prefer; either compiles.)

- [ ] **Step 3: Write failing planner tests**

Add to the `tests` module at the bottom of `crates/hector-core/src/adapter/ops.rs` (it already has `harness(name)` and `env(tmp)` helpers). Add `use crate::adapter::{plan_install, plan_uninstall, PlanStep};` to that module's `use` lines.

```rust
#[test]
fn plan_install_jsonhook_lists_files_patch_and_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let e = env(tmp.path());
    let steps = plan_install(&harness("claude-code"), &e, Scope::Local);
    // two hook files + one patch + one skill
    let hooks = steps.iter().filter(|s| matches!(s, PlanStep::Hook { .. })).count();
    assert_eq!(hooks, 2, "claude-code ships hook.sh + synthesize_diff.sh");
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Patch { key, .. } if *key == "PostToolUse")));
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
}

#[test]
fn plan_install_plugin_lists_plugin_and_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let e = env(tmp.path());
    let steps = plan_install(&harness("pi"), &e, Scope::Local);
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Plugin { .. })));
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
    assert!(!steps.iter().any(|s| matches!(s, PlanStep::Patch { .. })));
}

#[test]
fn plan_install_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let e = env(tmp.path());
    let _ = plan_install(&harness("reasonix"), &e, Scope::Global);
    assert!(!e.config_home.join("hector/adapters/reasonix/hook.sh").exists());
    assert!(!tmp.path().join(".reasonix/settings.json").exists());
}

#[test]
fn plan_uninstall_jsonhook_lists_dir_patch_and_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let e = env(tmp.path());
    let steps = plan_uninstall(&harness("reasonix"), &e, Scope::Global);
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Hook { .. })));
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Patch { .. })));
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
}

#[test]
fn plan_uninstall_plugin_lists_file_and_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let e = env(tmp.path());
    let steps = plan_uninstall(&harness("opencode"), &e, Scope::Local);
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Plugin { .. })));
    assert!(steps.iter().any(|s| matches!(s, PlanStep::Skill { .. })));
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p hector-core plan_install plan_uninstall 2>&1 | tail -20`
Expected: FAIL — `cannot find function plan_install` / `plan_uninstall` (not yet implemented).

- [ ] **Step 5: Implement the planners in `ops.rs`**

Add near the top of `ops.rs` after the imports: extend the existing `use crate::adapter::{...}` to include `PlanStep` (or add `use crate::adapter::plan::PlanStep;`). Then add these functions (place them after `install_skill_file` / before the `uninstall` section):

```rust
// --- plan (preview; writes nothing) ------------------------------------------

/// Full onboarding footprint for `h`: hook/plugin file(s), the settings patch
/// (JsonHook only), and the authoring skill. Computes paths only — no I/O.
pub fn plan_install(h: &Harness, env: &AdapterEnv, scope: Scope) -> Vec<PlanStep> {
    let mut steps = match &h.kind {
        HarnessKind::JsonHook(spec) => {
            let dir = adapters_dir(env).join(h.name);
            let mut v: Vec<PlanStep> = spec
                .files
                .iter()
                .map(|(f, _)| PlanStep::Hook { path: dir.join(f) })
                .collect();
            v.push(PlanStep::Patch {
                path: settings_path(spec, env, scope),
                key: spec.array_key,
            });
            v
        }
        HarnessKind::Plugin(spec) => vec![PlanStep::Plugin {
            path: plugin_dir(spec, env, scope).join(spec.filename),
        }],
    };
    let skill_dir = skill_base(&h.skill, env, scope).join(SKILL_NAME);
    steps.push(PlanStep::Skill {
        path: skill_dir.join("SKILL.md"),
    });
    steps
}

/// Removal footprint for `h`: the adapter dir (JsonHook) or plugin file
/// (Plugin), the settings patch (JsonHook), and the skill directory.
pub fn plan_uninstall(h: &Harness, env: &AdapterEnv, scope: Scope) -> Vec<PlanStep> {
    let mut steps = match &h.kind {
        HarnessKind::JsonHook(spec) => vec![
            PlanStep::Hook {
                path: adapters_dir(env).join(h.name),
            },
            PlanStep::Patch {
                path: settings_path(spec, env, scope),
                key: spec.array_key,
            },
        ],
        HarnessKind::Plugin(spec) => vec![PlanStep::Plugin {
            path: plugin_dir(spec, env, scope).join(spec.filename),
        }],
    };
    steps.push(PlanStep::Skill {
        path: skill_base(&h.skill, env, scope).join(SKILL_NAME),
    });
    steps
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p hector-core plan_install plan_uninstall 2>&1 | tail -20`
Expected: PASS (5 tests). Then `cargo test -p hector-core 2>&1 | tail -5` — all core tests still green.

- [ ] **Step 7: Lint**

Run: `cargo clippy -p hector-core --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/hector-core/src/adapter/plan.rs crates/hector-core/src/adapter/ops.rs crates/hector-core/src/adapter/mod.rs
git commit -m "feat(core): add PlanStep model and plan_install/plan_uninstall planners"
```

---

## Task 2: CLI plan renderer (pure formatting)

Add the pretty-printer and the tiny ANSI helper. Pure functions, fully unit-tested, not wired into the flow yet.

**Files:**
- Create: `crates/hector-cli/src/commands/init/render.rs`
- Modify: `crates/hector-cli/src/commands/init/mod.rs:9` (add `mod render;`)

**Interfaces:**
- Consumes: `hector_core::adapter::{AdapterEnv, PlanStep}`.
- Produces:
  - `pub enum Source { Detected, Requested }` (derives `Clone, Copy`)
  - `pub struct HarnessPlan { pub name: &'static str, pub source: Source, pub steps: Vec<PlanStep> }`
  - `pub fn render_plan(plans: &[HarnessPlan], uninstall: bool, env: &AdapterEnv, color: bool) -> String`

- [ ] **Step 1: Declare the module**

In `crates/hector-cli/src/commands/init/mod.rs`, change `mod onboard;` to:

```rust
mod onboard;
mod render;
```

- [ ] **Step 2: Write failing renderer tests**

Create `crates/hector-cli/src/commands/init/render.rs` with only the test module first (so it fails to compile against missing items), OR write the full file (Step 4) and run tests after. To follow TDD, write this test module now and stub the public items to `unimplemented!()`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use hector_core::adapter::{AdapterEnv, PlanStep};
    use std::path::PathBuf;

    fn env() -> AdapterEnv {
        AdapterEnv {
            home: PathBuf::from("/home/u"),
            config_home: PathBuf::from("/home/u/.config"),
            project_root: PathBuf::from("/home/u/proj"),
        }
    }

    fn claude_plan() -> HarnessPlan {
        HarnessPlan {
            name: "claude-code",
            source: Source::Detected,
            steps: vec![
                PlanStep::Hook { path: PathBuf::from("/home/u/.config/hector/adapters/claude-code/hook.sh") },
                PlanStep::Patch { path: PathBuf::from("/home/u/proj/.claude/settings.json"), key: "PostToolUse" },
                PlanStep::Skill { path: PathBuf::from("/home/u/proj/.claude/skills/hector-config/SKILL.md") },
            ],
        }
    }

    #[test]
    fn renders_header_tag_and_tree_without_color() {
        let out = render_plan(&[claude_plan()], false, &env(), false);
        assert!(out.contains("hector · onboarding"), "header:\n{out}");
        assert!(out.contains("claude-code"), "harness name:\n{out}");
        assert!(out.contains("detected"), "source tag:\n{out}");
        assert!(out.contains("hook"), "hook label:\n{out}");
        assert!(out.contains("patch"), "patch label:\n{out}");
        assert!(out.contains("PostToolUse"), "patch key:\n{out}");
        assert!(out.contains("skill"), "skill label:\n{out}");
        // path shortening
        assert!(out.contains("~/.config/hector/adapters/claude-code/hook.sh"), "home-relative:\n{out}");
        assert!(out.contains("./.claude/settings.json"), "project-relative:\n{out}");
        // tree glyphs
        assert!(out.contains('├') && out.contains('└'), "tree glyphs:\n{out}");
    }

    #[test]
    fn no_color_output_has_no_escape_bytes() {
        let out = render_plan(&[claude_plan()], false, &env(), false);
        assert!(!out.contains('\u{1b}'), "must be plain when color off:\n{out:?}");
    }

    #[test]
    fn color_output_has_escape_bytes() {
        let out = render_plan(&[claude_plan()], false, &env(), true);
        assert!(out.contains('\u{1b}'), "must emit ANSI when color on");
    }

    #[test]
    fn requested_tag_and_uninstall_header() {
        let plan = HarnessPlan {
            name: "pi",
            source: Source::Requested,
            steps: vec![PlanStep::Plugin { path: PathBuf::from("/home/u/proj/.pi/extensions/hector.ts") }],
        };
        let out = render_plan(&[plan], true, &env(), false);
        assert!(out.contains("hector · uninstall"), "uninstall header:\n{out}");
        assert!(out.contains("requested"), "requested tag:\n{out}");
        assert!(out.contains("plugin"), "plugin label:\n{out}");
        assert!(out.contains("./.pi/extensions/hector.ts"), "project path:\n{out}");
    }

    #[test]
    fn absolute_path_fallback_when_outside_home_and_project() {
        let plan = HarnessPlan {
            name: "opencode",
            source: Source::Requested,
            steps: vec![PlanStep::Plugin { path: PathBuf::from("/opt/elsewhere/hector.ts") }],
        };
        let out = render_plan(&[plan], false, &env(), false);
        assert!(out.contains("/opt/elsewhere/hector.ts"), "absolute fallback:\n{out}");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p hector-cli render_ 2>&1 | tail -20`
Expected: FAIL — items unimplemented / not found.

- [ ] **Step 4: Implement the renderer**

Write the implementation above the test module in `crates/hector-cli/src/commands/init/render.rs`:

```rust
//! Pretty-printer for the `hector init` onboarding plan. Pure: takes structured
//! `PlanStep`s and returns the tree string. Color is TTY-gated by the caller and
//! passed in as `color`; when false, output contains no ANSI escapes.
use hector_core::adapter::{AdapterEnv, PlanStep};
use std::path::Path;

#[derive(Clone, Copy)]
pub enum Source {
    Detected,
    Requested,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Detected => "detected",
            Source::Requested => "requested",
        }
    }
}

pub struct HarnessPlan {
    pub name: &'static str,
    pub source: Source,
    pub steps: Vec<PlanStep>,
}

/// Minimal ANSI wrapper. `on == false` returns the input unchanged, so non-TTY
/// output is plain text.
struct Paint {
    on: bool,
}
impl Paint {
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\u{1b}[{code}m{s}\u{1b}[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
}

pub fn render_plan(plans: &[HarnessPlan], uninstall: bool, env: &AdapterEnv, color: bool) -> String {
    let p = Paint { on: color };
    let mut out = String::new();
    let title = if uninstall {
        "hector · uninstall"
    } else {
        "hector · onboarding"
    };
    out.push_str(&format!("\n  {}\n", p.bold(title)));
    out.push_str(&format!("  {}\n\n", p.dim(&"─".repeat(title.chars().count()))));
    for hp in plans {
        render_harness(&mut out, hp, env, &p);
    }
    out
}

fn render_harness(out: &mut String, hp: &HarnessPlan, env: &AdapterEnv, p: &Paint) {
    out.push_str(&format!(
        "  {}  {}\n",
        p.bold(&format!("{:<12}", hp.name)),
        p.green(hp.source.label())
    ));
    let last = hp.steps.len().saturating_sub(1);
    for (i, step) in hp.steps.iter().enumerate() {
        let branch = if i == last { "└" } else { "├" };
        let (kind, path) = step_parts(step);
        out.push_str(&format!(
            "    {} {} {}{}\n",
            branch,
            p.dim(&format!("{kind:<7}")),
            p.dim(&short_path(path, env)),
            patch_suffix(step, p),
        ));
    }
    out.push('\n');
}

fn step_parts(step: &PlanStep) -> (&'static str, &Path) {
    match step {
        PlanStep::Hook { path } => ("hook", path),
        PlanStep::Plugin { path } => ("plugin", path),
        PlanStep::Patch { path, .. } => ("patch", path),
        PlanStep::Skill { path } => ("skill", path),
    }
}

fn patch_suffix(step: &PlanStep, p: &Paint) -> String {
    match step {
        PlanStep::Patch { key, .. } => format!("  {} {}", p.dim("›"), p.cyan(key)),
        _ => String::new(),
    }
}

/// Project-relative (`./…`) first (more specific — a project under `$HOME`
/// should read `./`), then home-relative (`~/…`), else absolute.
fn short_path(path: &Path, env: &AdapterEnv) -> String {
    if let Ok(rel) = path.strip_prefix(&env.project_root) {
        return format!("./{}", rel.display());
    }
    if let Ok(rel) = path.strip_prefix(&env.home) {
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p hector-cli render_ 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 6: Lint**

Run: `cargo clippy -p hector-cli --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings. (If `render.rs` items are flagged dead-code because nothing calls `render_plan` yet, that resolves in Task 3 which wires it in; if clippy denies dead-code here, add `#[allow(dead_code)]` on the module items temporarily and remove it in Task 3 Step 6. Prefer completing Task 3 before the final clippy gate.)

- [ ] **Step 7: Commit**

```bash
git add crates/hector-cli/src/commands/init/render.rs crates/hector-cli/src/commands/init/mod.rs
git commit -m "feat(cli): add pure onboarding plan renderer with TTY-gated color"
```

---

## Task 3: Rewire onboarding flow; remove `DryRun`/`dry_run`

Switch `onboard.rs` to the plan-based pipeline, delete the `dry_run` parameter and `InstallResult::DryRun` variant, and fix every call site. On completion the new UX works end-to-end and the old string dry-run path is gone.

**Files:**
- Modify: `crates/hector-cli/src/commands/init/onboard.rs` (rewrite flow; drop `dry_run` args; remove `DryRun` match arms + their unit tests)
- Modify: `crates/hector-core/src/adapter/ops.rs` (remove `dry_run` param from `install`, `install_jsonhook`, `install_plugin`, `install_skill`, `install_skill_file`, `uninstall`, `uninstall_jsonhook`, `uninstall_plugin`, `uninstall_skill`; delete the `if dry_run { … }` blocks; remove `InstallResult::DryRun`; delete the 4 dry-run ops tests)
- Modify: `crates/hector-cli/src/commands/doctor.rs:452,471` (drop the trailing `, false` arg)
- Modify: `crates/hector-cli/tests/cli_init.rs:160-193` (update `init_dry_run_plans_skill_installs_for_explicit_harnesses` assertions to the new output)

**Interfaces:**
- Consumes: `hector_core::adapter::{all_harnesses, detect, install, install_skill, uninstall, uninstall_skill, plan_install, plan_uninstall, AdapterEnv, Harness, InstallResult, PlanStep, Scope}`; `super::render::{render_plan, HarnessPlan, Source}`.
- Produces: new signatures (all lose `dry_run`):
  - `pub fn install(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome>`
  - `pub fn uninstall(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome>`
  - `pub fn install_skill(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome>`
  - `pub fn uninstall_skill(h: &Harness, env: &AdapterEnv, scope: Scope) -> Result<InstallOutcome>`

- [ ] **Step 1: Remove `dry_run` + `DryRun` from `ops.rs`**

In `crates/hector-core/src/adapter/ops.rs`:

1. Delete the `DryRun(Vec<String>)` line from `enum InstallResult`.
2. In `install`, `install_jsonhook`, `install_plugin`, `install_skill`, `install_skill_file`, `uninstall`, `uninstall_jsonhook`, `uninstall_plugin`, `uninstall_skill`: remove the `dry_run: bool` parameter and delete the leading `if dry_run { return Ok(InstallResult::DryRun(...)); }` (or `let result = if dry_run {…} else {…}`) block, keeping only the real-work branch. Update the internal calls (`install_jsonhook(h.name, spec, env, scope)`, etc.) to drop the arg.
3. For `uninstall_jsonhook`/`uninstall_plugin`/`uninstall_skill`, which returned `InstallResult::Installed` after real work, keep that return; just remove the dry-run arm and param.
4. Delete these 4 now-obsolete tests from the `tests` module: `dry_run_writes_nothing`, `uninstall_dry_run_removes_nothing`, `install_skill_dry_run_writes_nothing`, `uninstall_skill_dry_run_removes_nothing`. (Their "writes/removes nothing" intent is now covered by `plan_install_writes_nothing` from Task 1 and by the CLI integration tests.)
5. Update the surviving ops tests that call the changed fns to drop the `false`/`true` arg (e.g. `install(&harness("reasonix"), &e, Scope::Global, false)` → `install(&harness("reasonix"), &e, Scope::Global)`).

- [ ] **Step 2: Fix `doctor.rs` call sites**

In `crates/hector-cli/src/commands/doctor.rs` lines ~452 and ~471, change:
```rust
hector_core::adapter::install(&h, &env, hector_core::adapter::Scope::Global, false)
```
to:
```rust
hector_core::adapter::install(&h, &env, hector_core::adapter::Scope::Global)
```

- [ ] **Step 3: Rewrite `onboard.rs`**

Replace the top-of-file `use` and the functions `run_hook_phase`, `choose_harnesses`, `format_outcome`, `run_skill_step`, and their `DryRun` arms with the following. Keep `select_harness_names`, `parse_confirm`, `should_install_skill`, `print_outcome`, `format_skill_outcome`, `print_skill_outcome` (minus their `DryRun` arms).

New imports (top of file):
```rust
use super::render::{render_plan, HarnessPlan, Source};
use super::Options;
use anyhow::{anyhow, Result};
use hector_core::adapter::{
    all_harnesses, detect, install, install_skill, plan_install, plan_uninstall, uninstall,
    uninstall_skill, AdapterEnv, Harness, InstallResult, PlanStep, Scope,
};
use std::io::{IsTerminal, Write};
```

New flow:
```rust
pub fn run_hook_phase(env: &AdapterEnv, opts: &Options) -> Result<i32> {
    let scope = if opts.global {
        Scope::Global
    } else {
        Scope::Local
    };
    let selected = resolve_harnesses(env, opts)?;
    if selected.is_empty() {
        println!("no supported harnesses detected; run `hector init --harness all` to wire all four");
        return Ok(0);
    }
    let plans = build_plans(&selected, env, scope, opts.uninstall);
    print!(
        "{}",
        render_plan(&plans, opts.uninstall, env, std::io::stdout().is_terminal())
    );
    if opts.dry_run {
        return Ok(0);
    }
    if !confirm_gate(opts, &selected)? {
        return Ok(0);
    }
    Ok(apply(&selected, env, scope, opts))
}

/// Resolve the harness set and tag each with why it is present. Explicit
/// `--harness` → `requested`; auto-detect → `detected`. No prompting here.
fn resolve_harnesses(env: &AdapterEnv, opts: &Options) -> Result<Vec<(String, Source)>> {
    if !opts.harnesses.is_empty() {
        let names = select_harness_names(&opts.harnesses)?;
        return Ok(names.into_iter().map(|n| (n, Source::Requested)).collect());
    }
    Ok(detect(env)
        .into_iter()
        .filter(|(_, found)| *found)
        .map(|(n, _)| (n.to_string(), Source::Detected))
        .collect())
}

/// Build the render-ready plan, honoring the opencode-skill dedup for install
/// (opencode reads claude-code's `.claude/skills/` copy).
fn build_plans(
    selected: &[(String, Source)],
    env: &AdapterEnv,
    scope: Scope,
    uninstall_mode: bool,
) -> Vec<HarnessPlan> {
    let registry = all_harnesses();
    let names: Vec<String> = selected.iter().map(|(n, _)| n.clone()).collect();
    selected
        .iter()
        .filter_map(|(name, source)| {
            let h = registry.iter().find(|h| h.name == *name)?;
            let mut steps = if uninstall_mode {
                plan_uninstall(h, env, scope)
            } else {
                plan_install(h, env, scope)
            };
            if !uninstall_mode && !should_install_skill(h.name, &names) {
                steps.retain(|s| !matches!(s, PlanStep::Skill { .. }));
            }
            Some(HarnessPlan {
                name: h.name,
                source: *source,
                steps,
            })
        })
        .collect()
}

/// Decide whether to proceed past the plan. `--yes` and explicit non-TTY
/// proceed; auto-detect non-TTY prints a hint and stops; TTY prompts.
fn confirm_gate(opts: &Options, selected: &[(String, Source)]) -> Result<bool> {
    if opts.yes {
        return Ok(true);
    }
    let explicit = !opts.harnesses.is_empty();
    if !std::io::stdin().is_terminal() {
        if explicit {
            return Ok(true);
        }
        let names = selected
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!("detected: {names} — re-run with `--yes` or `--harness <name>` to proceed");
        return Ok(false);
    }
    print!("  Proceed? [Y/n] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(parse_confirm(&line))
}

/// Install or uninstall the resolved set, printing per-harness result lines.
/// Returns the phase exit code: 3 only if every harness failed.
fn apply(selected: &[(String, Source)], env: &AdapterEnv, scope: Scope, opts: &Options) -> i32 {
    let registry = all_harnesses();
    let names: Vec<String> = selected.iter().map(|(n, _)| n.clone()).collect();
    let mut any_ok = false;
    let mut any_fail = false;
    for name in &names {
        let Some(h) = registry.iter().find(|h| h.name == *name) else {
            continue;
        };
        let outcome = if opts.uninstall {
            uninstall(h, env, scope)
        } else {
            install(h, env, scope)
        };
        match outcome {
            Ok(o) => {
                any_ok = true;
                print_outcome(o.harness, &o.result, o.hint, opts.uninstall);
            }
            Err(e) => {
                any_fail = true;
                println!("  {:<12} failed: {e:#}", h.name);
            }
        }
        let (skill_ok, skill_fail) = run_skill_step(h, env, scope, opts, &names);
        any_ok |= skill_ok;
        any_fail |= skill_fail;
    }
    if any_fail && !any_ok {
        3
    } else {
        0
    }
}
```

Update `format_outcome` — delete the `InstallResult::DryRun(plan) => {…}` arm (the match then has no `DryRun` case; since the variant is gone, exhaustiveness is fine). Same for `format_skill_outcome`. Update `run_skill_step` to drop `dry_run`:
```rust
    let s = if opts.uninstall {
        uninstall_skill(h, env, scope)
    } else {
        install_skill(h, env, scope)
    };
```

- [ ] **Step 4: Remove obsolete `onboard.rs` unit tests**

In the `onboard.rs` `tests` module, delete the two `DryRun`-specific assertions inside `format_outcome_covers_every_variant` and `format_skill_outcome_covers_variants` (the `let dr = format_outcome(... &DryRun(...) ...)` blocks and their asserts). Keep the rest of both tests. The `parse_confirm`, `select_explicit_*`, and `dedup_*` tests are unchanged.

- [ ] **Step 5: Update the CLI dry-run integration test**

In `crates/hector-cli/tests/cli_init.rs`, replace the body assertions of `init_dry_run_plans_skill_installs_for_explicit_harnesses` (lines ~180-192) with assertions against the new plan output:

```rust
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("hector · onboarding"), "plan header:\n{s}");
    assert!(s.contains("pi"), "must mention the pi harness:\n{s}");
    assert!(s.contains("requested"), "explicit harness tagged requested:\n{s}");
    assert!(s.contains("skill"), "plan must include the skill step:\n{s}");
    assert!(
        s.contains("skills/hector-config/SKILL.md"),
        "plan must name the skill path:\n{s}"
    );
```

(The `init_dedups_opencode_skill_when_claude_also_selected` test at lines ~195-228 asserts on paths that survive shortening — `.claude/skills/hector-config/SKILL.md` present, `.opencode/skills/hector-config` absent — and needs no change; verify it in Step 6.)

- [ ] **Step 6: Build, test, lint**

Run:
```bash
cargo test -p hector-core 2>&1 | tail -5
cargo test -p hector-cli 2>&1 | tail -15
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```
Expected: all green; no clippy warnings (the dead-code `#[allow]` from Task 2 Step 6, if added, is now removable — remove it and re-run).

- [ ] **Step 7: Commit**

```bash
git add crates/hector-core/src/adapter/ops.rs crates/hector-cli/src/commands/init/onboard.rs crates/hector-cli/src/commands/doctor.rs crates/hector-cli/tests/cli_init.rs
git commit -m "feat(cli): plan-and-confirm onboarding for both --harness and auto-detect"
```

---

## Task 4: End-to-end coverage + manual smoke + cleanup

Add integration tests for the new UX paths and verify by eye, then clean up any build artifacts this task produced.

**Files:**
- Modify: `crates/hector-cli/tests/cli_init_onboarding.rs` (add tests)

**Interfaces:**
- Consumes: the `hector(&home, &project)` test helper already defined at the top of `cli_init_onboarding.rs` (sets `HOME`/`--dir`, isolates `XDG_CONFIG_HOME`). Confirm its signature by reading the file top before writing (it is used at lines 44-52, 92-95).

- [ ] **Step 1: Add integration tests for the plan + tag + confirm**

Append to `crates/hector-cli/tests/cli_init_onboarding.rs`:

```rust
#[test]
fn explicit_harness_renders_plan_with_requested_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = hector(&home, &project)
        .args(["init", "--hook-only", "--harness", "reasonix", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("hector · onboarding"), "header:\n{s}");
    assert!(s.contains("reasonix"), "harness:\n{s}");
    assert!(s.contains("requested"), "explicit → requested tag:\n{s}");
    assert!(s.contains("hook"), "hook step listed:\n{s}");
    // --yes still installs
    assert!(home.join(".config/hector/adapters/reasonix/hook.sh").exists());
}

#[test]
fn dry_run_renders_plan_but_installs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = hector(&home, &project)
        .args(["init", "--hook-only", "--harness", "reasonix", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("hector · onboarding"), "dry-run still renders plan:\n{s}");
    assert!(
        !home.join(".config/hector/adapters/reasonix/hook.sh").exists(),
        "dry-run writes nothing"
    );
}

#[test]
fn uninstall_renders_removal_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    hector(&home, &project)
        .args(["init", "--hook-only", "--harness", "reasonix", "--yes"])
        .assert()
        .success();
    let out = hector(&home, &project)
        .args(["init", "--uninstall", "--harness", "reasonix", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("hector · uninstall"), "uninstall header:\n{s}");
    assert!(
        !home.join(".config/hector/adapters/reasonix/hook.sh").exists(),
        "uninstall removes the hook"
    );
}
```

Note: the existing `dry_run_writes_nothing` test (lines ~61-83) still passes as-is (it asserts absence of files). Keep it; the new `dry_run_renders_plan_but_installs_nothing` adds the plan-output assertion.

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p hector-cli --test cli_init_onboarding 2>&1 | tail -20`
Expected: PASS (existing + 3 new tests).

- [ ] **Step 3: Full workspace test + lint**

Run:
```bash
cargo test 2>&1 | tail -20
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --check
```
Expected: all green.

- [ ] **Step 4: Manual smoke (see the pretty output in a real terminal)**

Run against a throwaway dir so color renders on a TTY:
```bash
cargo build --release 2>&1 | tail -3
TMP=$(mktemp -d)
./target/release/hector init --dir "$TMP" --harness claude-code --harness pi --dry-run
```
Expected: the boxed header `hector · onboarding`, `claude-code  requested` / `pi  requested`, a `├/└` tree of `hook`/`patch`/`skill` / `plugin`/`skill` lines with `~/…` and `./…` paths, and NO `Proceed?` prompt (dry-run stops before it). Confirm colors appear (bold names, dim paths).

- [ ] **Step 5: Clean up build artifacts**

Per repo rule, drop anything this task built for verification:
```bash
rm -rf "$TMP"
cargo clean -p hector-cli
```
(Only removes the release artifact built for the smoke check; the iterating `target/` debug tree stays.)

- [ ] **Step 6: Commit**

```bash
git add crates/hector-cli/tests/cli_init_onboarding.rs
git commit -m "test(cli): cover plan render, requested tag, dry-run, and uninstall paths"
```

---

## Self-Review Notes

- **Spec coverage:** §1 plan model → Task 1. §2 unified flow + edge-case table (`--yes`, `--dry-run`, non-TTY explicit/auto, nothing-detected, `--uninstall`, `--no-hook`) → Task 3 (`run_hook_phase`, `confirm_gate`; `--no-hook` is handled upstream in `init/mod.rs::run` and untouched). §3 rendering (header+rule, tree, aligned kind column, patch key suffix, home/project/absolute path shortening, TTY-gated color, no-escape-when-plain) → Task 2. Testing section → Tasks 1/2/4. `DryRun` removal → Task 3.
- **Placeholder scan:** none — every code step shows complete code; every run step shows the command + expected result.
- **Type consistency:** `PlanStep` variants (`Hook`/`Plugin`/`Patch{path,key}`/`Skill`) are used identically in Tasks 1–3; `Source` (`Detected`/`Requested`), `HarnessPlan { name, source, steps }`, and `render_plan(&[HarnessPlan], bool, &AdapterEnv, bool)` match across Tasks 2 and 3; the de-`dry_run`'d signatures in Task 3's Interfaces match the doctor.rs and onboard.rs call-site edits.
- **Coverage/complexity:** new functions are small and single-purpose; renderer match arms + color on/off + path-shortening branches are each hit by Task 2 tests; flow branches by Task 4 integration tests.
