//! Interactive harness multi-select for `ironlint init`.
//!
//! Pure render logic (`render_select`) and a pure update reducer (`update_select`)
//! are unit-tested with ratatui's `TestBackend`; the thin live event loop in
//! `prompt_multi_select` is intentionally untested TTY glue.
use anyhow::Result;
use std::io::Stdout;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Alignment, Constraint, Flex, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph};
use ratatui::Frame;
use ratatui::Terminal;

// Re-export types needed by callers and tests so they are part of the module
// public surface without requiring callers to import ratatui themselves.
pub use ratatui::crossterm::event::KeyEvent;
pub use ratatui::widgets::ListState;

#[derive(Debug, Clone)]
pub struct SelectItem {
    pub name: String,
    pub detected: bool,
    pub selected: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SelectOutcome {
    #[allow(dead_code)]
    Pending,
    Confirmed,
    Aborted,
}

pub fn prompt_multi_select(items: Vec<SelectItem>) -> Result<Vec<String>> {
    if items.is_empty() {
        return Ok(vec![]);
    }

    let mut guard = TerminalGuard::new()?;

    let mut state = ListState::default();
    state.select(Some(0));
    let mut current_items = items;

    loop {
        guard.0.draw(|f| render_select(f, &current_items, &state))?;
        let event = event::read()?;
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                let (new_items, new_state, outcome) = update_select(&current_items, key, &state);
                current_items = new_items;
                state = new_state;
                match outcome {
                    Some(SelectOutcome::Confirmed) => {
                        return Ok(current_items
                            .into_iter()
                            .filter(|i| i.selected)
                            .map(|i| i.name)
                            .collect());
                    }
                    Some(SelectOutcome::Aborted) => return Ok(vec![]),
                    _ => {}
                }
            }
        }
    }
}

struct TerminalGuard(Terminal<CrosstermBackend<Stdout>>);

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(e.into());
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self(terminal)),
            Err(e) => {
                let _ = execute!(std::io::stdout(), LeaveAlternateScreen, Show);
                let _ = disable_raw_mode();
                Err(e.into())
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(self.0.backend_mut(), LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
    }
}

fn update_select(
    items: &[SelectItem],
    key: KeyEvent,
    state: &ListState,
) -> (Vec<SelectItem>, ListState, Option<SelectOutcome>) {
    let mut new_items = items.to_vec();
    let mut new_state = *state;
    let no_mods = key.modifiers.is_empty();
    let outcome = match key.code {
        KeyCode::Up | KeyCode::Char('k') if no_mods => {
            move_cursor(-1, items.len(), &mut new_state);
            None
        }
        KeyCode::Down | KeyCode::Char('j') if no_mods => {
            move_cursor(1, items.len(), &mut new_state);
            None
        }
        KeyCode::Char(' ') if no_mods => {
            if let Some(idx) = state.selected() {
                if let Some(item) = new_items.get_mut(idx) {
                    item.selected = !item.selected;
                }
            }
            None
        }
        KeyCode::Enter => Some(SelectOutcome::Confirmed),
        KeyCode::Esc | KeyCode::Char('q') if no_mods => Some(SelectOutcome::Aborted),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(SelectOutcome::Aborted)
        }
        _ => None,
    };
    (new_items, new_state, outcome)
}

