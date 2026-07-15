use ironlint_core::watch::LogSummary;
use ratatui::crossterm::event::KeyCode;

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
