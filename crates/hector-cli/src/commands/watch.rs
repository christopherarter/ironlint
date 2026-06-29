//! `hector watch` — a read-only live TUI over `.hector/log.jsonl`.
//!
//! All decision logic (aggregation in core, plus `handle_key`/`stream_lines`/
//! `explorer_lines`/`ui` here) is pure and tested; the only uncovered code is
//! the terminal setup (`run_tui`) and event loop (`event_loop`), kept minimal.
use anyhow::Result;
use hector_core::telemetry::LogEntry;
use hector_core::verdict::Status;
use hector_core::watch::{fmt_elapsed, short_time, status_glyph, LogSummary};
use ratatui::crossterm::event::KeyCode;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::io::IsTerminal;
use std::path::Path;

// ── Color vocabulary ──────────────────────────────────────────────────────────

const ORANGE: Color = Color::Rgb(255, 92, 56);
const GREEN: Color = Color::Rgb(52, 211, 153);
const AMBER: Color = Color::Rgb(245, 191, 79);
const MUTED: Color = Color::Rgb(132, 132, 140);

fn status_color(status: Status) -> Color {
    match status {
        Status::Pass => GREEN,
        Status::Block => ORANGE,
        Status::InternalError => AMBER,
        _ => MUTED,
    }
}

/// Target label: a file path, or `pre-commit · N files` for a set run.
fn target_label(file: Option<&String>, set_size: Option<usize>) -> String {
    match file {
        Some(f) => f.clone(),
        None => format!("pre-commit · {} files", set_size.unwrap_or(0)),
    }
}

/// Detail sub-line text for a non-pass per-check record. `event` decides the
/// block verb. Returns `None` for passing records.
fn detail_text(check: &str, status: Status, reason: Option<&str>, event: &str) -> Option<String> {
    match status {
        Status::Block => {
            let verb = if event == "pre-commit" {
                "commit blocked"
            } else {
                "write rejected"
            };
            Some(format!("  └ {check} · {verb}"))
        }
        Status::InternalError => Some(format!(
            "  └ {check} · check error: {}",
            reason.unwrap_or("unknown")
        )),
        _ => None,
    }
}

/// Newest-first stream lines. `filter` keeps only entries whose `checks`
/// contains that check name.
// Phase 4 event loop and ui() consume this; not dead.
#[allow(dead_code)]
pub fn stream_lines(entries: &[LogEntry], filter: Option<&str>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for entry in entries.iter().rev() {
        let LogEntry::Check {
            ts,
            file,
            set_size,
            event,
            status,
            elapsed_ms,
            checks,
        } = entry;
        if let Some(f) = filter {
            if !checks.iter().any(|c| c.check == f) {
                continue;
            }
        }
        let glyph = status_glyph(*status);
        let target = target_label(file.as_ref(), *set_size);
        let badge = if event == "pre-commit" {
            "commit"
        } else {
            "write"
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>8}  ", short_time(ts)),
                Style::default().fg(MUTED),
            ),
            Span::styled(
                format!("{glyph}  "),
                Style::default().fg(status_color(*status)),
            ),
            Span::raw(format!("{target:<40}")),
            Span::styled(
                format!("  {:>6}  ", fmt_elapsed(*elapsed_ms)),
                Style::default().fg(MUTED),
            ),
            Span::styled(badge.to_string(), Style::default().fg(MUTED)),
        ]));
        for c in checks {
            if let Some(text) = detail_text(&c.check, c.status, c.reason.as_deref(), event) {
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(status_color(c.status)),
                )));
            }
        }
    }
    lines
}

/// Entry point. Requires an interactive terminal; otherwise exits 1 with a hint.
pub fn run(dir: &Path) -> Result<i32> {
    if !std::io::stdout().is_terminal() {
        eprintln!("hector watch: requires an interactive terminal (no TTY detected).");
        eprintln!(
            "A non-interactive `--once` snapshot is planned; for now inspect {}/.hector/log.jsonl directly.",
            dir.display()
        );
        return Ok(1);
    }
    // Phase 4 replaces this stub with the live loop.
    Ok(0)
}

/// Active pane.
// Phase 3 rendering and Phase 4 event loop consume this; not dead.
#[allow(dead_code)]
pub enum View {
    Stream,
    Explorer,
}

