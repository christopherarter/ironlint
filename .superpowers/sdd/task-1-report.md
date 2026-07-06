# Task 1 Report: Implement select.rs multi-select module

## What was implemented

Created `crates/ironlint-cli/src/commands/init/select.rs`, the interactive harness multi-select picker for `ironlint init`.

Public surface:

- `SelectItem { name, detected, selected }`
- `SelectOutcome { Pending, Confirmed, Aborted }`
- `prompt_multi_select(items: Vec<SelectItem>) -> Result<Vec<String>>`
- `update_select(items, key, state) -> (Vec<SelectItem>, ListState, Option<SelectOutcome>)`
- `render_select(f, items, state)`

Key implementation details:

- `prompt_multi_select` is the only I/O entry point. Returns `Ok(vec![])` for empty input, enables raw mode, enters the alternate screen, hides the cursor, wraps the `Terminal` in a `TerminalGuard` whose `Drop` restores terminal state, and loops on crossterm events.
- `update_select` is a pure reducer handling cursor movement (Up/k, Down/j with wrap), toggling (Space), confirm (Enter), and abort (Esc/q/Ctrl-C).
- `render_select` is a pure ratatui render using `TestBackend`-drivable widgets, centered vertically, with header, help line, list, and live footer count.
- No new runtime dependencies: re-uses existing `ratatui = "0.30"` (which re-exports `crossterm`).
- Added `mod select;` to `crates/ironlint-cli/src/commands/init/mod.rs` so the module compiles and tests run.

## TDD evidence

### Red: failing stub tests

Created the file with stub `update_select` / `render_select` implementations and all unit tests. Running:

```bash
cargo test -p ironlint-cli select
```

produced 8 failures for the new `select::tests` cases (cursor moves, toggles, confirm/abort, render assertions), confirming the tests were exercising the right surface.

### Green: implementation passes

After implementing `update_select`, `render_select`, and `move_cursor`:

```bash
cargo test -p ironlint-cli select
# cargo test: 14 passed, 270 filtered out
```

Full crate tests:

```bash
cargo test -p ironlint-cli
# cargo test: 284 passed
```

Lint:

```bash
cargo clippy --all-targets -- -D warnings
# cargo clippy: No issues found
```

Coverage:

```bash
bash scripts/ci-coverage.sh
# ci-coverage: all files ≥ 80% region coverage.
```

## Tests added

Eight new unit tests in `select.rs`:

1. `render_select_all_detected_checked`
2. `render_select_none_detected`
3. `render_select_mixed`
4. `render_select_cursor_position`
5. `render_select_toggle_reflected`
6. `update_select_moves_cursor`
7. `update_select_toggles`
8. `update_select_confirm_and_abort`

These mirror the `watch.rs` `TestBackend` convention and keep the live event loop untested, as specified.

## Files changed

- `crates/ironlint-cli/src/commands/init/select.rs` (new, 434 lines)
- `crates/ironlint-cli/src/commands/init/mod.rs` (added `mod select;`)

## Self-review findings

- The pure reducer keeps the event loop thin; all cursor/toggle logic is in `update_select`.
- `TerminalGuard` restores raw mode, alternate screen, and cursor visibility on all exit paths, including panic unwinding.
- `move_cursor` uses wrapping `usize` arithmetic, avoiding clippy's `cast_possible_wrap` warning.
- `ListState` is `Copy`, so cloning was replaced by dereferencing.
- Region coverage gate passes; the only uncovered region is the thin event-loop/TUI setup in `prompt_multi_select`.
- The `#![allow(dead_code)]` module-level attribute is temporary: Task 2 will wire `prompt_multi_select` into `onboard.rs`, making the public items live and removing the need for the suppression.

## Concerns

- `SelectOutcome::Pending` is defined because the brief lists it in the public surface, but it is never constructed: `update_select` returns `None` for pending state per the brief's reducer contract. This is a harmless inconsistency in the spec that will become relevant only if a future caller expects to consume `SelectOutcome::Pending`; for now it is dead code tolerated by the temporary `#![allow(dead_code)]` attribute.
- The module is not yet wired into the interactive flow. `onboard.rs` still uses the old `confirm_gate` path. That wiring is explicitly owned by Task 2.

## Post-review fix (2026-07-06)

### Issues addressed

1. **Important — Terminal state leak if `Terminal::new` fails before the guard exists**
   - Moved all terminal setup (`enable_raw_mode`, `EnterAlternateScreen`, `Hide`) into a new `TerminalGuard::new()` constructor.
   - If entering the alternate screen fails, the constructor disables raw mode before returning the error.
   - If `Terminal::new` fails after setup, the constructor leaves the alternate screen, shows the cursor, and disables raw mode before returning the error.
   - `prompt_multi_select` now simply calls `TerminalGuard::new()?`, so any failure path cleans up.

2. **Minor — Modified j/k/Space still trigger actions**
   - `update_select` now only handles `Char('j')`, `Char('k')`, and `Char(' ')` when `key.modifiers.is_empty()`.
   - Combinations like `Ctrl+j`, `Ctrl+k`, and `Ctrl+Space` no longer move or toggle.

3. **Minor — `TerminalGuard` was generic but assumed stdout**
   - Removed the generic `Backend` parameter; `TerminalGuard` is now specific to `Terminal<CrosstermBackend<Stdout>>`.
   - The `Drop` impl routes `LeaveAlternateScreen`/`Show` through `self.0.backend_mut()` instead of a fresh stdout handle.

### Verification

```bash
cargo test -p ironlint-cli select
# cargo test: 14 passed, 270 filtered out (33 suites, 0.76s)

cargo clippy --all-targets -- -D warnings
# cargo clippy: No issues found

bash scripts/ci-coverage.sh
# ci-coverage: all files ≥ 80% region coverage.
```
