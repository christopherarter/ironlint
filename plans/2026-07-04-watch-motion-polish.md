# Watch Motion & Block-Tint Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ironlint watch`'s Stream view feel live — newest result wipes in, bursts cascade one row per tick, blocked rows carry a standing dim-red tint — and drop the decorative wall clock for a static verdict dot.

**Architecture:** All timing arithmetic goes in `ironlint-core::watch` as pure, unit-tested functions. The CLI renderers (`stream_lines`, `header_line`, `ui`) stay pure — they receive per-row entrance state (`age_ms`) and pane width as *data* and never read a clock. The `event_loop` owns the reveal queue + Instant math and ticks at ~40ms only while something is animating, idling at ~250ms otherwise.

**Tech Stack:** Rust, ratatui 0.29 (crossterm backend), the existing `ironlint-core` / `ironlint-cli` workspace. No new dependencies.

## Global Constraints

- Per-file **≥80% region coverage** (`bash scripts/ci-coverage.sh`, cargo-llvm-cov). New code must not drop a file below the gate.
- Per-function **cognitive complexity ≤ 15** (clippy, `-D warnings`). Refactor over `#[allow]`.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must pass. No cast lints — use `u16::try_from(..)`/`usize::try_from(..)`, not `as`.
- **No new dependencies.** Stream content is single-column (ASCII paths + single-width glyphs `✓ ✗ ⚠ ≈ ▤ ● └`), so treat display column ≈ `char` count — do not pull in `unicode-width`.
- Renderers stay **pure** (no `Instant::now`/clock reads inside `stream_lines`/`header_line`/`ui`). Timing lives in the event loop only.
- Colors come from the existing vocabulary in `crates/ironlint-cli/src/commands/watch.rs` (`ORANGE` `#FF5C38`, `GREEN` `#34D399`, `AMBER`, `MUTED`); this plan adds exactly one: `RED_REST = Color::Rgb(36, 16, 21)`.
- Scope is the Stream view only. Do not touch Explorer, `check`/`init`/`doctor`, core aggregation, or any wire/exit contract.
- Spec: `specs/2026-07-04-ironlint-watch-motion-polish-design.md`.

---

### Task 1: Core timing math (`entrance_reveal`, `cascade_released`)

Two pure functions in the core watch module — the only place the animation "logic" lives, so it's fully tested even though the loop that calls it isn't.

**Files:**
- Modify: `crates/ironlint-core/src/watch.rs` (add fns after `status_glyph`, ~line 212; add tests in the existing `mod tests`)

**Interfaces:**
- Produces:
  - `pub fn entrance_reveal(elapsed_ms: u64, enter_ms: u64, full_cells: u16) -> Option<u16>`
  - `pub fn cascade_released(elapsed_ms: u64, step_ms: u64, queued: usize) -> usize`

- [ ] **Step 1: Write the failing tests**

Add to `crates/ironlint-core/src/watch.rs`, inside `mod tests`:

```rust
#[test]
fn entrance_reveal_starts_at_zero_grows_then_settles() {
    // Full row is 40 cells, wipe lasts 200ms.
    assert_eq!(entrance_reveal(0, 200, 40), Some(0));
    assert_eq!(entrance_reveal(100, 200, 40), Some(20)); // halfway
    assert_eq!(entrance_reveal(199, 200, 40), Some(39));
    assert_eq!(entrance_reveal(200, 200, 40), None); // settled at the boundary
    assert_eq!(entrance_reveal(999, 200, 40), None);
}

#[test]
fn entrance_reveal_is_monotonic_non_decreasing() {
    let mut prev = 0;
    for t in 0..200 {
        let cells = entrance_reveal(t, 200, 64).unwrap();
        assert!(cells >= prev, "reveal went backwards at t={t}");
        assert!(cells <= 64);
        prev = cells;
    }
}

#[test]
fn entrance_reveal_zero_duration_is_instant() {
    assert_eq!(entrance_reveal(0, 0, 40), None);
}

#[test]
fn cascade_releases_one_immediately_then_one_per_step() {
    assert_eq!(cascade_released(0, 95, 5), 1); // first row shows at once
    assert_eq!(cascade_released(94, 95, 5), 1);
    assert_eq!(cascade_released(95, 95, 5), 2);
    assert_eq!(cascade_released(950, 95, 5), 5); // clamps at queued
}

#[test]
fn cascade_released_clamps_and_handles_edges() {
    assert_eq!(cascade_released(1000, 95, 0), 0); // nothing queued
    assert_eq!(cascade_released(0, 0, 5), 5); // zero step = reveal all
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-core entrance_reveal cascade`
Expected: FAIL — `cannot find function \`entrance_reveal\`` / `cascade_released`.

