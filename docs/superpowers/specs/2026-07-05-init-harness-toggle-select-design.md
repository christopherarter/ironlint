# `ironlint init` — interactive harness multi-select

**Date:** 2026-07-05
**Status:** Design (pre-implementation)
**Scope:** `crates/ironlint-cli` — `commands/init/`

## Problem

`ironlint init` with no `--harness` flag is all-or-nothing: it auto-detects
installed coding agents, renders a plan for the detected set, and offers a
single `Proceed? [Y/n]` confirm. The user cannot opt out of a detected
harness, nor opt into a harness that wasn't auto-detected, without abandoning
the bare flow and re-running with explicit `--harness <name>` flags. When zero
harnesses are detected, the bare flow is a dead-end: it prints a hint and
exits, forcing `--harness all` or `--harness <name>`.

## Goal

Replace the Y/n confirm on the bare auto-detect path with an interactive
multi-select. Detected harnesses are checked by default; the user can check or
uncheck any of the four supported harnesses; if none are detected, all four are
shown unchecked and the user can opt in. Explicit `--harness`, `--yes`, and the
non-TTY/scripted paths are unchanged.

## Non-goals

- No changes to `--harness` flag semantics, exit codes, the opencode-skill
  dedup, the `Source` enum's existing variants, or `render.rs`.
- No new runtime dependencies.
- No general-purpose reusable picker widget — the selector is init-specific.
- No PTY-based testing of the live event loop; the render surface is tested via
  ratatui's `TestBackend`, matching the existing `watch.rs` convention.

## Background

Four supported harnesses, in registry order: `claude-code`, `codex`, `pi`,
`opencode` (`ironlint_core::adapter::all_harnesses`).

Current bare flow (`commands/init/onboard.rs`):

1. `resolve_harnesses` runs `detect(env)` and keeps entries where `found` is true.
2. If the detected set is empty → prints
   `no supported harnesses detected; run `ironlint init --harness all` to wire all four`
   and returns `Ok(0)`.
3. Otherwise `build_plans` → `render_plan` → `confirm_gate`:
   - `--yes` → proceed.
   - non-TTY + explicit `--harness` → proceed.
   - non-TTY + auto-detect → prints
     `detected: <names> — re-run with `--yes` or `--harness <name>` to proceed`
     and stops.
   - TTY → prints `  Proceed? [Y/n] ` and reads one line (defaults to yes on empty).
4. `apply` installs/uninstalls the resolved set.

`confirm_gate` currently returns `bool` and takes `selected: &[(String, Source)]`
by immutable reference. The toggle breaks both: the user can change the set at
confirm time, so the gate must be able to rewrite the selection.

`ratatui = "0.30"` is already a workspace dependency (`watch` command); it
re-exports `crossterm`. `watch.rs` already uses `ratatui::backend::TestBackend`
to render-test its `ui()` function (lines ~1085–1196).

## Design

### Module layout

```
crates/ironlint-cli/src/commands/init/
  mod.rs        ← Options, run(), scaffold_config  (unchanged)
  onboard.rs    ← run_hook_phase, resolve_harnesses, confirm_gate  (modified)
  render.rs     ← render_plan, HarnessPlan, Source  (unchanged)
  select.rs     ← NEW: SelectItem, prompt_multi_select, render_select
```

`select.rs` owns the interactive multi-select. It is a self-contained unit
with one job: render the harness list, read keys, return the chosen names. It
mirrors the existing `render.rs` split (pure render vs. orchestration).

### `select.rs` public surface

```rust
pub struct SelectItem {
    pub name: String,
    pub detected: bool,   // drives the "detected" tag + default-checked state
    pub selected: bool,   // mutable; toggled by space
}

/// Interactive multi-select over the harness list.
///
/// Caller guarantees stdin is a TTY and `items` is non-empty.
/// Returns the names of items whose `selected` is true on confirm.
/// Abort (Esc/q/Ctrl-C) or an empty result returns an empty `Vec`.
pub fn prompt_multi_select(items: Vec<SelectItem>) -> Result<Vec<String>>;

/// Pure render of the picker into a ratatui frame. Extracted so tests can
/// drive it against `TestBackend` without a TTY or event loop.
fn render_select(f: &mut Frame, items: &[SelectItem], state: &ListState);
```