fn render_select(f: &mut Frame, items: &[SelectItem], state: &ListState) {
    f.render_widget(Clear, f.area());
    let area = f.area();

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .flex(Flex::Center)
    .split(area);

    let header = Paragraph::new(Line::from(vec![Span::styled(
        "ironlint \u{00b7} onboarding",
        Style::default().add_modifier(Modifier::BOLD),
    )]))
    .alignment(Alignment::Center);
    f.render_widget(header, chunks[0]);

    let help = Paragraph::new("Select harnesses to wire (space toggles, enter confirms):")
        .alignment(Alignment::Center);
    f.render_widget(help, chunks[1]);

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let prefix = if state.selected() == Some(i) {
                "> "
            } else {
                "  "
            };
            let marker = if item.selected { "[x]" } else { "[ ]" };
            let name_padded = format!("{:<12}", item.name);
            let base = format!("{}{} {}    ", prefix, marker, name_padded);
            let mut spans = vec![Span::raw(base)];
            if item.detected {
                spans.push(Span::styled(
                    "detected",
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(list_items).highlight_symbol("");
    let mut render_state = *state;
    f.render_stateful_widget(list, chunks[2], &mut render_state);

    let selected_count = items.iter().filter(|i| i.selected).count();
    let footer = Paragraph::new(format!(
        "\u{2500} {} of {} selected \u{2500}",
        selected_count,
        items.len()
    ))
    .alignment(Alignment::Center);
    f.render_widget(footer, chunks[3]);
}

fn move_cursor(delta: isize, len: usize, state: &mut ListState) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0);
    let delta_abs = delta.unsigned_abs();
    let new = if delta < 0 {
        (current + len - (delta_abs % len)) % len
    } else {
        (current + (delta_abs % len)) % len
    };
    state.select(Some(new));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn item(name: &str, detected: bool, selected: bool) -> SelectItem {
        SelectItem {
            name: name.to_string(),
            detected,
            selected,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn state_at(index: usize) -> ListState {
        let mut s = ListState::default();
        s.select(Some(index));
        s
    }

    #[test]
    fn render_select_all_detected_checked() {
        let items = vec![
            item("claude-code", true, true),
            item("codex", true, true),
            item("pi", true, true),
            item("opencode", true, true),
        ];
        let state = state_at(0);
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| render_select(f, &items, &state)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("ironlint · onboarding"));
        assert!(text.contains("4 of 4 selected"));
        for name in ["claude-code", "codex", "pi", "opencode"] {
            assert!(
                text.contains(&format!("[x] {}", name)),
                "missing checked: {}",
                name
            );
        }
        assert!(text.contains("detected"));
    }

    #[test]
    fn render_select_none_detected() {
        let items = vec![
            item("claude-code", false, false),
            item("codex", false, false),
            item("pi", false, false),
            item("opencode", false, false),
        ];
        let state = state_at(0);
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| render_select(f, &items, &state)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("0 of 4 selected"));
        for name in ["claude-code", "codex", "pi", "opencode"] {
            assert!(
                text.contains(&format!("[ ] {}", name)),
                "missing unchecked: {}",
                name
            );
        }
        assert!(!text.contains("detected"));
    }

    #[test]
    fn render_select_mixed() {
        let items = vec![
            item("claude-code", true, true),
            item("codex", true, true),
            item("pi", false, false),
            item("opencode", false, false),
        ];
        let state = state_at(0);
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| render_select(f, &items, &state)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("2 of 4 selected"));
        assert!(text.contains("[x] claude-code"));
        assert!(text.contains("[x] codex"));
        assert!(text.contains("[ ] pi"));
        assert!(text.contains("[ ] opencode"));
        assert!(text.contains("detected"));
    }

    #[test]
    fn render_select_cursor_position() {
        let items = vec![
            item("claude-code", true, true),
            item("codex", true, true),
            item("pi", true, true),
            item("opencode", true, true),
        ];
        for cursor in 0..4 {
            let state = state_at(cursor);
            let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
            term.draw(|f| render_select(f, &items, &state)).unwrap();
            let text = buffer_text(&term);
            let cursor_row = format!(
                "> [{}] {}",
                if items[cursor].selected { "x" } else { " " },
                items[cursor].name
            );
            assert!(
                text.contains(&cursor_row),
                "cursor {} row not found: {}",
                cursor,
                text
            );
            for (i, it) in items.iter().enumerate() {
                if i == cursor {
                    continue;
                }
                let sibling_row =
                    format!("  [{}] {}", if it.selected { "x" } else { " " }, it.name);
                assert!(
                    text.contains(&sibling_row),
                    "sibling row {} not found: {}",
                    i,
                    text
                );
            }
        }
    }

    #[test]
    fn render_select_toggle_reflected() {
        let mut items = vec![
            item("claude-code", true, true),
            item("codex", false, false),
            item("pi", false, false),
            item("opencode", false, false),
        ];
        let state = state_at(0);
        let mut term1 = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term1.draw(|f| render_select(f, &items, &state)).unwrap();
        let text1 = buffer_text(&term1);
        assert!(text1.contains("[x] claude-code"));
        assert!(text1.contains("1 of 4 selected"));

        items[0].selected = false;
        let mut term2 = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term2.draw(|f| render_select(f, &items, &state)).unwrap();
        let text2 = buffer_text(&term2);
        assert!(text2.contains("[ ] claude-code"));
        assert!(text2.contains("0 of 4 selected"));
    }

    #[test]
    fn update_select_moves_cursor() {
        let items = vec![
            item("claude-code", true, true),
            item("codex", true, true),
            item("pi", true, true),
            item("opencode", true, true),
        ];
        let state = state_at(0);

        let (_, s, _) = update_select(&items, key(KeyCode::Up), &state);
        assert_eq!(s.selected(), Some(3), "Up from 0 should wrap to bottom");

        let state = state_at(3);
        let (_, s, _) = update_select(&items, key(KeyCode::Down), &state);
        assert_eq!(s.selected(), Some(0), "Down from 3 should wrap to top");

        let state = state_at(1);
        let (_, s, _) = update_select(&items, key(KeyCode::Char('k')), &state);
        assert_eq!(s.selected(), Some(0), "k should move up");

        let (_, s, _) = update_select(&items, key(KeyCode::Char('j')), &s);
        assert_eq!(s.selected(), Some(1), "j should move down");
    }

    #[test]
    fn update_select_toggles() {
        let items = vec![
            item("claude-code", true, true),
            item("codex", false, false),
            item("pi", false, false),
            item("opencode", false, false),
        ];
        let state = state_at(0);
        let (items, _, _) = update_select(&items, key(KeyCode::Char(' ')), &state);
        assert!(!items[0].selected);
        assert!(items[1..].iter().all(|i| !i.selected));

        let (items, _, _) = update_select(&items, key(KeyCode::Char(' ')), &state);
        assert!(items[0].selected);
    }

    #[test]
    fn update_select_confirm_and_abort() {
        let items = vec![
            item("claude-code", true, true),
            item("codex", true, true),
            item("pi", true, true),
            item("opencode", true, true),
        ];
        let state = state_at(0);

        let (_, _, outcome) = update_select(&items, key(KeyCode::Enter), &state);
        assert_eq!(outcome, Some(SelectOutcome::Confirmed));

        let (_, _, outcome) = update_select(&items, key(KeyCode::Esc), &state);
        assert_eq!(outcome, Some(SelectOutcome::Aborted));

        let (_, _, outcome) = update_select(&items, key(KeyCode::Char('q')), &state);
        assert_eq!(outcome, Some(SelectOutcome::Aborted));

        let (_, _, outcome) = update_select(&items, ctrl_c(), &state);
        assert_eq!(outcome, Some(SelectOutcome::Aborted));

        let (_, _, outcome) = update_select(&items, key(KeyCode::Char('x')), &state);
        assert_eq!(outcome, None, "unknown key should stay pending");
    }
}
