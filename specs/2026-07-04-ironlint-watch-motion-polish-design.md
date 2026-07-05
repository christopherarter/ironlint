# IronLint `watch` — motion & block-tint polish

**Status:** design, approved direction (2026-07-04)
**Builds on:** the watch TUI (`specs/2026-06-28-ironlint-watch-tui-design.md`, shipped in `crates/ironlint-cli/src/commands/watch.rs`)
**Scope:** the `ironlint watch` Stream view only. No change to Explorer, to the everyday `check`/`init`/`doctor` output, to core aggregation, or to any wire/exit contract.
**Non-breaking:** cosmetic + event-loop timing. No config, verdict-JSON, or telemetry change.

## 1. Thesis

The watch TUI is correct and legible but *static* — results appear as hard cuts and a wall clock decorates the corner. Two cheap changes make it read as a live instrument instead of a log dump: **the newest result animates in**, and **a block is visible at a glance by a standing red tint** rather than only by its glyph. Nothing about *what* is shown changes; only how the newest row arrives and how a block row is painted. The motion is disciplined — it exists to draw the eye to the thing that just happened, then get out of the way.

A browser mock (`scratchpad/watch-preview.html`, exact CLI colors, row-quantized) validated the feel before this spec: single-edit wipe-in ✓, burst cascade ✓, constant tint ✓.

## 2. Locked decisions

1. **Drop the clock.** The header corner shows only the verdict — `● PASS` (green) / `● BLOCK` (orange), static. Every stream row already carries its own timestamp; a wall clock competed with that signal. No pulsing/breathing dot (considered, rejected).
2. **Single edit → horizontal wipe-in.** The new top row reveals its cells left→right over ~210ms. A terminal can't slide text sub-row, so a lone new row would otherwise just pop; the wipe is what makes a single edit feel alive.
3. **Burst → row-quantized cascade.** When multiple entries are observed at once (an agent saving N files appends N log lines in one tick), they are released **one per ~95ms**, each doing its own wipe. The list steps down one whole row per release — honest to the cell grid, and the satisfying case.
4. **Block → constant slight red tint.** A blocked row *and its `└ <check> · <verb>` detail line* carry a dim red background (`#241015`), full pane width, standing (no flash, no settle). Passing rows have no background.
5. **Speed is fixed at the tuned "medium".** ~210ms wipe, ~95ms cascade step. Exposed only as internal constants, not config.

## 3. Before / after (Stream header + a block)

```
before   ≈ stream   ▤ explorer            BLOCK · 14:24:07
after    ≈ Stream   ▤ Explorer                     ● BLOCK
                                          (clock removed; dot carries verdict)

before     14:23:58  ✗  src/lib.rs          8ms   write
             └ ruff · write rejected            (orange glyph only)
after    ░░14:23:58  ✗  src/lib.rs          8ms   write░░   ← dim-red, full width
         ░░  └ ruff · write rejected             ░░░░░░░░░
```

## 4. Architecture

The existing split is preserved: **decision/format logic is pure and unit-tested; the terminal event loop is thin and minimally covered.** Animation is threaded through as *data*, never read from a clock inside a renderer.

### 4.1 Timing math → `ironlint-core::watch` (pure, tested)

Two small pure functions live beside the existing `summarize`/`fmt_elapsed` helpers:

```rust
/// Cells of a row to reveal for the wipe-in. `None` once fully revealed.
/// full_cells = the row's rendered width; enter_ms = wipe duration.
pub fn entrance_reveal(elapsed_ms: u64, enter_ms: u64, full_cells: u16) -> Option<u16>;

/// How many queued entries have been released by `elapsed_ms` at `step_ms` cadence.
pub fn cascade_released(elapsed_ms: u64, step_ms: u64, queued: usize) -> usize;
```