### The picker UI

Alternate-screen, raw mode, single centered `List` widget:

```
  ironlint · onboarding

  Select harnesses to wire (space toggles, enter confirms):

  > [x] claude-code    detected
    [x] codex          detected
    [ ] pi
    [ ] opencode

  ─ 2 of 4 selected ─
```

- `>` marks the cursor (ratatui `ListState::selected`).
- `[x]` / `[ ]` is the checkbox, derived from `SelectItem::selected`.
- `detected` is a dim tag shown only when `SelectItem::detected` is true.
- Footer shows the live count: `N of 4 selected`.
- Header is bold.

### Key handling (crossterm `KeyEvent`)

| Key | Action |
|---|---|
| `Up` / `k` | move cursor up |
| `Down` / `j` | move cursor down |
| `Space` | toggle current item's `selected` |
| `Enter` | confirm, return current selection |
| `Esc` / `q` / `Ctrl-C` | abort → returns `Ok(vec![])` |

Vim `j`/`k` come free and match `watch`'s keybindings.

### Terminal restoration

A `TerminalGuard` holds the `Terminal` and restores the prior state in `Drop`:
disable raw mode, leave alternate screen, show cursor. Runs on `?` early
returns, panics (via `Drop` during unwind), and normal completion. `SIGKILL` is
out of scope (the terminal emulator resets the alternate screen on process
death; raw mode is process-scoped).

### Data flow

New flow: **resolve → build initial plans → render plan → confirm_gate (may
rewrite the set) → rebuild plans if the set changed → apply**.

`confirm_gate` signature change:

```rust
enum Proceed { Yes, No }

// Before:
fn confirm_gate(opts: &Options, selected: &[(String, Source)]) -> Result<bool>;

// After:
fn confirm_gate(opts: &Options, selected: &mut Vec<(String, Source)>) -> Result<Proceed>;
```

`run_hook_phase` calls the gate with `&mut`, then rebuilds plans from the
(possibly mutated) `selected` before `apply`. Rebuild is cheap (four harnesses).

#### `build_items` — bridge from `selected` + registry to `SelectItem`s

```rust
fn build_items(selected: &[(String, Source)]) -> Vec<SelectItem> {
    all_harnesses().iter().map(|h| {
        let is_selected = selected.iter().any(|(n, _)| n == h.name);
        SelectItem {
            name: h.name.to_string(),
            detected: is_selected,  // "detected" == was in the auto-detected set
            selected: is_selected,  // default-checked == detected
        }
    }).collect()
}
```

`detected` drives both the tag and the default checkbox. The user can toggle
any of the four.

#### Reconciling the result back into `selected`

After the toggle returns its chosen names, `confirm_gate` rebuilds `selected`,
preserving the `Source` tag:

```rust
let chosen = select::prompt_multi_select(build_items(selected))?;
let originally_detected: Vec<String> = selected.iter()
    .filter(|(_, s)| *s == Source::Detected)
    .map(|(n, _)| n.clone()).collect();
*selected = chosen.into_iter().map(|n| {
    let src = if originally_detected.contains(&n) { Source::Detected }
              else { Source::Requested };
    (n, src)
}).collect();
```

Detected-and-still-checked → `Source::Detected`. User-checked-undetected →
`Source::Requested`. Unchecked items are dropped. `render_plan`'s tags flow
through unchanged.

#### Empty result handling

If the user unchecks everything and hits Enter, `prompt_multi_select` returns
an empty `Vec`. `confirm_gate` prints
`no harnesses selected; nothing to do` and returns `Proceed::No` (exit 0, no
install). This is the interactive analog of the "none detected" rule: the user
can also choose none. Abort (`Esc`/`q`/`Ctrl-C`) likewise returns `Ok(vec![])`
→ `Proceed::No` → exit 0, nothing installed. Abort is the user explicitly
bailing; it is not an `Err`.