- [ ] **Step 3: Implement the functions**

Add after `status_glyph` (before `#[cfg(test)]`) in `crates/ironlint-core/src/watch.rs`:

```rust
/// Cells of a row's width to reveal for the wipe-in entrance, stepped by the
/// caller's frame cadence. Returns `None` once the row is fully revealed
/// (`elapsed_ms >= enter_ms`) or when `enter_ms == 0` (no animation).
pub fn entrance_reveal(elapsed_ms: u64, enter_ms: u64, full_cells: u16) -> Option<u16> {
    if enter_ms == 0 || elapsed_ms >= enter_ms {
        return None;
    }
    let cells = (u64::from(full_cells) * elapsed_ms) / enter_ms;
    Some(u16::try_from(cells).unwrap_or(full_cells))
}

/// How many queued entries have been released for display by `elapsed_ms`,
/// at a `step_ms` cadence: one immediately, then one per step, clamped to
/// `queued`. `step_ms == 0` releases the whole queue at once.
pub fn cascade_released(elapsed_ms: u64, step_ms: u64, queued: usize) -> usize {
    if step_ms == 0 {
        return queued;
    }
    let steps = usize::try_from(elapsed_ms / step_ms).unwrap_or(usize::MAX);
    steps.saturating_add(1).min(queued)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-core entrance_reveal cascade`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-core/src/watch.rs
git commit -m "feat(watch): pure entrance_reveal + cascade_released timing math"
```

---

### Task 2: Cell-accurate line helpers (`truncate_line`, `pad_line`)

Pure helpers on ratatui `Line`s: keep the first N columns (for the wipe) and fill to pane width with a styled run (for the full-width tint). Both live in the CLI watch module.

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch.rs` (add fns near the top after the color consts; add tests in `mod tests`)

**Interfaces:**
- Produces:
  - `fn truncate_line(line: Line<'static>, cells: u16) -> Line<'static>`
  - `fn pad_line(line: Line<'static>, width: u16, style: Style) -> Line<'static>`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/ironlint-cli/src/commands/watch.rs` (`line_text`/`all_text` helpers already exist there):

```rust
#[test]
fn truncate_line_keeps_first_n_columns_across_spans() {
    let line = Line::from(vec![
        Span::styled("abc", Style::default().fg(GREEN)),
        Span::styled("defg", Style::default().fg(MUTED)),
    ]);
    assert_eq!(line_text(&truncate_line(line.clone(), 0)), "");
    assert_eq!(line_text(&truncate_line(line.clone(), 2)), "ab");
    assert_eq!(line_text(&truncate_line(line.clone(), 3)), "abc");
    assert_eq!(line_text(&truncate_line(line.clone(), 5)), "abcde");
    assert_eq!(line_text(&truncate_line(line, 99)), "abcdefg"); // over-long = whole line
}

#[test]
fn pad_line_fills_to_width_with_style() {
    let line = Line::from(Span::raw("abc"));
    let padded = pad_line(line, 6, Style::default().bg(RED_REST));
    assert_eq!(line_text(&padded), "abc   ");
    // the padding span carries the fill style
    let last = padded.spans.last().unwrap();
    assert_eq!(last.style.bg, Some(RED_REST));
}

