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

use super::{handle_key, ui, Loop, StreamRow, ViewState};

/// Horizontal wipe-in duration in ms (spec §2, decision 5).
pub(super) const ENTER_MS: u64 = 210;

/// Cascade release cadence — one queued row shown per this interval (spec §2).
pub(super) const STEP_MS: u64 = 95;
/// Poll interval while an entrance is in flight (smooth-enough stepping).
const ACTIVE_POLL_MS: u64 = 40;
/// Poll interval when nothing is animating — a quiet watcher stays near-zero CPU.
const IDLE_POLL_MS: u64 = 250;

/// Resolve the armed-check projection from `<dir>/.ironlint.yml`. Best-effort:
/// returns `(checks, true)` on success, `([], false)` on any load error so the
/// caller can show a degraded-config banner while still tailing the log.
pub(super) fn load_armed(dir: &Path) -> (Vec<ArmedCheck>, bool) {
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
pub(super) struct Cascade {
    pub(super) shown: usize,
    pub(super) anchor_ms: u64,
    pub(super) anchor_base: usize,
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
pub(super) fn advance_cascade(
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
pub(super) fn prime_backlog(revealed_ms: &mut Vec<Option<u64>>, n: usize, c: &mut Cascade) {
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