### `confirm_gate` dispatch

```rust
fn confirm_gate(opts: &Options, selected: &mut Vec<(String, Source)>) -> Result<Proceed> {
    if opts.yes { return Ok(Proceed::Yes); }
    if !std::io::stdin().is_terminal() {
        // non-TTY: existing fallback. Explicit --harness proceeds; auto-detect
        // prints the re-run hint and stops.
        let explicit = !opts.harnesses.is_empty();
        if explicit { return Ok(Proceed::Yes); }
        let names = selected.iter().map(|(n, _)| n.as_str())
            .collect::<Vec<_>>().join(", ");
        println!("detected: {names} — re-run with `--yes` or `--harness <name>` to proceed");
        return Ok(Proceed::No);
    }
    // TTY + no explicit --harness → interactive toggle (the new path).
    if opts.harnesses.is_empty() {
        let chosen = select::prompt_multi_select(build_items(selected))?;
        if chosen.is_empty() {
            println!("no harnesses selected; nothing to do");
            return Ok(Proceed::No);
        }
        *selected = reconcile(chosen, selected);
        return Ok(Proceed::Yes);
    }
    // TTY + explicit --harness → existing Y/n prompt.
    print!("  Proceed? [Y/n] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(if parse_confirm(&line) { Proceed::Yes } else { Proceed::No })
}
```

### `run_hook_phase` change

```rust
pub fn run_hook_phase(env: &AdapterEnv, opts: &Options) -> Result<i32> {
    let scope = if opts.global { Scope::Global } else { Scope::Local };
    let mut selected = resolve_harnesses(env, opts)?;

    // Bare + none detected is no longer a dead-end when interactive: render
    // the empty-selection plan only on non-TTY. On TTY, fall through to the
    // toggle, which shows all four unchecked. To keep the non-TTY hint honest,
    // keep the existing empty-set early return only when not interactive.
    if selected.is_empty() && !std::io::stdin().is_terminal() {
        println!(
            "no supported harnesses detected; run `ironlint init --harness all` to wire all four"
        );
        return Ok(0);
    }

    let plans = build_plans(&selected, env, scope, opts.uninstall);
    print!("{}", render_plan(&plans, opts.uninstall, env, std::io::stdout().is_terminal()));
    if opts.dry_run { return Ok(0); }

    match confirm_gate(opts, &mut selected)? {
        Proceed::No => return Ok(0),
        Proceed::Yes => {}
    }
    // Rebuild from the possibly-toggle-mutated set.
    let plans = build_plans(&selected, env, scope, opts.uninstall);
    Ok(apply(&selected, env, scope, opts))
}
```

The empty-detected-set early return now keys off non-TTY, so the bare TTY path
with zero detections proceeds to the toggle (all four shown, none checked).
The non-TTY empty-set message is preserved verbatim.

### Behavior matrix

| Invocation | TTY | Behavior |
|---|---|---|
| `init` (no flags) | yes | multi-select, detected pre-checked, Enter installs chosen |
| `init` (no flags) | no | `detected: … — re-run with --yes or --harness <name>` (unchanged) |
| `init --yes` | any | install detected set, no toggle (unchanged) |
| `init --harness codex` | any | install codex, no toggle (unchanged) |
| `init --harness all` | any | install all four, no toggle (unchanged) |
| `init --uninstall` | yes | multi-select, detected pre-checked, Enter uninstalls chosen |
| `init --dry-run` | any | render plan, exit 0, no toggle (unchanged) |
| none detected, `init` | yes | multi-select, all four shown, none checked |
| none detected, `init` | no | `no supported harnesses detected; run --harness all` (unchanged) |
| user unchecks all, Enter | yes | `no harnesses selected; nothing to do`, exit 0 |

### Edge cases & error handling

- **Terminal setup failure** (`enable_raw_mode` / `EnterAlternateScreen`):
  `prompt_multi_select` returns `Err` → propagates → CLI exit 1. No silent
  fallback to Y/n; a broken terminal is a real error.