/// What the loop should do after a key.
#[derive(Debug, PartialEq, Eq)]
// Phase 4 event loop consumes this; not dead.
#[allow(dead_code)]
pub enum Loop {
    Continue,
    Quit,
}

/// UI state threaded through the loop. Pure data.
// Phase 3 rendering and Phase 4 event loop consume this; not dead.
#[allow(dead_code)]
pub struct ViewState {
    pub view: View,
    pub selected: usize,
    pub filter: Option<String>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            view: View::Stream,
            selected: 0,
            filter: None,
        }
    }
}

// Called by handle_key; Phase 4 wires that into the event loop.
#[allow(dead_code)]
fn toggle(view: &View) -> View {
    match view {
        View::Stream => View::Explorer,
        View::Explorer => View::Stream,
    }
}

/// Map a key to a state mutation (spec §5.3). Pure; no I/O.
// Phase 4 event loop calls this; not dead.
#[allow(dead_code)]
pub fn handle_key(code: KeyCode, state: &mut ViewState, summary: &LogSummary) -> Loop {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return Loop::Quit,
        KeyCode::Tab | KeyCode::Right | KeyCode::Left => {
            state.view = toggle(&state.view);
            state.selected = 0;
        }
        KeyCode::Down if matches!(state.view, View::Explorer) => {
            let max = summary.rollups.len().saturating_sub(1);
            state.selected = (state.selected + 1).min(max);
        }
        KeyCode::Up if matches!(state.view, View::Explorer) => {
            state.selected = state.selected.saturating_sub(1);
        }
        KeyCode::Enter if matches!(state.view, View::Explorer) => {
            if let Some(r) = summary.rollups.get(state.selected) {
                state.filter = Some(r.name.clone());
                state.view = View::Stream;
            }
        }
        _ => {}
    }
    Loop::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use hector_core::config::Lifecycle;
    use hector_core::telemetry::{LogEntry, PerCheckRecord};
    use hector_core::verdict::Status;
    use hector_core::watch::{CheckRollup, CheckRollup as Roll, LogSummary};
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
        recs: Vec<PerCheckRecord>,
    ) -> LogEntry {
        LogEntry::Check {
            ts: "2026-06-28T14:23:09+00:00".into(),
            file: file.map(Into::into),
            set_size: set,
            event: event.into(),
            status,
            elapsed_ms: 12,
            checks: recs,
        }
    }
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
    #[allow(dead_code)]
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

    // ── Task 3.1: stream_lines ────────────────────────────────────────────────

    #[test]
    fn stream_renders_pass_row_with_time_file_elapsed_event() {
        let e = vec![entry(
            Some("src/auth.ts"),
            None,
            "write",
            Status::Pass,
            vec![prec("lint", Status::Pass, None)],
        )];
        let text = all_text(&stream_lines(&e, None));
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
            vec![prec("no-focused-tests", Status::Block, None)],
        )];
        let text = all_text(&stream_lines(&e, None));
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
            vec![prec("lint", Status::Pass, None)],
        )];
        let text = all_text(&stream_lines(&e, None));
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
            vec![prec("types-pass", Status::InternalError, Some("timeout"))],
        )];
        let text = all_text(&stream_lines(&e, None));
        assert!(text.contains("check error: timeout"));
    }

    #[test]
    fn stream_is_newest_first() {
        let mut a = entry(
            Some("old.ts"),
            None,
            "write",
            Status::Pass,
            vec![prec("lint", Status::Pass, None)],
        );
        let LogEntry::Check { ts, .. } = &mut a;
        *ts = "2026-06-28T14:00:00+00:00".into();
        let b = entry(
            Some("new.ts"),
            None,
            "write",
            Status::Pass,
            vec![prec("lint", Status::Pass, None)],
        );
        let lines = stream_lines(&[a, b], None);
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
                vec![prec("lint", Status::Pass, None)],
            ),
            entry(
                Some("b.ts"),
                None,
                "write",
                Status::Pass,
                vec![prec("types", Status::Pass, None)],
            ),
        ];
        let text = all_text(&stream_lines(&e, Some("types")));
        assert!(text.contains("b.ts"));
        assert!(!text.contains("a.ts"));
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
}
