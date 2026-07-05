//! Pure aggregation + formatting for `ironlint watch`. No I/O, no terminal.
//!
//! `summarize` folds the telemetry log (+ the configured check list) into a
//! `LogSummary` the TUI renders. This is the single definition of the watch
//! numbers (spec §6) — `ironlint review` should consume it too so they agree.

use crate::config::Lifecycle;
use crate::telemetry::LogEntry;
use crate::verdict::Status;
use std::collections::HashMap;

/// A configured check projected to what the summary needs (name + lifecycle).
/// Built by the CLI from `IronLintEngine::checks()`; keeps core free of the
/// full `Check`/`BTreeMap` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmedCheck {
    pub name: String,
    pub on: Vec<Lifecycle>,
}

/// Per-check rollup across the whole log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRollup {
    pub name: String,
    pub on: Vec<Lifecycle>,
    pub runs: usize,
    pub blocks: usize,
    pub internal: usize,
    pub p50_ms: Option<u64>,
}

impl CheckRollup {
    /// Block rate in [0,1]; `0.0` when the check never ran.
    #[allow(clippy::cast_precision_loss)]
    pub fn rate(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            self.blocks as f64 / self.runs as f64
        }
    }
}

/// Whole-log aggregate the TUI renders from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSummary {
    pub runs: usize,
    pub blocks: usize,
    pub internal: usize,
    pub pass: usize,
    pub rollups: Vec<CheckRollup>,
}

impl LogSummary {
    /// Entry-level pass percent, rounded. `None` on an empty log (avoids a
    /// misleading "100%").
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    pub fn pass_pct(&self) -> Option<u32> {
        if self.runs == 0 {
            None
        } else {
            Some(((self.pass as f64 / self.runs as f64) * 100.0).round() as u32)
        }
    }
}

#[derive(Default)]
struct Acc {
    runs: usize,
    blocks: usize,
    internal: usize,
    elapsed: Vec<u64>,
}

/// Lower-median of an already-sorted slice. `None` when empty.
fn median(sorted: &[u64]) -> Option<u64> {
    if sorted.is_empty() {
        None
    } else {
        Some(sorted[(sorted.len() - 1) / 2])
    }
}

/// Entry-level totals + per-check accumulators.
fn accumulate(entries: &[LogEntry]) -> (LogSummary, HashMap<String, Acc>) {
    let mut totals = LogSummary {
        runs: 0,
        blocks: 0,
        internal: 0,
        pass: 0,
        rollups: Vec::new(),
    };
    let mut per: HashMap<String, Acc> = HashMap::new();
    for entry in entries {
        let LogEntry::Check { status, checks, .. } = entry;
        totals.runs += 1;
        match status {
            Status::Pass => totals.pass += 1,
            Status::Block => totals.blocks += 1,
            Status::InternalError => totals.internal += 1,
        }
        for c in checks {
            let a = per.entry(c.check.clone()).or_default();
            a.runs += 1;
            a.elapsed.push(c.elapsed_ms);
            match c.status {
                Status::Pass => {}
                Status::Block => a.blocks += 1,
                Status::InternalError => a.internal += 1,
            }
        }
    }
    (totals, per)
}