#[test]
fn pad_line_noop_when_already_at_width() {
    let line = Line::from(Span::raw("abcdef"));
    let padded = pad_line(line, 4, Style::default().bg(RED_REST));
    assert_eq!(line_text(&padded), "abcdef");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-cli truncate_line pad_line`
Expected: FAIL — `cannot find function \`truncate_line\`` / `pad_line`.

- [ ] **Step 3: Implement the helpers**

Add just after the color consts block (below `const MUTED`, ~line 36) in `crates/ironlint-cli/src/commands/watch.rs`. `Span` is already imported (`use ratatui::text::{Line, Span};`); add `Color` is already imported via the style import:

```rust
/// Blocked-row background — a slight, standing dim red (spec §4.2).
const RED_REST: Color = Color::Rgb(36, 16, 21);

/// Horizontal wipe-in duration in ms (spec §2, decision 5).
const ENTER_MS: u64 = 210;

/// Keep the first `cells` display columns of a line, across its spans. Stream
/// content is single-column, so a column is one `char`.
fn truncate_line(line: Line<'static>, cells: u16) -> Line<'static> {
    let mut remaining = usize::from(cells);
    let mut out: Vec<Span<'static>> = Vec::new();
    for span in line.spans {
        if remaining == 0 {
            break;
        }
        let n = span.content.chars().count();
        if n <= remaining {
            remaining -= n;
            out.push(span);
        } else {
            let kept: String = span.content.chars().take(remaining).collect();
            out.push(Span::styled(kept, span.style));
            remaining = 0;
        }
    }
    Line::from(out)
}

/// Extend `line` with a `style`-filled space run until it spans `width`
/// columns (so a tinted background reaches the pane edge).
fn pad_line(mut line: Line<'static>, width: u16, style: Style) -> Line<'static> {
    let cur: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    let target = usize::from(width);
    if cur < target {
        line.spans.push(Span::styled(" ".repeat(target - cur), style));
    }
    line
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-cli truncate_line pad_line`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/src/commands/watch.rs
git commit -m "feat(watch): cell-accurate truncate_line + pad_line helpers"
```

---

### Task 3: `StreamRow`, block tint, and wipe in `stream_lines`

Rework `stream_lines` to take entrance-annotated rows plus pane width. Blocked rows (and their `└ …` detail line) get a full-width red tint; animating rows are truncated to their reveal width. Extract the row-building into a helper so `stream_lines` stays under the complexity cap.

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch.rs` (`stream_lines` at ~line 75; its tests at ~562–665)

**Interfaces:**
- Consumes: `entrance_reveal` (Task 1), `truncate_line`/`pad_line`/`RED_REST`/`ENTER_MS` (Task 2)
- Produces:
  - `pub struct StreamRow<'a> { pub entry: &'a LogEntry, pub age_ms: Option<u64> }`
  - `pub fn stream_lines(rows: &[StreamRow], filter: Option<&str>, width: u16) -> Vec<Line<'static>>`
  - `fn main_row_line(entry: &LogEntry) -> Line<'static>` (private helper)

- [ ] **Step 1: Update existing stream tests + add tint/wipe tests**

The existing stream tests call `stream_lines(&entries, filter)`. Add a small test helper and update call sites, then add the new behavior tests. In `mod tests`, add near the top:

```rust
/// Wrap entries as settled (fully revealed) rows for render tests.
fn settled(entries: &[LogEntry]) -> Vec<StreamRow<'_>> {
    entries.iter().map(|e| StreamRow { entry: e, age_ms: None }).collect()
}

const W: u16 = 80; // test pane width
```

Update every existing `stream_lines(&entries, None)` / `stream_lines(&entries, Some("x"))` call in the stream tests to `stream_lines(&settled(&entries), None, W)` / `stream_lines(&settled(&entries), Some("x"), W)`. (Tests affected: `stream_renders_pass_row_*`, `stream_block_row_*`, `stream_precommit_row_*`, `stream_internal_error_*`, `stream_is_newest_first`, `stream_filter_keeps_only_matching_check`.)

Then add:

```rust
#[test]
fn stream_block_row_is_tinted_full_width_on_row_and_detail() {
    let e = entry(Some("src/lib.rs"), None, "write", Status::Block, 8,
                  vec![prec("ruff", Status::Block, None)]);
    let lines = stream_lines(&settled(&[e]), None, W);
    // main row + detail line, both padded to full width and background-tinted.
    let row = &lines[0];
    let detail = &lines[1];
    assert!(line_text(row).chars().count() as u16 >= W, "block row not padded to width");
    assert!(row.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
            "every span of a blocked row carries the red tint");
    assert!(detail.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
            "blocked detail line is tinted too");
}

#[test]
fn stream_pass_row_is_not_tinted() {
    let e = entry(Some("src/main.rs"), None, "write", Status::Pass, 12, vec![]);
    let lines = stream_lines(&settled(&[e]), None, W);
    assert!(lines[0].spans.iter().all(|s| s.style.bg.is_none()),
            "passing rows have no background");
}

#[test]
fn stream_mid_wipe_row_is_truncated() {
    let e = entry(Some("src/main.rs"), None, "write", Status::Pass, 12, vec![]);
    // age halfway through the wipe -> reveal ~half the pane width.
    let rows = vec![StreamRow { entry: &e, age_ms: Some(ENTER_MS / 2) }];
    let lines = stream_lines(&rows, None, W);
    let shown = line_text(&lines[0]).chars().count();
    assert!(shown > 0 && shown < usize::from(W), "expected a partial reveal, got {shown}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-cli stream_`
Expected: FAIL to compile — `stream_lines` takes 2 args / `StreamRow` not found.

- [ ] **Step 3: Rework `stream_lines`**

Replace the whole `stream_lines` function (lines ~73–125) with the struct, the extracted row builder, and the new body:

```rust
/// A stream entry plus its entrance state. `age_ms` is ms since the row was
/// released for display (`None` once settled); the event loop supplies it.
pub struct StreamRow<'a> {
    pub entry: &'a LogEntry,
    pub age_ms: Option<u64>,
}

/// Build the styled main row for one entry (time · glyph · target · elapsed · badge).
fn main_row_line(entry: &LogEntry) -> Line<'static> {
    let LogEntry::Check {
        ts, file, set_size, event, status, elapsed_ms, ..
    } = entry;
    let glyph = status_glyph(*status);
    let target = target_label(file.as_ref(), *set_size);
    let badge = if event == "pre-commit" { "commit" } else { "write" };
    Line::from(vec![
        Span::styled(format!("{:>8}  ", short_time(ts)), Style::default().fg(MUTED)),
        Span::styled(format!("{glyph}  "), Style::default().fg(status_color(*status))),
        Span::raw(format!("{target:<40}")),
        Span::styled(format!("  {:>6}  ", fmt_elapsed(*elapsed_ms)), Style::default().fg(MUTED)),
        Span::styled(badge.to_string(), Style::default().fg(MUTED)),
    ])
}

/// Apply the standing red tint: background on every span, padded to `width`.
fn tint(line: Line<'static>, width: u16) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| {
            let style = s.style.bg(RED_REST);
            Span::styled(s.content, style)
        })
        .collect();
    pad_line(Line::from(spans), width, Style::default().bg(RED_REST))
}

/// Newest-first stream lines. `filter` keeps only entries whose `checks`
/// contains that check name. `width` is the pane width (for the block tint).
pub fn stream_lines(rows: &[StreamRow], filter: Option<&str>, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for row in rows.iter().rev() {
        let LogEntry::Check { event, status, checks, .. } = row.entry;
        if let Some(f) = filter {
            if !checks.iter().any(|c| c.check == f) {
                continue;
            }
        }
        let reveal = row.age_ms.and_then(|a| entrance_reveal(a, ENTER_MS, width));
        push_row(&mut lines, main_row_line(row.entry), matches!(status, Status::Block), reveal, width);
        for c in checks {
            if let Some(text) = detail_text(&c.check, c.status, c.reason.as_deref(), event) {
                let d = Line::from(Span::styled(text, Style::default().fg(status_color(c.status))));
                push_row(&mut lines, d, matches!(c.status, Status::Block), reveal, width);
            }
        }
    }
    lines
}

/// Apply optional tint + optional wipe truncation, then push.
fn push_row(
    lines: &mut Vec<Line<'static>>,
    mut line: Line<'static>,
    blocked: bool,
    reveal: Option<u16>,
    width: u16,
) {
    if blocked {
        line = tint(line, width);
    }
    if let Some(k) = reveal {
        line = truncate_line(line, k);
    }
    lines.push(line);
}
```

Note: the imports already include `use ironlint_core::watch::{... status_glyph, ...}`. Add `entrance_reveal` to that `use` list.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-cli stream_`
Expected: PASS (existing stream tests + 3 new).

- [ ] **Step 5: Clippy the complexity**

Run: `cargo clippy -p ironlint-cli --all-targets -- -D warnings`
Expected: clean. (`stream_lines` delegates row building to `main_row_line`/`push_row`, keeping cognitive complexity under 15.)

- [ ] **Step 6: Commit**

```bash
git add crates/ironlint-cli/src/commands/watch.rs
git commit -m "feat(watch): StreamRow entrance state, full-width block tint, wipe-in"
```

---

### Task 4: Drop the clock, add the verdict dot

Remove the wall clock from the header and `ui` signature; the corner becomes a static `● PASS` / `● BLOCK` whose dot carries the verdict color.

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch.rs` (`header_line` ~200, `ui` ~241; their tests ~759–878)

**Interfaces:**
- Consumes: `StreamRow` (Task 3)
- Produces:
  - `fn header_line(state: &ViewState, summary: &LogSummary) -> Line<'static>` (drops `clock`)
  - `pub fn ui(frame, rows: &[StreamRow], summary, armed, state, config_loaded)` (drops `clock`)

- [ ] **Step 1: Update header/ui tests**

In `mod tests`, update the `ui(...)` call sites (in `ui_stream_view_renders_feed_and_footer`, `ui_explorer_view_renders_table`, `ui_config_unavailable_shows_degraded_banner`, `ui_cold_start_shows_waiting_hint`) to drop the clock arg and pass rows. Example for the stream test:

```rust
// before: ui(f, &entries, &summary, 7, &state, "14:24:00", true)
// after:
term.draw(|f| ui(f, &settled(&entries), &summary, 7, &state, true)).unwrap();
```

For the empty-body tests pass `&[]` (an empty `&[StreamRow]`). Then add:

```rust
#[test]
fn header_shows_verdict_dot_not_a_clock() {
    let pass = summary_with(&["ruff"]); // no blocks
    let line = header_line(&ViewState::default(), &pass);
    let text = line_text(&line);
    assert!(text.contains("● PASS"), "got: {text}");
    assert!(!text.contains(':'), "clock should be gone, got: {text}");
}

#[test]
fn header_flips_to_block_when_blocks_present() {
    let mut s = summary_with(&["ruff"]);
    s.blocks = 3;
    let text = line_text(&header_line(&ViewState::default(), &s));
    assert!(text.contains("● BLOCK"), "got: {text}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ironlint-cli header_ ui_`
Expected: FAIL to compile — `header_line`/`ui` arity mismatch.

- [ ] **Step 3: Update `header_line` and `ui`**

Replace `header_line` (lines ~200–222):

```rust
fn header_line(state: &ViewState, summary: &LogSummary) -> Line<'static> {
    let (stream_style, explorer_style) = match state.view {
        View::Stream => (
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
            Style::default().fg(MUTED),
        ),
        View::Explorer => (
            Style::default().fg(MUTED),
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        ),
    };
    let (word, dot) = if summary.blocks > 0 { ("BLOCK", ORANGE) } else { ("PASS", GREEN) };
    Line::from(vec![
        Span::styled("≈ stream", stream_style),
        Span::raw("   "),
        Span::styled("▤ explorer", explorer_style),
        Span::raw("        "),
        Span::styled("● ", Style::default().fg(dot)),
        Span::styled(word, Style::default().fg(MUTED)),
    ])
}
```

In `ui` (lines ~241–298): change the signature to take `rows: &[StreamRow]` and drop `clock`; update the header call and the body construction:

```rust
pub fn ui(
    frame: &mut Frame,
    rows: &[StreamRow],
    summary: &LogSummary,
    armed: usize,
    state: &ViewState,
    config_loaded: bool,
) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    frame.render_widget(Paragraph::new(header_line(state, summary)), chunks[0]);

    let width = chunks[1].width;
    let mut body = if !config_loaded && matches!(state.view, View::Stream) && rows.is_empty() {
        vec![]
    } else if config_loaded && matches!(state.view, View::Stream) && rows.is_empty() {
        vec![Line::from(Span::styled(
            "waiting for edits\u{2026}",
            Style::default().fg(MUTED),
        ))]
    } else {
        match state.view {
            View::Stream => stream_lines(rows, state.filter.as_deref(), width),
            View::Explorer => explorer_lines(summary, state.selected),
        }
    };

    if !config_loaded {
        body.insert(
            0,
            Line::from(Span::styled(
                "\u{26a0} config unavailable \u{2014} tailing log only",
                Style::default().fg(AMBER),
            )),
        );
    }

    frame.render_widget(
        Paragraph::new(body).block(Block::default().borders(Borders::TOP)),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(footer_line(state, summary, armed)),
        chunks[2],
    );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ironlint-cli header_ ui_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ironlint-cli/src/commands/watch.rs
git commit -m "feat(watch): replace header clock with a static verdict dot"
```

---

### Task 5: Wire the event loop — reveal queue, cascade timer, tick cadence

Give `event_loop` the entrance model: hold newly-observed entries in a queue, release them on the cascade cadence (stamping a jitter-proof `revealed_at`), compute each visible row's `age_ms`, and poll fast only while animating.

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch.rs` (`event_loop` lines ~332–375; add poll consts near `ENTER_MS`)

**Interfaces:**
- Consumes: `cascade_released` (Task 1), `StreamRow`/`ui` (Tasks 3–4), `ENTER_MS` (Task 2)

- [ ] **Step 1: Add cadence constants**

Below `ENTER_MS` (added in Task 2) in `crates/ironlint-cli/src/commands/watch.rs`:

```rust
/// Cascade release cadence — one queued row shown per this interval (spec §2).
const STEP_MS: u64 = 95;
/// Poll interval while an entrance is in flight (smooth-enough stepping).
const ACTIVE_POLL_MS: u64 = 40;
/// Poll interval when nothing is animating — a quiet watcher stays near-zero CPU.
const IDLE_POLL_MS: u64 = 250;
```

Add `use std::time::Instant;` next to the existing `use std::time::Duration;`.

- [ ] **Step 2: Replace the `event_loop` body**

Replace `event_loop` (lines ~332–375) with:

```rust
fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    dir: &Path,
    armed: &[ArmedCheck],
    config_loaded: bool,
) -> Result<()> {
    let log = dir.join(".ironlint/log.jsonl");
    let mut state = ViewState::default();
    let mut entries: Vec<LogEntry> = Vec::new();
    let mut offset: u64 = 0;

    // Entrance model: `shown` entries are on screen; `revealed_at[i]` is when
    // entry i was released. A burst releases one row per STEP_MS from `anchor`.
    let mut shown: usize = 0;
    let mut revealed_at: Vec<Instant> = Vec::new();
    let mut anchor = Instant::now();
    let mut anchor_base: usize = 0;

    loop {
        if let Ok((new, reset)) = ironlint_core::telemetry::read_since(&log, &mut offset) {
            if reset {
                entries.clear();
                shown = 0;
                revealed_at.clear();
                anchor_base = 0;
            }
            entries.extend(new);
        }

        let now = Instant::now();
        // While caught up, keep the anchor at `now` so the next burst's first
        // row releases immediately; once behind, release on the STEP_MS cadence.
        if shown == entries.len() {
            anchor = now;
            anchor_base = shown;
        } else {
            let queued = entries.len() - anchor_base;
            let elapsed = u64::try_from(now.duration_since(anchor).as_millis()).unwrap_or(u64::MAX);
            let target = anchor_base + cascade_released(elapsed, STEP_MS, queued);
            while shown < target {
                // Exact per-row stagger, independent of tick jitter.
                let offset_ms = STEP_MS * u64::try_from(shown - anchor_base).unwrap_or(0);
                revealed_at.push(anchor + Duration::from_millis(offset_ms));
                shown += 1;
            }
        }

        let mut animating = shown < entries.len();
        let rows: Vec<StreamRow> = (0..shown)
            .map(|i| {
                let age = u64::try_from(now.saturating_duration_since(revealed_at[i]).as_millis())
                    .unwrap_or(u64::MAX);
                let age_ms = if age < ENTER_MS {
                    animating = true;
                    Some(age)
                } else {
                    None
                };
                StreamRow { entry: &entries[i], age_ms }
            })
            .collect();

        let summary = summarize(&entries[..shown], armed);
        terminal.draw(|f| ui(f, &rows, &summary, armed.len(), &state, config_loaded))?;

        let poll = if animating { ACTIVE_POLL_MS } else { IDLE_POLL_MS };
        if event::poll(Duration::from_millis(poll))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press
                    && handle_key(key.code, &mut state, &summary) == Loop::Quit
                {
                    return Ok(());
                }
            }
        }
    }
}
```

Remove the now-unused `let clock = short_time(...)` line and the `chrono` call it made. `short_time` is still used by core; no import change needed here (it was used only for `clock`). If `short_time` becomes unused in this file, drop it from the `use ironlint_core::watch::{...}` list to keep clippy clean.

- [ ] **Step 3: Verify it compiles and all tests pass**

Run: `cargo test -p ironlint-cli`
Expected: PASS (all watch tests green).

- [ ] **Step 4: Clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean. If `event_loop` trips cognitive-complexity (it gained the entrance block), extract the release step into a helper `fn advance_shown(entries_len, anchor, anchor_base, now, shown, revealed_at)` and call it — do not `#[allow]`.

