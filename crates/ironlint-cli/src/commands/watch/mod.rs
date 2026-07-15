//! `ironlint watch` — a read-only live TUI over `.ironlint/log.jsonl`.
//!
//! All decision logic (aggregation in core, plus `handle_key`/`stream_lines`/
//! `explorer_lines`/`ui`/`advance_cascade` here, and the `truncate_line`/
//! `pad_line` line helpers) is pure and tested; the only uncovered code is
//! the terminal setup (`run_tui`) and event loop (`event_loop`), kept minimal.
use anyhow::Result;
use ironlint_core::runner::IronLintEngine;
use ironlint_core::telemetry::LogEntry;
use ironlint_core::watch::{cascade_released, summarize, ArmedCheck};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

mod render;
mod state;

#[allow(unused_imports)]
pub use render::{explorer_lines, stream_lines, ui, StreamRow};
pub use state::{handle_key, Loop, View, ViewState};

// Existing root tests currently reach KeyCode through `use super::*`.
#[cfg(test)]
use ratatui::crossterm::event::KeyCode;

// Existing root tests still use these render-private helpers and Ratatui types.
#[cfg(test)]
use ironlint_core::watch::entrance_reveal;
#[cfg(test)]
use ratatui::style::Style;
#[cfg(test)]
use ratatui::text::Span;
#[cfg(test)]
use render::{header_line, pad_line, truncate_line, GREEN, MUTED, RED_REST};

/// Horizontal wipe-in duration in ms (spec §2, decision 5).
const ENTER_MS: u64 = 210;

/// Cascade release cadence — one queued row shown per this interval (spec §2).
const STEP_MS: u64 = 95;
/// Poll interval while an entrance is in flight (smooth-enough stepping).
const ACTIVE_POLL_MS: u64 = 40;
/// Poll interval when nothing is animating — a quiet watcher stays near-zero CPU.
const IDLE_POLL_MS: u64 = 250;

/// Resolve the armed-check projection from `<dir>/.ironlint.yml`. Best-effort:
/// returns `(checks, true)` on success, `([], false)` on any load error so the
/// caller can show a degraded-config banner while still tailing the log.
fn load_armed(dir: &Path) -> (Vec<ArmedCheck>, bool) {
    let config = dir.join(".ironlint.yml");
    match IronLintEngine::load(&config) {
        Ok(engine) => (
            engine
                .checks()
                .iter()
                .map(|(name, check)| ArmedCheck {
                    name: name.clone(),
                    on: check.on.clone(),
                })
                .collect(),
            true,
        ),
        Err(_) => (Vec::new(), false),
    }
}

