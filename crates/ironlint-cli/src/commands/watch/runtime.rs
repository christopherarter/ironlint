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
pub(super) const ACTIVE_POLL_MS: u64 = 40;
/// Poll interval when nothing is animating — a quiet watcher stays near-zero CPU.
pub(super) const IDLE_POLL_MS: u64 = 250;

pub(super) trait RuntimeIo {
    fn read_since(&mut self, log: &Path, offset: &mut u64)
        -> anyhow::Result<(Vec<LogEntry>, bool)>;
    fn now_ms(&mut self) -> u64;
    fn poll(&mut self, wait: Duration) -> std::io::Result<bool>;
    fn read(&mut self) -> std::io::Result<Event>;
}

struct CrosstermRuntimeIo {
    loop_start: Instant,
}

impl RuntimeIo for CrosstermRuntimeIo {
    fn read_since(
        &mut self,
        log: &Path,
        offset: &mut u64,
    ) -> anyhow::Result<(Vec<LogEntry>, bool)> {
        ironlint_core::telemetry::read_since(log, offset)
    }

    fn now_ms(&mut self) -> u64 {
        millis(self.loop_start.elapsed())
    }

    fn poll(&mut self, wait: Duration) -> std::io::Result<bool> {
        event::poll(wait)
    }

    fn read(&mut self) -> std::io::Result<Event> {
        event::read()
    }
}

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
    let mut io = CrosstermRuntimeIo {
        loop_start: Instant::now(),
    };
    let result = event_loop(&mut terminal, dir, armed, config_loaded, &mut io);
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

struct LoopState {
    state: ViewState,
    entries: Vec<LogEntry>,
    offset: u64,
    primed: bool,
    cascade: Cascade,
    revealed_ms: Vec<Option<u64>>,
}

impl Default for LoopState {
    fn default() -> Self {
        Self {
            state: ViewState::default(),
            entries: Vec::new(),
            offset: 0,
            primed: false,
            cascade: Cascade {
                shown: 0,
                anchor_base: 0,
                anchor_ms: 0,
            },
            revealed_ms: Vec::new(),
        }
    }
}

impl LoopState {
    fn refresh<I: RuntimeIo>(&mut self, io: &mut I, log: &Path, now_ms: u64) {
        if let Ok((new, reset)) = io.read_since(log, &mut self.offset) {
            if reset {
                self.reset_entries();
            }
            self.entries.extend(new);
            self.prime_if_needed();
        }

        advance_cascade(
            &mut self.cascade,
            self.entries.len(),
            now_ms,
            &mut self.revealed_ms,
        );
    }

    fn rows(&self, now_ms: u64) -> (Vec<StreamRow<'_>>, bool) {
        let mut animating = self.cascade.shown < self.entries.len();
        let rows = (0..self.cascade.shown)
            .map(|i| {
                let age_ms = self.row_age(i, now_ms, &mut animating);
                StreamRow {
                    entry: &self.entries[i],
                    age_ms,
                }
            })
            .collect();
        (rows, animating)
    }

    fn reset_entries(&mut self) {
        self.entries.clear();
        self.revealed_ms.clear();
        self.cascade.shown = 0;
        self.cascade.anchor_base = 0;
        self.primed = false;
    }

    fn prime_if_needed(&mut self) {
        if !self.primed {
            prime_backlog(&mut self.revealed_ms, self.entries.len(), &mut self.cascade);
            self.primed = true;
        }
    }

    fn row_age(&self, index: usize, now_ms: u64, animating: &mut bool) -> Option<u64> {
        let revealed_ms = self.revealed_ms[index]?;
        let age_ms = now_ms.saturating_sub(revealed_ms);
        if age_ms < ENTER_MS {
            *animating = true;
            Some(age_ms)
        } else {
            None
        }
    }
}

pub(super) fn event_loop<B: Backend, I: RuntimeIo>(
    terminal: &mut Terminal<B>,
    dir: &Path,
    armed: &[ArmedCheck],
    config_loaded: bool,
    io: &mut I,
) -> Result<()>
where
    // ratatui 0.30 gave `Backend` an associated `Error` type; bound it so
    // `terminal.draw(..)?` can convert into `anyhow::Error`. Both backends we
    // use (`CrosstermBackend`, `TestBackend`) surface `std::io::Error`.
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let log = dir.join(".ironlint/log.jsonl");
    let mut loop_state = LoopState::default();

    loop {
        let now_ms = io.now_ms();
        loop_state.refresh(io, &log, now_ms);
        let (rows, animating) = loop_state.rows(now_ms);
        let summary = summarize(&loop_state.entries[..loop_state.cascade.shown], armed);
        terminal.draw(|f| {
            ui(
                f,
                &rows,
                &summary,
                armed.len(),
                &loop_state.state,
                config_loaded,
            );
        })?;

        let poll = if animating {
            ACTIVE_POLL_MS
        } else {
            IDLE_POLL_MS
        };
        if io.poll(Duration::from_millis(poll))? {
            if let Event::Key(key) = io.read()? {
                if key.kind == KeyEventKind::Press
                    && handle_key(key.code, &mut loop_state.state, &summary) == Loop::Quit
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
