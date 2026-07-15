use ironlint_core::telemetry::LogEntry;
use ironlint_core::verdict::Status;
use ironlint_core::watch::{
    entrance_reveal, fmt_elapsed, lifecycle_badge, short_time, status_glyph, CheckRollup,
    LogSummary,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::runtime::ENTER_MS;
use super::{View, ViewState};

// ── Color vocabulary ──────────────────────────────────────────────────────────

const ORANGE: Color = Color::Rgb(255, 92, 56);
pub(super) const GREEN: Color = Color::Rgb(52, 211, 153);
const AMBER: Color = Color::Rgb(245, 191, 79);
pub(super) const MUTED: Color = Color::Rgb(132, 132, 140);

/// Blocked-row background — a slight, standing dim red (spec §4.2).
pub(super) const RED_REST: Color = Color::Rgb(36, 16, 21);

/// Keep the first `cells` display columns of a line, across its spans. Stream
/// content is single-column, so a column is one `char`.
pub(super) fn truncate_line(line: Line<'static>, cells: u16) -> Line<'static> {
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
pub(super) fn pad_line(mut line: Line<'static>, width: u16, style: Style) -> Line<'static> {
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
                    // Deliberate per-check keying (not the entry's status): a
                    // blocking check's own detail line gets the red tint, but an
                    // InternalError detail line keeps its amber text untinted —
                    // amber-on-red would read wrong.
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

pub(super) fn header_line(state: &ViewState, summary: &LogSummary) -> Line<'static> {
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
    let (word, dot) = if summary.blocks > 0 {
        ("BLOCK", ORANGE)
    } else {
        ("PASS", GREEN)
    };
    Line::from(vec![
        Span::styled("≈ stream", stream_style),
        Span::raw("   "),
        Span::styled("▤ explorer", explorer_style),
        Span::raw("        "),
        Span::styled("● ", Style::default().fg(dot)),
        Span::styled(word, Style::default().fg(MUTED)),
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
        // Degraded + cold: show only the banner (no empty box beneath it).
        vec![]
    } else if config_loaded && matches!(state.view, View::Stream) && rows.is_empty() {
        // Cold-start hint: no entries yet but config is fine.
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