- **`read()` error mid-loop:** `Err` → propagates → `Drop` restores terminal →
  exit 1.
- **Empty `items` passed in:** return `Ok(vec![])` without entering the loop
  (defensive; never happens in practice since `build_items` enumerates all four).
- **`--dry-run`:** returns from `run_hook_phase` before `confirm_gate`, so the
  toggle never runs. Correct: dry-run is non-interactive.
- **`--uninstall`:** same `confirm_gate`; the toggle picks which harnesses to
  uninstall, detected pre-checked. `Source` flows to `render_plan`'s uninstall
  header. No special-casing.
- **`--global`:** changes `Scope` only; the toggle is scope-agnostic. No
  interaction.

## Testing

### `TestBackend` render tests for `render_select` (in `select.rs`)

Mirror `watch.rs` lines ~1085–1196: build `TestBackend::new(W, H)`,
`term.draw(|f| render_select(f, &items, &state))`, collect
`term.backend().buffer().content()` into a string, assert on it. Cases:

- All four detected + checked — footer reads `4 of 4 selected`, every row shows
  `[x]` and `detected`.
- None detected, none checked — footer `0 of 4 selected`, every row `[ ]`, no
  `detected` tag anywhere.
- Mixed — two detected/checked, two not — exact rows and footer `2 of 4`.
- Cursor positioning — the row under the `ListState` cursor carries the `>`
  marker; siblings do not.
- Toggle reflected in render — flip one item's `selected` between draws and
  assert its marker flips `[ ]` ↔ `[x]` and the footer count changes.

### Pure-logic tests (in `select.rs` / `onboard.rs`)

- `build_items`: detected set maps to `detected && selected == true`;
  undetected harnesses map to both false; all four are present in registry
  order.
- `reconcile`: a chosen name that was originally detected keeps
  `Source::Detected`; a chosen name that was not detected becomes
  `Source::Requested`; dropped names do not appear.
- Abort / empty-chosen → `Proceed::No`, exit 0, `apply` never called.

### Integration tests (`tests/cli_init_onboarding.rs`)

Existing `no_tty_without_yes_or_harness_skips_hooks` stays green unchanged
(assert_cmd pipes stdin → non-TTY → hint path). New tests:

- `--yes` installs the detected set without any toggle (non-TTY; pins the
  bypass).
- explicit `--harness codex --yes` skips the toggle and installs codex
  (already partially covered by `explicit_harness_renders_plan_with_requested_tag`;
  add an assertion that the toggle path was not entered, e.g. no
  `Select harnesses` text in stdout).
- `build_items` none-detected case yields four items all unchecked (pure unit
  test, no TTY).

The live event loop (raw mode, key reads, `Drop` guard) is intentionally
untested — it is thin glue over crossterm, and every behavior worth verifying
is exercised through `render_select` + the pure logic.

### Coverage gate

`select.rs` is under `crates/ironlint-cli/src/` → ≥80% region coverage per
`scripts/ci-coverage.sh`. The extracted `render_select` + pure logic
(`build_items`, `reconcile`) carry the coverage; the event-loop body is the
only uncovered region and is kept minimal.

## Dependencies

None added. `ratatui = "0.30"` (re-exports `crossterm`) already in
`crates/ironlint-cli/Cargo.toml` for `watch`. No MSRV impact, no supply-chain
review, no `Cargo.lock` churn beyond what ratatui already pins.

## Files touched

- **New:** `crates/ironlint-cli/src/commands/init/select.rs`
- **Modified:** `crates/ironlint-cli/src/commands/init/onboard.rs`
  (`confirm_gate` signature + dispatch, `run_hook_phase` rebuild + empty-set
  guard, new `build_items` + `reconcile` helpers)
- **Modified:** `crates/ironlint-cli/src/commands/init/mod.rs`
  (add `mod select;`)
- **Modified:** `crates/ironlint-cli/tests/cli_init_onboarding.rs`
  (new bypass/`build_items` tests)
