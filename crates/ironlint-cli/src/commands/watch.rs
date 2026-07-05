//! `ironlint watch` — a read-only live TUI over `.ironlint/log.jsonl`.
//!
//! All decision logic (aggregation in core, plus `handle_key`/`stream_lines`/
//! `explorer_lines`/`ui` here) is pure and tested; the only uncovered code is
//! the terminal setup (`run_tui`) and event loop (`event_loop`), kept minimal.
use anyhow::Result;
use ironlint_core::runner::IronLintEngine;
use ironlint_core::telemetry::LogEntry;
use ironlint_core::verdict::Status;
use ironlint_core::watch::{
    entrance_reveal, fmt_elapsed, lifecycle_badge, short_time, status_glyph, summarize, ArmedCheck,
    CheckRollup, LogSummary,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use ratatui::Terminal;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

// ── Color vocabulary ──────────────────────────────────────────────────────────

const ORANGE: Color = Color::Rgb(255, 92, 56);
const GREEN: Color = Color::Rgb(52, 211, 153);
const AMBER: Color = Color::Rgb(245, 191, 79);
const MUTED: Color = Color::Rgb(132, 132, 140);

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
        line.spans
            .push(Span::styled(" ".repeat(target - cur), style));
    }
    line
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Block => ORANGE,
        Status::InternalError => AMBER,
        _ => GREEN, // Pass, and any future #[non_exhaustive] variant, default green
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

/// A stream entry plus its entrance state. `age_ms` is ms since the row was
/// released for display (`None` once settled); the event loop supplies it.
pub struct StreamRow<'a> {
    pub entry: &'a LogEntry,
    pub age_ms: Option<u64>,
}

/// Build the styled main row for one entry (time · glyph · target · elapsed · badge).
fn main_row_line(entry: &LogEntry) -> Line<'static> {
    let LogEntry::Check {
        ts,
        file,
        set_size,
        event,
        status,
        elapsed_ms,
        ..
    } = entry;
    let glyph = status_glyph(*status);
    let target = target_label(file.as_ref(), *set_size);
    let badge = if event == "pre-commit" {
        "commit"
    } else {
        "write"
    };
    Line::from(vec![
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
        let LogEntry::Check {
            event,
            status,
            checks,
            ..
        } = row.entry;
        if let Some(f) = filter {
            if !checks.iter().any(|c| c.check == f) {
                continue;
            }
        }
        let reveal = row.age_ms.and_then(|a| entrance_reveal(a, ENTER_MS, width));
        push_row(
            &mut lines,
            main_row_line(row.entry),
            matches!(status, Status::Block),
            reveal,
            width,
        );
        for c in checks {
            if let Some(text) = detail_text(&c.check, c.status, c.reason.as_deref(), event) {
                let d = Line::from(Span::styled(
                    text,
                    Style::default().fg(status_color(c.status)),
                ));
                push_row(
                    &mut lines,
                    d,
                    matches!(c.status, Status::Block),
                    reveal,
                    width,
                );
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

fn pass_pct_text(summary: &LogSummary) -> String {
    summary
        .pass_pct()
        .map(|p| format!("{p}% pass"))
        .unwrap_or_else(|| "— pass".into())
}

fn rollup_line(r: &CheckRollup, selected: bool) -> Line<'static> {
    let dot_color = if r.blocks > 0 { ORANGE } else { GREEN };
    // rate() returns blocks/runs ∈ [0.0, 1.0] — product is non-negative.
    #[allow(clippy::cast_sign_loss)]
    let rate = (r.rate() * 100.0).round() as u32;
    let p50 = r.p50_ms.map(fmt_elapsed).unwrap_or_else(|| "—".into());
    let warn = if r.internal > 0 {
        format!("  ⚠ {}", r.internal)
    } else {
        String::new()
    };
    let marker = if selected { "› " } else { "  " };
    let name_style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::raw(marker),
        Span::styled("● ", Style::default().fg(dot_color)),
        Span::styled(format!("{:<20}", r.name), name_style),
        Span::styled(
            format!("{} ", lifecycle_badge(&r.on)),
            Style::default().fg(MUTED),
        ),
        Span::styled(warn, Style::default().fg(AMBER)),
        Span::raw(format!("  {:>3}", r.blocks)),
        Span::styled(format!("  {:>4}%", rate), Style::default().fg(MUTED)),
        Span::styled(format!("  {:>6}", p50), Style::default().fg(MUTED)),
    ])
}

/// Explorer view: summary bar + ranked per-check table with a divider before
/// the zero-block checks (spec §5.2).
pub fn explorer_lines(summary: &LogSummary, selected: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "log {} runs · {} blocks · {} internal · {}",
            summary.runs,
            summary.blocks,
            summary.internal,
            pass_pct_text(summary),
        ),
        Style::default().fg(MUTED),
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "CHECKS · RANKED BY BLOCKS                              blocks  rate     p50",
        Style::default().fg(MUTED),
    )));

    let mut divider_emitted = false;
    for (i, r) in summary.rollups.iter().enumerate() {
        if r.blocks == 0 && !divider_emitted {
            lines.push(Line::from(Span::styled(
                "✓ NO BLOCKS IN LOG",
                Style::default().fg(GREEN),
            )));
            divider_emitted = true;
        }
        lines.push(rollup_line(r, i == selected));
    }
    lines
}