- [ ] **Step 5: Coverage gate**

Run: `bash scripts/ci-coverage.sh`
Expected: `crates/ironlint-cli/src/commands/watch.rs` and `crates/ironlint-core/src/watch.rs` both ≥80% region. The pure helpers carry the new coverage; the `event_loop` additions are the documented uncovered terminal glue. If the CLI file dips below 80% because `event_loop` grew, add a `stream_lines`/`ui` test exercising a block row mid-wipe (a `StreamRow { age_ms: Some(ENTER_MS/2) }` over a `Status::Block` entry) to lift covered regions.

- [ ] **Step 6: Manual smoke (real terminal)**

Build and drive it against a scratch log:

```bash
cargo build --release
cd "$(mktemp -d)" && mkdir -p .ironlint
printf '{"kind":"check","ts":"2026-07-04T14:24:01Z","file":"src/main.rs","set_size":null,"event":"write","status":"pass","elapsed_ms":12,"checks":[]}\n' >> .ironlint/log.jsonl
# In one pane: run the watcher (needs the project's .ironlint.yml or it degrades gracefully)
# /path/to/target/release/ironlint watch
# In another: append a block line and watch it wipe in with the red tint:
printf '{"kind":"check","ts":"2026-07-04T14:24:07Z","file":"src/lib.rs","set_size":null,"event":"write","status":"block","elapsed_ms":8,"checks":[{"check":"ruff","status":"block","reason":null}]}\n' >> .ironlint/log.jsonl
```
Expected: new rows wipe in left→right; the block row shows a full-width dim-red tint on its row and its `└ ruff · write rejected` line; the header reads `● BLOCK`; no wall clock. Quit with `q`.

