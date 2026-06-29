//! `hector watch` — a read-only live TUI over `.hector/log.jsonl`.
//!
//! All decision logic (aggregation in core, plus `handle_key`/`stream_lines`/
//! `explorer_lines`/`ui` here) is pure and tested; the only uncovered code is
//! the terminal setup (`run_tui`) and event loop (`event_loop`), kept minimal.
use anyhow::Result;
use hector_core::watch::LogSummary;
use ratatui::crossterm::event::KeyCode;
use std::io::IsTerminal;
use std::path::Path;

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
    use hector_core::watch::{CheckRollup, LogSummary};

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
}