fn header_line(state: &ViewState, summary: &LogSummary, clock: &str) -> Line<'static> {
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
    let status_word = if summary.blocks > 0 { "BLOCK" } else { "PASS" };
    Line::from(vec![
        Span::styled("≈ stream", stream_style),
        Span::raw("   "),
        Span::styled("▤ explorer", explorer_style),
        Span::raw("        "),
        Span::styled(
            format!("{status_word} · {clock}"),
            Style::default().fg(MUTED),
        ),
    ])
}

fn footer_line(state: &ViewState, summary: &LogSummary, armed: usize) -> Line<'static> {
    match state.view {
        View::Stream => Line::from(Span::styled(
            format!(
                "{armed} checks armed · {} runs · {} blocks        → explorer   q quit",
                summary.runs, summary.blocks
            ),
            Style::default().fg(MUTED),
        )),
        View::Explorer => Line::from(Span::styled(
            "↑↓ select   ↵ open check log        → stream   q quit".to_string(),
            Style::default().fg(MUTED),
        )),
    }
}

/// Draw the full TUI for the current state.
pub fn ui(
    frame: &mut Frame,
    entries: &[LogEntry],
    summary: &LogSummary,
    armed: usize,
    state: &ViewState,
    clock: &str,
    config_loaded: bool,
) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    frame.render_widget(
        Paragraph::new(header_line(state, summary, clock)),
        chunks[0],
    );

    let mut body = if !config_loaded && matches!(state.view, View::Stream) && entries.is_empty() {
        // Degraded + cold: show only the banner (no empty box beneath it).
        vec![]
    } else if config_loaded && matches!(state.view, View::Stream) && entries.is_empty() {
        // Cold-start hint: no entries yet but config is fine.
        vec![Line::from(Span::styled(
            "waiting for edits\u{2026}",
            Style::default().fg(MUTED),
        ))]
    } else {
        match state.view {
            View::Stream => {
                let rows: Vec<StreamRow> = entries
                    .iter()
                    .map(|entry| StreamRow {
                        entry,
                        age_ms: None,
                    })
                    .collect();
                stream_lines(&rows, state.filter.as_deref(), chunks[1].width)
            }
            View::Explorer => explorer_lines(summary, state.selected),
        }
    };

    // Degraded-config banner at the top of the body area.
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

fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    dir: &Path,
    armed: &[ArmedCheck],
    config_loaded: bool,
) -> Result<()> {
    let log = dir.join(".ironlint/log.jsonl");
    let mut state = ViewState::default();
    // Incremental tail: accumulate entries across ticks, reading only the bytes
    // appended since the last tick instead of re-parsing the whole log each time.
    let mut entries: Vec<LogEntry> = Vec::new();
    let mut offset: u64 = 0;
    loop {
        // On a transient read error, keep the prior view (entries/offset unchanged).
        if let Ok((new, reset)) = ironlint_core::telemetry::read_since(&log, &mut offset) {
            if reset {
                entries.clear();
            }
            entries.extend(new);
        }
        let summary = summarize(&entries, armed);
        let clock = short_time(&chrono::Utc::now().to_rfc3339());
        terminal.draw(|f| {
            ui(
                f,
                &entries,
                &summary,
                armed.len(),
                &state,
                &clock,
                config_loaded,
            );
        })?;
        if event::poll(Duration::from_millis(250))? {
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

/// Active pane.
pub enum View {
    Stream,
    Explorer,
}

/// What the loop should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Loop {
    Continue,
    Quit,
}

/// UI state threaded through the loop. Pure data.
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

fn toggle(view: &View) -> View {
    match view {
        View::Stream => View::Explorer,
        View::Explorer => View::Stream,
    }
}

/// Map a key to a state mutation (spec §5.3). Pure; no I/O.
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
            line_text(row).chars().count() as u16 >= W,
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
        term.draw(|f| ui(f, &entries, &summary, 7, &state, "14:24:00", true))
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
        term.draw(|f| ui(f, &[], &summary, 7, &state, "14:24:09", true))
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
        term.draw(|f| ui(f, &[], &summary, 0, &state, "14:00:00", false))
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
        term.draw(|f| ui(f, &[], &summary, 0, &state, "14:00:00", true))
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