fn run_tui(dir: &Path, armed: &[ArmedCheck], config_loaded: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = event_loop(&mut terminal, dir, armed, config_loaded);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

/// Cascade release state, timed in ms since the watch loop started so the
/// release decision is pure and unit-testable (no real clock).
struct Cascade {
    shown: usize,
    anchor_ms: u64,
    anchor_base: usize,
}

/// Milliseconds of a duration, saturating (`u128` -> `u64`).
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Advance the cascade one tick: release every entry due by `now_ms`, pushing
/// each released row's reveal time (ms) onto `revealed_ms`.
///
/// Re-anchors to `now_ms` at the start of a fresh burst (`shown == anchor_base`,
/// nothing released yet) so a row's wipe is measured from its arrival, not from
/// however long the idle poll slept before the entry was seen.
fn advance_cascade(
    c: &mut Cascade,
    entries_len: usize,
    now_ms: u64,
    revealed_ms: &mut Vec<Option<u64>>,
) {
    if c.shown >= entries_len {
        c.anchor_base = c.shown;
        return;
    }
    if c.shown == c.anchor_base {
        c.anchor_ms = now_ms;
    }
    let queued = entries_len - c.anchor_base;
    let elapsed = now_ms.saturating_sub(c.anchor_ms);
    let target = c.anchor_base + cascade_released(elapsed, STEP_MS, queued);
    while c.shown < target {
        let offset = STEP_MS * u64::try_from(c.shown - c.anchor_base).unwrap_or(0);
        revealed_ms.push(Some(c.anchor_ms + offset));
        c.shown += 1;
    }
}

/// Mark `n` already-present entries as settled (no wipe-in): the launch
/// backlog, or the full re-read after a log rotation. Pushes `None` reveal
/// times and advances the cascade to "caught up" so genuinely new arrivals
/// afterwards still cascade normally. Pure + unit-tested.
fn prime_backlog(revealed_ms: &mut Vec<Option<u64>>, n: usize, c: &mut Cascade) {
    for _ in 0..n {
        revealed_ms.push(None);
    }
    c.shown = n;
    c.anchor_base = n;
}

fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    dir: &Path,
    armed: &[ArmedCheck],
    config_loaded: bool,
) -> Result<()>
where
    // ratatui 0.30 gave `Backend` an associated `Error` type; bound it so
    // `terminal.draw(..)?` can convert into `anyhow::Error`. Both backends we
    // use (`CrosstermBackend`, `TestBackend`) surface `std::io::Error`.
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let log = dir.join(".ironlint/log.jsonl");
    let mut state = ViewState::default();
    let mut entries: Vec<LogEntry> = Vec::new();
    let mut offset: u64 = 0;
    let mut primed = false;

    let loop_start = Instant::now();
    let mut cascade = Cascade {
        shown: 0,
        anchor_base: 0,
        anchor_ms: 0,
    };
    let mut revealed_ms: Vec<Option<u64>> = Vec::new();

    loop {
        if let Ok((new, reset)) = ironlint_core::telemetry::read_since(&log, &mut offset) {
            if reset {
                entries.clear();
                revealed_ms.clear();
                cascade.shown = 0;
                cascade.anchor_base = 0;
                primed = false;
            }
            entries.extend(new);

            // Prime any already-present entries so they render instantly on
            // the first successful tick (and after a log-rotation full
            // reread) instead of cascading one row per STEP_MS — a launch
            // backlog is history, not live traffic. Gated on a successful
            // read: a first-tick I/O error must NOT mark an empty backlog as
            // primed, or the real backlog would silently animate once the
            // read later succeeds.
            if !primed {
                prime_backlog(&mut revealed_ms, entries.len(), &mut cascade);
                primed = true;
            }
        }

        let now_ms = millis(loop_start.elapsed());
        advance_cascade(&mut cascade, entries.len(), now_ms, &mut revealed_ms);

        let mut animating = cascade.shown < entries.len();
        let rows: Vec<StreamRow> = (0..cascade.shown)
            .map(|i| {
                let age_ms = match revealed_ms[i] {
                    None => None, // backlog row: settled, no wipe
                    Some(t) => {
                        let age = now_ms.saturating_sub(t);
                        if age < ENTER_MS {
                            animating = true;
                            Some(age)
                        } else {
                            None
                        }
                    }
                };
                StreamRow {
                    entry: &entries[i],
                    age_ms,
                }
            })
            .collect();

        let summary = summarize(&entries[..cascade.shown], armed);
        terminal.draw(|f| ui(f, &rows, &summary, armed.len(), &state, config_loaded))?;

        let poll = if animating {
            ACTIVE_POLL_MS
        } else {
            IDLE_POLL_MS
        };
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

/// Entry point. Requires an interactive terminal; otherwise exits 1 with a hint.
pub fn run(dir: &Path) -> Result<i32> {
    if !std::io::stdout().is_terminal() {
        eprintln!("ironlint watch: requires an interactive terminal (no TTY detected).");
        eprintln!(
            "A non-interactive `--once` snapshot is planned; for now inspect {}/.ironlint/log.jsonl directly.",
            dir.display()
        );
        return Ok(1);
    }
    let (armed, config_loaded) = load_armed(dir);
    run_tui(dir, &armed, config_loaded)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironlint_core::config::Lifecycle;
    use ironlint_core::telemetry::{LogEntry, PerCheckRecord};
    use ironlint_core::verdict::Status;
    use ironlint_core::watch::{CheckRollup, CheckRollup as Roll, LogSummary};
    use ratatui::text::Line;

    fn line_text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn all_text(lines: &[Line]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }
    fn entry(
        file: Option<&str>,
        set: Option<usize>,
        event: &str,
        status: Status,
        elapsed_ms: u64,
        recs: Vec<PerCheckRecord>,
    ) -> LogEntry {
        LogEntry::Check {
            ts: "2026-06-28T14:23:09+00:00".into(),
            file: file.map(Into::into),
            set_size: set,
            event: event.into(),
            status,
            elapsed_ms,
            checks: recs,
        }
    }

    /// Wrap entries as settled (fully revealed) rows for render tests.
    fn settled(entries: &[LogEntry]) -> Vec<StreamRow<'_>> {
        entries
            .iter()
            .map(|e| StreamRow {
                entry: e,
                age_ms: None,
            })
            .collect()
    }

    const W: u16 = 80; // test pane width
    fn prec(name: &str, status: Status, reason: Option<&str>) -> PerCheckRecord {
        PerCheckRecord {
            check: name.into(),
            step: None,
            status,
            elapsed_ms: 12,
            reason: reason.map(Into::into),
        }
    }
    // Used by Task 3.2 and 3.3 tests added in subsequent commits.
    fn roll(name: &str, runs: usize, blocks: usize, internal: usize, p50: Option<u64>) -> Roll {
        Roll {
            name: name.into(),
            on: vec![Lifecycle::Write],
            runs,
            blocks,
            internal,
            p50_ms: p50,
        }
    }

    fn summary_with(names: &[&str]) -> LogSummary {
        LogSummary {
            runs: 0,
            blocks: 0,
            internal: 0,
            pass: 0,
            rollups: names
                .iter()
                .map(|n| CheckRollup {
                    name: (*n).into(),
                    on: vec![Lifecycle::Write],
                    runs: 0,
                    blocks: 0,
                    internal: 0,
                    p50_ms: None,
                })
                .collect(),
        }
    }

    // ── Task 4.1: load_armed ─────────────────────────────────────────────────

    #[test]
    fn load_armed_reads_check_names_and_lifecycles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".ironlint.yml"),
            "checks:\n  lint:\n    files: \"*.ts\"\n    run: \"true\"\n    on: [write, pre-commit]\n",
        )
        .unwrap();
        let (armed, ok) = load_armed(dir.path());
        assert!(ok);
        assert_eq!(armed.len(), 1);
        assert_eq!(armed[0].name, "lint");
        assert_eq!(
            armed[0].on,
            vec![
                ironlint_core::config::Lifecycle::Write,
                ironlint_core::config::Lifecycle::PreCommit
            ]
        );
    }

    #[test]
    fn load_armed_is_empty_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (armed, ok) = load_armed(dir.path());
        assert!(armed.is_empty());
        assert!(!ok);
    }

    // ── Task 3.1: stream_lines ────────────────────────────────────────────────

    #[test]
    fn stream_renders_pass_row_with_time_file_elapsed_event() {
        let e = vec![entry(
            Some("src/auth.ts"),
            None,
            "write",
            Status::Pass,
            12,
            vec![prec("lint", Status::Pass, None)],
        )];
        let text = all_text(&stream_lines(&settled(&e), None, W));
        assert!(text.contains("14:23:09"));
        assert!(text.contains("src/auth.ts"));
        assert!(text.contains("12ms"));
        assert!(text.contains("write"));
    }

    #[test]
    fn stream_block_row_has_check_and_write_rejected_no_message() {
        let e = vec![entry(
            Some("src/auth.test.ts"),
            None,
            "write",
            Status::Block,
            12,
            vec![prec("no-focused-tests", Status::Block, None)],
        )];
        let text = all_text(&stream_lines(&settled(&e), None, W));
        assert!(text.contains("no-focused-tests"));
        assert!(text.contains("write rejected"));
        assert!(!text.contains("exited"));
    }

    #[test]
    fn stream_precommit_row_shows_set_size_and_commit() {
        let e = vec![entry(
            None,
            Some(47),
            "pre-commit",
            Status::Pass,
            12,
            vec![prec("lint", Status::Pass, None)],
        )];
        let text = all_text(&stream_lines(&settled(&e), None, W));
        assert!(text.contains("pre-commit · 47 files"));
        assert!(text.contains("commit"));
    }

    #[test]
    fn stream_internal_error_shows_reason() {
        let e = vec![entry(
            Some("big.ts"),
            None,
            "write",
            Status::InternalError,
            12,
            vec![prec("types-pass", Status::InternalError, Some("timeout"))],
        )];
        let text = all_text(&stream_lines(&settled(&e), None, W));
        assert!(text.contains("check error: timeout"));
    }

    #[test]
    fn stream_is_newest_first() {
        let mut a = entry(
            Some("old.ts"),
            None,
            "write",
            Status::Pass,
            12,
            vec![prec("lint", Status::Pass, None)],
        );
        let LogEntry::Check { ts, .. } = &mut a;
        *ts = "2026-06-28T14:00:00+00:00".into();
        let b = entry(
            Some("new.ts"),
            None,
            "write",
            Status::Pass,
            12,
            vec![prec("lint", Status::Pass, None)],
        );
        let lines = stream_lines(&settled(&[a, b]), None, W);
        assert!(line_text(&lines[0]).contains("new.ts"));
    }

    #[test]
    fn stream_filter_keeps_only_matching_check() {
        let e = vec![
            entry(
                Some("a.ts"),
                None,
                "write",
                Status::Pass,
                12,
                vec![prec("lint", Status::Pass, None)],
            ),
            entry(
                Some("b.ts"),
                None,
                "write",
                Status::Pass,
                12,
                vec![prec("types", Status::Pass, None)],
            ),
        ];
        let text = all_text(&stream_lines(&settled(&e), Some("types"), W));
        assert!(text.contains("b.ts"));
        assert!(!text.contains("a.ts"));
    }

    #[test]
    fn stream_block_row_is_tinted_full_width_on_row_and_detail() {
        let e = entry(
            Some("src/lib.rs"),
            None,
            "write",
            Status::Block,
            8,
            vec![prec("ruff", Status::Block, None)],
        );
        let lines = stream_lines(&settled(&[e]), None, W);
        // main row + detail line, both padded to full width and background-tinted.
        let row = &lines[0];
        let detail = &lines[1];
        assert!(
            line_text(row).chars().count() >= usize::from(W),
            "block row not padded to width"
        );
        assert!(
            row.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
            "every span of a blocked row carries the red tint"
        );
        assert!(
            detail.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
            "blocked detail line is tinted too"
        );
    }

    #[test]
    fn stream_pass_row_is_not_tinted() {
        let e = entry(Some("src/main.rs"), None, "write", Status::Pass, 12, vec![]);
        let lines = stream_lines(&settled(&[e]), None, W);
        assert!(
            lines[0].spans.iter().all(|s| s.style.bg.is_none()),
            "passing rows have no background"
        );
    }

    #[test]
    fn stream_mid_wipe_row_is_truncated() {
        let e = entry(Some("src/main.rs"), None, "write", Status::Pass, 12, vec![]);
        // age halfway through the wipe -> reveal ~half the pane width.
        let rows = vec![StreamRow {
            entry: &e,
            age_ms: Some(ENTER_MS / 2),
        }];
        let lines = stream_lines(&rows, None, W);
        let shown = line_text(&lines[0]).chars().count();
        assert!(
            shown > 0 && shown < usize::from(W),
            "expected a partial reveal, got {shown}"
        );
    }

    #[test]
    fn stream_block_row_mid_wipe_is_tinted_and_truncated() {
        // Proves push_row's tint-then-truncate order: a blocked row that is
        // still animating must show BOTH the red tint on its revealed spans
        // AND a partial (not full-width) reveal.
        let e = entry(
            Some("src/lib.rs"),
            None,
            "write",
            Status::Block,
            8,
            vec![prec("ruff", Status::Block, None)],
        );
        let rows = vec![StreamRow {
            entry: &e,
            age_ms: Some(ENTER_MS / 2),
        }];
        let lines = stream_lines(&rows, None, W);
        let row = &lines[0];
        assert!(
            row.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
            "mid-wipe blocked row must keep the red tint on its revealed spans"
        );
        let shown = line_text(row).chars().count();
        assert!(
            shown > 0 && shown < usize::from(W),
            "mid-wipe blocked row must be truncated to a partial reveal, got {shown}"
        );
    }

    #[test]
    fn stream_block_entry_tints_block_detail_but_not_internal_detail() {
        // Entry-level status is Block (one of its checks blocked); it carries
        // both a Block sub-check and an InternalError sub-check. Proves the
        // per-check tint keying: the main row and the blocking check's detail
        // line get the red tint, but the InternalError detail line does not.
        let e = entry(
            Some("src/lib.rs"),
            None,
            "write",
            Status::Block,
            8,
            vec![
                prec("ruff", Status::Block, None),
                prec("types-check", Status::InternalError, Some("timeout")),
            ],
        );
        let lines = stream_lines(&settled(&[e]), None, W);
        let main = &lines[0];
        let block_detail = &lines[1];
        let internal_detail = &lines[2];
        assert!(
            main.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
            "entry blocked -> main row tinted full width"
        );
        assert!(
            block_detail
                .spans
                .iter()
                .all(|s| s.style.bg == Some(RED_REST)),
            "blocking check's own detail line is tinted red"
        );
        assert!(
            internal_detail.spans.iter().all(|s| s.style.bg.is_none()),
            "internal-error detail line keeps its amber text untinted"
        );
        assert!(line_text(internal_detail).contains("check error"));
    }

    // ── Task 3.2: explorer_lines ──────────────────────────────────────────────

    #[test]
    fn explorer_summary_line_has_totals_and_pass_pct() {
        let s = LogSummary {
            runs: 159,
            blocks: 4,
            internal: 1,
            pass: 154,
            rollups: vec![],
        };
        let text = all_text(&explorer_lines(&s, 0));
        assert!(text.contains("159 runs"));
        assert!(text.contains("4 blocks"));
        assert!(text.contains("1 internal"));
        assert!(text.contains("97% pass"));
    }

    #[test]
    fn explorer_summary_shows_dash_pass_on_empty_log() {
        let s = LogSummary {
            runs: 0,
            blocks: 0,
            internal: 0,
            pass: 0,
            rollups: vec![],
        };
        let text = all_text(&explorer_lines(&s, 0));
        assert!(text.contains("— pass"));
    }

    #[test]
    fn explorer_lists_blocking_then_divider_then_clean() {
        let s = LogSummary {
            runs: 10,
            blocks: 3,
            internal: 0,
            pass: 7,
            rollups: vec![
                roll("nft", 15, 3, 0, Some(11)),
                roll("no-secrets", 50, 0, 0, Some(3)),
            ],
        };
        let lines = explorer_lines(&s, 0);
        let text = all_text(&lines);
        assert!(text.contains("nft"));
        assert!(text.contains("20%"));
        assert!(text.contains("11ms"));
        assert!(text.contains("NO BLOCKS IN LOG"));
        assert!(text.contains("no-secrets"));
        let nft_idx = lines
            .iter()
            .position(|l| line_text(l).contains("nft"))
            .unwrap();
        let div_idx = lines
            .iter()
            .position(|l| line_text(l).contains("NO BLOCKS IN LOG"))
            .unwrap();
        let sec_idx = lines
            .iter()
            .position(|l| line_text(l).contains("no-secrets"))
            .unwrap();
        assert!(nft_idx < div_idx && div_idx < sec_idx);
    }

    #[test]
    fn explorer_shows_internal_warning_and_dash_p50() {
        let s = LogSummary {
            runs: 1,
            blocks: 0,
            internal: 1,
            pass: 0,
            rollups: vec![roll("types", 1, 0, 1, None)],
        };
        let text = all_text(&explorer_lines(&s, 0));
        assert!(text.contains("⚠ 1"));
        assert!(text.contains("—"));
    }

    #[test]
    fn explorer_omits_divider_when_all_checks_block() {
        let s = LogSummary {
            runs: 1,
            blocks: 1,
            internal: 0,
            pass: 0,
            rollups: vec![roll("only", 1, 1, 0, Some(5))],
        };
        let text = all_text(&explorer_lines(&s, 0));
        assert!(!text.contains("NO BLOCKS IN LOG"));
    }

    // ── Task 3.3: ui() ───────────────────────────────────────────────────────

    #[test]
    fn ui_stream_view_renders_feed_and_footer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let entries = vec![entry(
            Some("src/auth.ts"),
            None,
            "write",
            Status::Pass,
            12,
            vec![prec("lint", Status::Pass, None)],
        )];
        let summary = LogSummary {
            runs: 1,
            blocks: 0,
            internal: 0,
            pass: 1,
            rollups: vec![roll("lint", 1, 0, 0, Some(5))],
        };
        let state = ViewState::default();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| ui(f, &settled(&entries), &summary, 7, &state, true))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("stream"));
        assert!(text.contains("explorer"));
        assert!(text.contains("src/auth.ts"));
        assert!(text.contains("7 checks armed"));
        assert!(text.contains("quit"));
    }

    #[test]
    fn ui_explorer_view_renders_table() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let summary = LogSummary {
            runs: 10,
            blocks: 1,
            internal: 0,
            pass: 9,
            rollups: vec![roll("nft", 5, 1, 0, Some(11))],
        };
        let state = ViewState {
            view: View::Explorer,
            selected: 0,
            filter: None,
        };
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| ui(f, &[], &summary, 7, &state, true))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("RANKED BY BLOCKS"));
        assert!(text.contains("nft"));
    }

    // ── New ui state tests ────────────────────────────────────────────────────

    #[test]
    fn ui_config_unavailable_shows_degraded_banner() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let summary = LogSummary {
            runs: 0,
            blocks: 0,
            internal: 0,
            pass: 0,
            rollups: vec![],
        };
        let state = ViewState::default();
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| ui(f, &[], &summary, 0, &state, false))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("config unavailable"));
    }

    #[test]
    fn ui_cold_start_shows_waiting_hint() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let summary = LogSummary {
            runs: 0,
            blocks: 0,
            internal: 0,
            pass: 0,
            rollups: vec![],
        };
        let state = ViewState::default(); // Stream view, no filter
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| ui(f, &[], &summary, 0, &state, true))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("waiting for edits"));
    }

    // ── Task 4: header verdict dot ────────────────────────────────────────────

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

    // ── Existing Phase 2 tests (handle_key) ──────────────────────────────────

    #[test]
    fn q_and_esc_quit() {
        let mut s = ViewState::default();
        assert_eq!(
            handle_key(KeyCode::Char('q'), &mut s, &summary_with(&[])),
            Loop::Quit
        );
        assert_eq!(
            handle_key(KeyCode::Esc, &mut s, &summary_with(&[])),
            Loop::Quit
        );
    }

    #[test]
    fn tab_toggles_view() {
        let mut s = ViewState::default();
        assert!(matches!(s.view, View::Stream));
        handle_key(KeyCode::Tab, &mut s, &summary_with(&[]));
        assert!(matches!(s.view, View::Explorer));
        handle_key(KeyCode::Tab, &mut s, &summary_with(&[]));
        assert!(matches!(s.view, View::Stream));
    }

    #[test]
    fn down_up_clamp_in_explorer() {
        let mut s = ViewState {
            view: View::Explorer,
            selected: 0,
            filter: None,
        };
        let sum = summary_with(&["a", "b"]);
        handle_key(KeyCode::Down, &mut s, &sum);
        assert_eq!(s.selected, 1);
        handle_key(KeyCode::Down, &mut s, &sum); // clamp at len-1
        assert_eq!(s.selected, 1);
        handle_key(KeyCode::Up, &mut s, &sum);
        assert_eq!(s.selected, 0);
        handle_key(KeyCode::Up, &mut s, &sum); // clamp at 0
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn enter_in_explorer_filters_stream() {
        let mut s = ViewState {
            view: View::Explorer,
            selected: 1,
            filter: None,
        };
        let sum = summary_with(&["a", "b"]);
        handle_key(KeyCode::Enter, &mut s, &sum);
        assert_eq!(s.filter.as_deref(), Some("b"));
        assert!(matches!(s.view, View::Stream));
    }

    #[test]
    fn navigation_returns_continue() {
        let mut s = ViewState::default();
        assert_eq!(
            handle_key(KeyCode::Tab, &mut s, &summary_with(&[])),
            Loop::Continue
        );
    }

    #[test]
    fn movement_keys_noop_in_stream_mode() {
        let mut s = ViewState::default(); // Stream
        let sum = summary_with(&["a"]);
        assert_eq!(handle_key(KeyCode::Down, &mut s, &sum), Loop::Continue);
        assert_eq!(s.selected, 0);
        assert_eq!(handle_key(KeyCode::Up, &mut s, &sum), Loop::Continue);
        assert_eq!(s.selected, 0);
        assert_eq!(handle_key(KeyCode::Enter, &mut s, &sum), Loop::Continue);
        assert!(s.filter.is_none());
    }

    #[test]
    fn tab_resets_selected_to_zero() {
        let mut s = ViewState {
            view: View::Explorer,
            selected: 2,
            filter: None,
        };
        handle_key(KeyCode::Tab, &mut s, &summary_with(&[]));
        assert_eq!(s.selected, 0, "toggle must reset selected");
    }

    #[test]
    fn enter_in_explorer_with_no_rollups_is_noop() {
        let mut s = ViewState {
            view: View::Explorer,
            selected: 0,
            filter: None,
        };
        let sum = summary_with(&[]); // no rollups
        assert_eq!(handle_key(KeyCode::Enter, &mut s, &sum), Loop::Continue);
        assert!(s.filter.is_none());
        assert!(matches!(s.view, View::Explorer));
    }

    #[test]
    fn right_and_left_also_toggle_view() {
        let mut s = ViewState::default(); // Stream
        handle_key(KeyCode::Right, &mut s, &summary_with(&[]));
        assert!(matches!(s.view, View::Explorer));
        handle_key(KeyCode::Left, &mut s, &summary_with(&[]));
        assert!(matches!(s.view, View::Stream));
    }

    // ── Task 2: line helpers ─────────────────────────────────────────────────

    #[test]
    fn truncate_line_keeps_first_n_columns_across_spans() {
        let line = Line::from(vec![
            Span::styled("abc", Style::default().fg(GREEN)),
            Span::styled("defg", Style::default().fg(MUTED)),
        ]);
        assert_eq!(line_text(&truncate_line(line.clone(), 0)), "");

        // Truncate to 2: "ab" keeps first span partial with GREEN style
        let t2 = truncate_line(line.clone(), 2);
        assert_eq!(line_text(&t2), "ab");
        assert_eq!(
            t2.spans[0].style.fg,
            Some(GREEN),
            "partial first span must retain GREEN"
        );

        // Truncate to 3: exact boundary of first span, keeps "abc" whole with GREEN
        let t3 = truncate_line(line.clone(), 3);
        assert_eq!(line_text(&t3), "abc");
        assert_eq!(t3.spans.len(), 1, "exact boundary: only first span kept");
        assert_eq!(t3.spans[0].style.fg, Some(GREEN), "first span keeps GREEN");

        // Truncate to 5: "abcde" splits second span, keeps MUTED on split
        let t5 = truncate_line(line.clone(), 5);
        assert_eq!(line_text(&t5), "abcde");
        assert_eq!(t5.spans.len(), 2, "split second span means two spans");
        assert_eq!(t5.spans[0].style.fg, Some(GREEN), "first span keeps GREEN");
        assert_eq!(
            t5.spans[1].style.fg,
            Some(MUTED),
            "split second span keeps MUTED"
        );

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

    // ── Idle-arrival wipe fix: testable cascade (Task 5 review fix) ──────────

    #[test]
    fn idle_arrival_anchors_at_now_so_wipe_plays() {
        // Caught up with nothing at t=0.
        let mut c = Cascade {
            shown: 0,
            anchor_ms: 0,
            anchor_base: 0,
        };
        let mut rev: Vec<Option<u64>> = Vec::new();
        advance_cascade(&mut c, 0, 0, &mut rev);
        assert_eq!(c.shown, 0);
        // One entry arrives; not seen until a full idle poll later (t=250).
        advance_cascade(&mut c, 1, 250, &mut rev);
        assert_eq!(c.shown, 1);
        assert_eq!(rev[0], Some(250));
        let age = 250u64.saturating_sub(rev[0].unwrap_or(0));
        assert_eq!(age, 0);
        assert!(
            entrance_reveal(age, ENTER_MS, 80).is_some(),
            "wipe must still render"
        );
    }

    #[test]
    fn burst_staggers_one_row_per_step() {
        let mut c = Cascade {
            shown: 0,
            anchor_ms: 0,
            anchor_base: 0,
        };
        let mut rev: Vec<Option<u64>> = Vec::new();
        advance_cascade(&mut c, 3, 0, &mut rev); // 3 arrive at t=0
        assert_eq!(c.shown, 1); // one released immediately
        assert_eq!(rev, vec![Some(0)]);
        advance_cascade(&mut c, 3, STEP_MS, &mut rev);
        assert_eq!(c.shown, 2);
        assert_eq!(rev[1], Some(STEP_MS)); // staggered by one step
        advance_cascade(&mut c, 3, 2 * STEP_MS, &mut rev);
        assert_eq!(c.shown, 3);
        assert_eq!(rev[2], Some(2 * STEP_MS));
    }

    #[test]
    fn caught_up_idle_releases_nothing() {
        let mut c = Cascade {
            shown: 2,
            anchor_ms: 0,
            anchor_base: 2,
        };
        let mut rev: Vec<Option<u64>> = vec![Some(0), Some(STEP_MS)];
        advance_cascade(&mut c, 2, 9_999, &mut rev);
        assert_eq!(c.shown, 2, "no spurious release when caught up");
        assert_eq!(rev.len(), 2);
    }

    #[test]
    fn prime_backlog_marks_history_settled_and_not_animating() {
        // 5 entries already in the log at launch.
        let mut c = Cascade {
            shown: 0,
            anchor_ms: 0,
            anchor_base: 0,
        };
        let mut rev: Vec<Option<u64>> = Vec::new();
        prime_backlog(&mut rev, 5, &mut c);
        assert_eq!(c.shown, 5, "backlog is shown immediately");
        assert_eq!(c.anchor_base, 5, "anchored past the backlog");
        assert_eq!(rev.len(), 5);
        // Every backlog row is settled — None reveal time, so no wipe-in plays.
        assert!(
            rev.iter().all(|r| r.is_none()),
            "backlog rows must not animate"
        );
    }

    #[test]
    fn new_arrival_after_backlog_still_cascades() {
        // Backlog of 2 primed at launch.
        let mut c = Cascade {
            shown: 0,
            anchor_ms: 0,
            anchor_base: 0,
        };
        let mut rev: Vec<Option<u64>> = Vec::new();
        prime_backlog(&mut rev, 2, &mut c);
        assert_eq!(c.shown, 2);
        assert!(rev.iter().all(|r| r.is_none()));
        // A genuinely new row arrives at t=250 (after an idle poll): it animates.
        advance_cascade(&mut c, 3, 250, &mut rev);
        assert_eq!(c.shown, 3);
        assert_eq!(
            rev[2],
            Some(250),
            "new row is anchored at arrival (Some), not None"
        );
    }

    #[test]
    fn pad_line_noop_when_already_at_width() {
        // Build a 6-char line, pad to 6: exact boundary must be a true no-op
        let line = Line::from(Span::raw("abcdef"));
        let spans_before = line.spans.len();
        let padded = pad_line(line, 6, Style::default().bg(RED_REST));
        assert_eq!(line_text(&padded), "abcdef");
        assert_eq!(
            padded.spans.len(),
            spans_before,
            "width == current width: no extra span appended"
        );

        // Also verify that padding a line shorter than the requested width works
        let line_short = Line::from(Span::raw("abc"));
        let padded_short = pad_line(line_short, 6, Style::default().bg(RED_REST));
        assert_eq!(line_text(&padded_short), "abc   ");
        // the padding span carries the fill style
        let last = padded_short.spans.last().unwrap();
        assert_eq!(last.style.bg, Some(RED_REST));
    }
}