/// Build the ranked rollup list from armed checks unioned with seen checks.
fn build_rollups(armed: &[ArmedCheck], per: &HashMap<String, Acc>) -> Vec<CheckRollup> {
    let mut names: Vec<String> = armed.iter().map(|a| a.name.clone()).collect();
    for k in per.keys() {
        if !names.contains(k) {
            names.push(k.clone());
        }
    }
    let on_for = |name: &str| {
        armed
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.on.clone())
            .unwrap_or_default()
    };
    let mut rollups: Vec<CheckRollup> = names
        .into_iter()
        .map(|name| {
            let (runs, blocks, internal, p50) = match per.get(&name) {
                Some(a) => {
                    let mut el = a.elapsed.clone();
                    el.sort_unstable();
                    (a.runs, a.blocks, a.internal, median(&el))
                }
                None => (0, 0, 0, None),
            };
            CheckRollup {
                on: on_for(&name),
                name,
                runs,
                blocks,
                internal,
                p50_ms: p50,
            }
        })
        .collect();
    rollups.sort_by(|a, b| {
        b.blocks
            .cmp(&a.blocks)
            .then_with(|| {
                b.rate()
                    .partial_cmp(&a.rate())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    rollups
}

/// Fold the log + armed checks into a `LogSummary` (spec §6).
pub fn summarize(entries: &[LogEntry], armed: &[ArmedCheck]) -> LogSummary {
    let (mut summary, per) = accumulate(entries);
    summary.rollups = build_rollups(armed, &per);
    summary
}

/// Human elapsed: `12ms` / `1.2s` / `1m30s`.
#[allow(clippy::cast_precision_loss)]
pub fn fmt_elapsed(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

/// `HH:MM:SS` from the log's RFC3339 `ts`, in the timestamp's own offset
/// (UTC, as written). Falls back to the first 8 chars if unparseable.
pub fn short_time(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| ts.chars().take(8).collect())
}

/// `[w]` / `[c]` / `[w+c]` lifecycle badge (spec §7).
pub fn lifecycle_badge(on: &[Lifecycle]) -> String {
    let w = on.contains(&Lifecycle::Write);
    let c = on.contains(&Lifecycle::PreCommit);
    match (w, c) {
        (true, true) => "[w+c]".into(),
        (true, false) => "[w]".into(),
        (false, true) => "[c]".into(),
        (false, false) => "[]".into(),
    }
}

/// `✓` pass / `✗` block / `⚠` internal error.
pub fn status_glyph(status: Status) -> char {
    match status {
        Status::Pass => '✓',
        Status::Block => '✗',
        Status::InternalError => '⚠',
    }
}

/// Cells of a row's width to reveal for the wipe-in entrance, stepped by
/// the caller's frame cadence.
///
/// Returns `None` once the row is fully revealed (`elapsed_ms >= enter_ms`)
/// or when `enter_ms == 0` (no animation).
pub fn entrance_reveal(elapsed_ms: u64, enter_ms: u64, full_cells: u16) -> Option<u16> {
    if enter_ms == 0 || elapsed_ms >= enter_ms {
        return None;
    }
    let cells = (u64::from(full_cells) * elapsed_ms) / enter_ms;
    Some(u16::try_from(cells).unwrap_or(full_cells))
}

/// How many queued entries have been released for display by `elapsed_ms`,
/// at a `step_ms` cadence: one immediately, then one per step, clamped to `queued`.
///
/// `step_ms == 0` releases the whole queue at once.
pub fn cascade_released(elapsed_ms: u64, step_ms: u64, queued: usize) -> usize {
    if step_ms == 0 {
        return queued;
    }
    let steps = usize::try_from(elapsed_ms / step_ms).unwrap_or(usize::MAX);
    steps.saturating_add(1).min(queued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::PerCheckRecord;

    fn check(file: &str, event: &str, status: Status, records: Vec<PerCheckRecord>) -> LogEntry {
        LogEntry::Check {
            ts: "2026-06-28T14:00:00+00:00".into(),
            file: Some(file.into()),
            set_size: None,
            event: event.into(),
            status,
            elapsed_ms: 10,
            checks: records,
        }
    }
    fn rec(name: &str, status: Status, ms: u64) -> PerCheckRecord {
        PerCheckRecord {
            check: name.into(),
            step: None,
            status,
            elapsed_ms: ms,
            reason: None,
        }
    }

    #[test]
    fn empty_log_is_all_zero_and_pass_pct_none() {
        let s = summarize(&[], &[]);
        assert_eq!((s.runs, s.blocks, s.internal, s.pass), (0, 0, 0, 0));
        assert_eq!(s.pass_pct(), None);
        assert!(s.rollups.is_empty());
    }

    #[test]
    fn counts_entries_by_status_and_pass_pct_rounds() {
        // 3 pass, 1 block => 4 runs, 75% pass
        let entries = vec![
            check(
                "a.ts",
                "write",
                Status::Pass,
                vec![rec("lint", Status::Pass, 5)],
            ),
            check(
                "b.ts",
                "write",
                Status::Pass,
                vec![rec("lint", Status::Pass, 5)],
            ),
            check(
                "c.ts",
                "write",
                Status::Pass,
                vec![rec("lint", Status::Pass, 5)],
            ),
            check(
                "d.ts",
                "write",
                Status::Block,
                vec![rec("lint", Status::Block, 5)],
            ),
        ];
        let s = summarize(&entries, &[]);
        assert_eq!((s.runs, s.blocks, s.internal, s.pass), (4, 1, 0, 3));
        assert_eq!(s.pass_pct(), Some(75));
    }

    #[test]
    fn per_check_rollup_rate_and_p50() {
        let entries = vec![
            check(
                "a",
                "write",
                Status::Block,
                vec![rec("nft", Status::Block, 10)],
            ),
            check(
                "b",
                "write",
                Status::Pass,
                vec![rec("nft", Status::Pass, 20)],
            ),
            check(
                "c",
                "write",
                Status::Pass,
                vec![rec("nft", Status::Pass, 30)],
            ),
        ];
        let s = summarize(&entries, &[]);
        let r = s.rollups.iter().find(|r| r.name == "nft").unwrap();
        assert_eq!(r.runs, 3);
        assert_eq!(r.blocks, 1);
        assert!((r.rate() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(r.p50_ms, Some(20)); // sorted [10,20,30], lower-median index 1
    }

    #[test]
    fn p50_lower_median_on_even_counts() {
        let entries = vec![
            check("a", "write", Status::Pass, vec![rec("x", Status::Pass, 10)]),
            check("b", "write", Status::Pass, vec![rec("x", Status::Pass, 40)]),
        ];
        let s = summarize(&entries, &[]);
        let r = s.rollups.iter().find(|r| r.name == "x").unwrap();
        assert_eq!(r.p50_ms, Some(10)); // [10,40] lower-median = 10
    }

    #[test]
    fn internal_errors_counted_per_check_and_overall() {
        let entries = vec![check(
            "a",
            "write",
            Status::InternalError,
            vec![rec("types", Status::InternalError, 240)],
        )];
        let s = summarize(&entries, &[]);
        assert_eq!(s.internal, 1);
        let r = s.rollups.iter().find(|r| r.name == "types").unwrap();
        assert_eq!(r.internal, 1);
    }

    #[test]
    fn armed_checks_with_zero_runs_appear_with_lifecycle() {
        let armed = vec![ArmedCheck {
            name: "unused".into(),
            on: vec![Lifecycle::Write],
        }];
        let s = summarize(&[], &armed);
        let r = s.rollups.iter().find(|r| r.name == "unused").unwrap();
        assert_eq!((r.runs, r.blocks), (0, 0));
        assert_eq!(r.p50_ms, None);
        assert_eq!(r.on, vec![Lifecycle::Write]);
    }

    #[test]
    fn ranking_is_blocks_then_rate_then_name() {
        // many: 1 block / 10 runs (rate .1); few: 1 block / 2 runs (rate .5); zero: 0 blocks
        let mut entries = vec![check(
            "x",
            "write",
            Status::Block,
            vec![rec("many", Status::Block, 1)],
        )];
        for _ in 0..9 {
            entries.push(check(
                "x",
                "write",
                Status::Pass,
                vec![rec("many", Status::Pass, 1)],
            ));
        }
        entries.push(check(
            "y",
            "write",
            Status::Block,
            vec![rec("few", Status::Block, 1)],
        ));
        entries.push(check(
            "y",
            "write",
            Status::Pass,
            vec![rec("few", Status::Pass, 1)],
        ));
        entries.push(check(
            "z",
            "write",
            Status::Pass,
            vec![rec("zero", Status::Pass, 1)],
        ));
        let s = summarize(&entries, &[]);
        let names: Vec<&str> = s.rollups.iter().map(|r| r.name.as_str()).collect();
        // both blockers (tie on 1 block) ordered by rate desc: few (.5) before many (.1); zero last
        assert_eq!(names, vec!["few", "many", "zero"]);
    }

    #[test]
    fn fmt_elapsed_boundaries() {
        assert_eq!(fmt_elapsed(12), "12ms");
        assert_eq!(fmt_elapsed(999), "999ms");
        assert_eq!(fmt_elapsed(1000), "1.0s");
        assert_eq!(fmt_elapsed(1200), "1.2s");
        assert_eq!(fmt_elapsed(30_000), "30.0s");
        assert_eq!(fmt_elapsed(90_500), "1m30s");
    }

    #[test]
    fn short_time_renders_hms_from_rfc3339() {
        assert_eq!(short_time("2026-06-28T14:23:09.5+00:00"), "14:23:09");
    }

    #[test]
    fn short_time_falls_back_on_garbage() {
        assert_eq!(short_time("nope"), "nope");
    }

    #[test]
    fn lifecycle_badge_variants() {
        assert_eq!(lifecycle_badge(&[Lifecycle::Write]), "[w]");
        assert_eq!(lifecycle_badge(&[Lifecycle::PreCommit]), "[c]");
        assert_eq!(
            lifecycle_badge(&[Lifecycle::Write, Lifecycle::PreCommit]),
            "[w+c]"
        );
        assert_eq!(lifecycle_badge(&[]), "[]");
    }

    #[test]
    fn status_glyph_per_status() {
        assert_eq!(status_glyph(Status::Pass), '✓');
        assert_eq!(status_glyph(Status::Block), '✗');
        assert_eq!(status_glyph(Status::InternalError), '⚠');
    }

    #[test]
    fn rate_returns_zero_on_zero_runs() {
        let r = CheckRollup {
            name: "x".into(),
            on: vec![],
            runs: 0,
            blocks: 0,
            internal: 0,
            p50_ms: None,
        };
        // rate() returns exactly 0.0 for zero runs (literal branch); epsilon check avoids float_cmp lint.
        assert!(r.rate() < f64::EPSILON);
    }

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
}
