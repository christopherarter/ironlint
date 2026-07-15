use super::StreamRow;
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

mod render;
mod runtime;
mod state;
