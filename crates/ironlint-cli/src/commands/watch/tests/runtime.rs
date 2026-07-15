use super::super::runtime::{
    advance_cascade, event_loop, load_armed, prime_backlog, Cascade, RuntimeIo, ACTIVE_POLL_MS,
    ENTER_MS, IDLE_POLL_MS, STEP_MS,
};
#[allow(unused_imports)]
use super::*;
use ironlint_core::watch::entrance_reveal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::time::Duration;

struct ScriptedRuntimeIo {
    reads: VecDeque<anyhow::Result<(Vec<LogEntry>, bool)>>,
    now_ms: VecDeque<u64>,
    polls: Vec<Duration>,
    events: VecDeque<Event>,
}

impl RuntimeIo for ScriptedRuntimeIo {
    fn read_since(&mut self, _: &Path, _: &mut u64) -> anyhow::Result<(Vec<LogEntry>, bool)> {
        self.reads.pop_front().expect("scripted telemetry read")
    }

    fn now_ms(&mut self) -> u64 {
        self.now_ms.pop_front().expect("scripted clock")
    }

    fn poll(&mut self, wait: Duration) -> io::Result<bool> {
        self.polls.push(wait);
        Ok(!self.events.is_empty())
    }

    fn read(&mut self) -> io::Result<Event> {
        Ok(self.events.pop_front().expect("scripted event"))
    }
}

fn scripted_io(
    reads: Vec<anyhow::Result<(Vec<LogEntry>, bool)>>,
    now_ms: Vec<u64>,
    events: Vec<Event>,
) -> ScriptedRuntimeIo {
    ScriptedRuntimeIo {
        reads: reads.into(),
        now_ms: now_ms.into(),
        polls: Vec::new(),
        events: events.into(),
    }
}

fn runtime_entry(file: &str) -> LogEntry {
    entry(Some(file), None, "write", Status::Pass, 12, vec![])
}

fn press(ch: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
}

fn release(ch: char) -> Event {
    Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char(ch),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ))
}

fn terminal_text(terminal: &Terminal<TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

struct OrderingRuntimeIo {
    calls: Vec<&'static str>,
}

impl RuntimeIo for OrderingRuntimeIo {
    fn read_since(&mut self, _: &Path, _: &mut u64) -> anyhow::Result<(Vec<LogEntry>, bool)> {
        self.calls.push("read_since");
        Ok((vec![], false))
    }

    fn now_ms(&mut self) -> u64 {
        assert_eq!(
            self.calls.last(),
            Some(&"read_since"),
            "sample the clock after completing the telemetry read"
        );
        self.calls.push("now_ms");
        0
    }

    fn poll(&mut self, _: Duration) -> io::Result<bool> {
        self.calls.push("poll");
        Ok(true)
    }

    fn read(&mut self) -> io::Result<Event> {
        self.calls.push("read_event");
        Ok(press('q'))
    }
}

#[test]
fn event_loop_samples_time_after_the_telemetry_read() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut io = OrderingRuntimeIo { calls: Vec::new() };

    event_loop(&mut terminal, Path::new("/project"), &[], true, &mut io).unwrap();

    assert_eq!(io.calls, ["read_since", "now_ms", "poll", "read_event"]);
}

#[test]
fn event_loop_primes_backlog_draws_and_quits_on_pressed_q() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut io = scripted_io(
        vec![Ok((vec![runtime_entry("src/backlog.rs")], false))],
        vec![0],
        vec![press('q')],
    );

    event_loop(&mut terminal, Path::new("/project"), &[], true, &mut io).unwrap();

    assert_eq!(io.polls, vec![Duration::from_millis(IDLE_POLL_MS)]);
}

#[test]
fn event_loop_uses_active_then_idle_poll_for_a_live_entry() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut io = scripted_io(
        vec![
            Ok((vec![], false)),
            Ok((vec![runtime_entry("src/live.rs")], false)),
            Ok((vec![], false)),
        ],
        vec![0, 250, 250 + ENTER_MS],
        vec![Event::Resize(100, 20), release('q'), press('q')],
    );

    event_loop(&mut terminal, Path::new("/project"), &[], true, &mut io).unwrap();

    assert_eq!(
        io.polls,
        vec![
            Duration::from_millis(IDLE_POLL_MS),
            Duration::from_millis(ACTIVE_POLL_MS),
            Duration::from_millis(IDLE_POLL_MS),
        ]
    );
}

#[test]
fn event_loop_retries_after_read_error_and_reprimes_after_reset() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut io = scripted_io(
        vec![
            Err(anyhow::anyhow!("missing log")),
            Ok((vec![runtime_entry("src/backlog.rs")], false)),
            Ok((vec![runtime_entry("src/replacement.rs")], true)),
        ],
        vec![0, 250, 500],
        vec![Event::Resize(100, 20), release('q'), press('q')],
    );

    event_loop(&mut terminal, Path::new("/project"), &[], true, &mut io).unwrap();

    assert!(
        io.reads.is_empty(),
        "every scripted telemetry read must run"
    );
    let frame = terminal_text(&terminal);
    assert!(frame.contains("src/replacement.rs"));
    assert!(!frame.contains("src/backlog.rs"));
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