- [ ] **Step 7: Clean up the build artifact**

Run: `cargo clean -p ironlint-cli` (removes the release binary built only for the smoke test), and delete the scratch `mktemp` dir.

- [ ] **Step 8: Commit**

```bash
git add crates/ironlint-cli/src/commands/watch.rs
git commit -m "feat(watch): live tail — cascade reveal queue + animation-aware poll cadence"
```

---

## Self-Review

**Spec coverage:**
- §2 decision 1 (drop clock, static dot) → Task 4. ✓
- §2 decision 2 (wipe-in) → Task 3 (`entrance_reveal` + `truncate_line`) + Task 1. ✓
- §2 decision 3 (cascade) → Task 1 (`cascade_released`) + Task 5 (reveal queue). ✓
- §2 decision 4 (constant full-width tint on row + detail) → Task 3 (`tint`/`pad_line`, applied to main + detail). ✓
- §2 decision 5 (fixed medium speed) → `ENTER_MS`/`STEP_MS` consts, not configurable. ✓
- §4.1 pure timing math in core → Task 1. ✓
- §4.2 `StreamRow` + width + cell helpers → Tasks 2–3. ✓ (Refinement: `StreamRow` carries `age_ms` and `stream_lines` derives `reveal` via `entrance_reveal`, rather than the spec's illustrative pre-computed `reveal` field — same intent, keeps the timing constant `ENTER_MS` in one place and the loop supplying only raw data.)
- §4.3 header/ui lose clock → Task 4. ✓
- §4.4 event-loop reveal queue + poll cadence → Task 5. ✓
- §5 testing (boundaries, truncation, full-width tint, verdict flip) → Tasks 1–4 tests. ✓
- §6 out of scope — no task touches Explorer/gauge/pulse/other commands. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every test shows assertions. ✓

**Type consistency:** `entrance_reveal(u64,u64,u16)->Option<u16>`, `cascade_released(u64,u64,usize)->usize`, `StreamRow{entry:&LogEntry, age_ms:Option<u64>}`, `stream_lines(&[StreamRow],Option<&str>,u16)`, `ui(Frame,&[StreamRow],&LogSummary,usize,&ViewState,bool)`, `header_line(&ViewState,&LogSummary)` — used consistently across Tasks 1–5. `RED_REST`/`ENTER_MS`/`STEP_MS` defined once in Task 2/5. ✓

**Post-implementation (CLAUDE.md rule):** after Task 5, request code review from a separate agent before considering the work done.