`entrance_reveal` is stepped (not linear-smooth) so it matches the terminal's discrete redraws. Boundaries — `elapsed 0 → Some(0)`, mid → `Some(k)` monotonic non-decreasing, `elapsed ≥ enter_ms → None` — are the test surface.

### 4.2 Render input gains an entrance annotation (pure, tested)

`stream_lines` stops taking bare entries and takes rows carrying their entrance state and the pane width (needed for full-width tint):

```rust
pub struct StreamRow<'a> {
    pub entry: &'a LogEntry,
    pub reveal: Option<u16>,   // Some(k)=show k leading cells; None=full row
}

pub fn stream_lines(rows: &[StreamRow], filter: Option<&str>, width: u16) -> Vec<Line<'static>>;
```

Per row, in order (all pure, each a testable helper):

1. Build the styled spans as today (time / glyph / file / elapsed / badge; block detail line unchanged in text).
2. **Tint:** if the entry blocked, set `.bg(RED_REST)` on every span and `pad_line(line, width)` — append a trailing bg-styled space run so the red reaches the pane edge. Same for the detail line.
3. **Wipe:** if `reveal == Some(k)`, `truncate_line(line, k)` — keep the first `k` display cells (tint travels with the text, so a block row wipes in red).

`pad_line` and `truncate_line` are cell-accurate helpers (respect multi-span, unicode width) with their own unit tests.

`RED_REST: Color = Color::Rgb(36, 16, 21)` joins the existing color vocabulary (`ORANGE`/`GREEN`/`AMBER`/`MUTED`).

### 4.3 Header loses the clock (pure, tested)

`header_line(state, summary)` drops its `clock: &str` param; the right side becomes a single `● PASS`/`● BLOCK` span (dot fg = verdict color, label muted). `ui(...)` drops its `clock` param accordingly. Existing `ui`/`header_line` tests update to the new signatures (churn is expected and covered by the coverage gate).

### 4.4 Event loop owns the timing (thin, minimally covered)

`event_loop` gains an **entrance model** it feeds to the pure renderers each frame:

- A **reveal queue**: entries observed since last poll are enqueued, not shown immediately.
- Release cadence: pop one per `STEP_MS` (uses `cascade_released`), assigning each a release `Instant`.
- Per released-and-still-animating entry, compute `reveal` via `entrance_reveal(elapsed, ENTER_MS, row_width)`; fully-revealed entries carry `reveal = None`.
- **Poll cadence:** if the queue is non-empty *or* any released entry is still animating → tick at `ACTIVE_POLL` (~40ms) so the wipe steps smoothly; otherwise fall back to `IDLE_POLL` (~250ms) / event-driven. A quiet watcher still burns ~zero CPU — motion never runs when nothing landed.

This is the only genuinely new machinery. It stays in the "uncovered terminal glue" bucket the file already documents; all its arithmetic is delegated to the two pure core functions so the logic *is* tested.

## 5. Testing

- **core:** `entrance_reveal` boundaries + monotonicity; `cascade_released` (0/partial/all-released, clamps at `queued`).
- **cli:** `truncate_line` (cell count, multi-span, wide-char safety); `pad_line` (fills to width, bg present); `stream_lines` — a mid-wipe row is truncated, a block row is tinted full-width on both its lines, a passing row is untinted; `header_line` has no clock and the verdict dot flips PASS↔BLOCK; a `ui` render test for one mid-entrance frame containing a block.
- Per-file **≥80% region coverage** holds (`scripts/ci-coverage.sh`). Cognitive complexity ≤15 — `stream_lines` gains steps; if it crosses the cap, split row-building into a `stream_row(entry, reveal, width)` helper rather than annotating.

## 6. Out of scope (explicit)

Explorer-view motion; a pass-rate gauge; any pulsing/breathing element; sub-cell "smooth" vertical scrolling (impossible in a cell grid — not a TODO); restyling `check`/`init`/`doctor`; making speed/tint configurable; `NO_COLOR` handling (pre-existing gap, unchanged here).
