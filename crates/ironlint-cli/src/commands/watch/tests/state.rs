use super::super::{handle_key, Loop, View, ViewState};
use super::summary_with;
use ratatui::crossterm::event::KeyCode;

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
