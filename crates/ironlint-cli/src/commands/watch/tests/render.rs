use super::super::render::{header_line, pad_line, truncate_line, GREEN, MUTED, RED_REST};
use super::super::runtime::ENTER_MS;
use super::super::{explorer_lines, stream_lines, ui, StreamRow, View, ViewState};
use super::{all_text, entry, line_text, prec, roll, settled, summary_with};
use ironlint_core::telemetry::LogEntry;
use ironlint_core::verdict::Status;
use ironlint_core::watch::LogSummary;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

const W: u16 = 80; // test pane width

// ── Task 3.1: stream_lines ────────────────────────────────────────────────

#[test]
fn stream_renders_pass_row_with_time_file_elapsed_event() {
    let e = vec![entry(
        Some("src/auth.ts"),
        None,
        "write",
        Status::Pass,
        12,
        vec![prec("lint", Status::Pass, None)],
    )];
    let text = all_text(&stream_lines(&settled(&e), None, W));
    assert!(text.contains("14:23:09"));
    assert!(text.contains("src/auth.ts"));
    assert!(text.contains("12ms"));
    assert!(text.contains("write"));
}

#[test]
fn stream_block_row_has_check_and_write_rejected_no_message() {
    let e = vec![entry(
        Some("src/auth.test.ts"),
        None,
        "write",
        Status::Block,
        12,
        vec![prec("no-focused-tests", Status::Block, None)],
    )];
    let text = all_text(&stream_lines(&settled(&e), None, W));
    assert!(text.contains("no-focused-tests"));
    assert!(text.contains("write rejected"));
    assert!(!text.contains("exited"));
}

#[test]
fn stream_precommit_row_shows_set_size_and_commit() {
    let e = vec![entry(
        None,
        Some(47),
        "pre-commit",
        Status::Pass,
        12,
        vec![prec("lint", Status::Pass, None)],
    )];
    let text = all_text(&stream_lines(&settled(&e), None, W));
    assert!(text.contains("pre-commit · 47 files"));
    assert!(text.contains("commit"));
}

#[test]
fn stream_internal_error_shows_reason() {
    let e = vec![entry(
        Some("big.ts"),
        None,
        "write",
        Status::InternalError,
        12,
        vec![prec("types-pass", Status::InternalError, Some("timeout"))],
    )];
    let text = all_text(&stream_lines(&settled(&e), None, W));
    assert!(text.contains("check error: timeout"));
}

#[test]
fn stream_is_newest_first() {
    let mut a = entry(
        Some("old.ts"),
        None,
        "write",
        Status::Pass,
        12,
        vec![prec("lint", Status::Pass, None)],
    );
    let LogEntry::Check { ts, .. } = &mut a;
    *ts = "2026-06-28T14:00:00+00:00".into();
    let b = entry(
        Some("new.ts"),
        None,
        "write",
        Status::Pass,
        12,
        vec![prec("lint", Status::Pass, None)],
    );
    let lines = stream_lines(&settled(&[a, b]), None, W);
    assert!(line_text(&lines[0]).contains("new.ts"));
}

#[test]
fn stream_filter_keeps_only_matching_check() {
    let e = vec![
        entry(
            Some("a.ts"),
            None,
            "write",
            Status::Pass,
            12,
            vec![prec("lint", Status::Pass, None)],
        ),
        entry(
            Some("b.ts"),
            None,
            "write",
            Status::Pass,
            12,
            vec![prec("types", Status::Pass, None)],
        ),
    ];
    let text = all_text(&stream_lines(&settled(&e), Some("types"), W));
    assert!(text.contains("b.ts"));
    assert!(!text.contains("a.ts"));
}

#[test]
fn stream_block_row_is_tinted_full_width_on_row_and_detail() {
    let e = entry(
        Some("src/lib.rs"),
        None,
        "write",
        Status::Block,
        8,
        vec![prec("ruff", Status::Block, None)],
    );
    let lines = stream_lines(&settled(&[e]), None, W);
    // main row + detail line, both padded to full width and background-tinted.
    let row = &lines[0];
    let detail = &lines[1];
    assert!(
        line_text(row).chars().count() >= usize::from(W),
        "block row not padded to width"
    );
    assert!(
        row.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
        "every span of a blocked row carries the red tint"
    );
    assert!(
        detail.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
        "blocked detail line is tinted too"
    );
}

#[test]
fn stream_pass_row_is_not_tinted() {
    let e = entry(Some("src/main.rs"), None, "write", Status::Pass, 12, vec![]);
    let lines = stream_lines(&settled(&[e]), None, W);
    assert!(
        lines[0].spans.iter().all(|s| s.style.bg.is_none()),
        "passing rows have no background"
    );
}

#[test]
fn stream_mid_wipe_row_is_truncated() {
    let e = entry(Some("src/main.rs"), None, "write", Status::Pass, 12, vec![]);
    // age halfway through the wipe -> reveal ~half the pane width.
    let rows = vec![StreamRow {
        entry: &e,
        age_ms: Some(ENTER_MS / 2),
    }];
    let lines = stream_lines(&rows, None, W);
    let shown = line_text(&lines[0]).chars().count();
    assert!(
        shown > 0 && shown < usize::from(W),
        "expected a partial reveal, got {shown}"
    );
}

#[test]
fn stream_block_row_mid_wipe_is_tinted_and_truncated() {
    // Proves push_row's tint-then-truncate order: a blocked row that is
    // still animating must show BOTH the red tint on its revealed spans
    // AND a partial (not full-width) reveal.
    let e = entry(
        Some("src/lib.rs"),
        None,
        "write",
        Status::Block,
        8,
        vec![prec("ruff", Status::Block, None)],
    );
    let rows = vec![StreamRow {
        entry: &e,
        age_ms: Some(ENTER_MS / 2),
    }];
    let lines = stream_lines(&rows, None, W);
    let row = &lines[0];
    assert!(
        row.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
        "mid-wipe blocked row must keep the red tint on its revealed spans"
    );
    let shown = line_text(row).chars().count();
    assert!(
        shown > 0 && shown < usize::from(W),
        "mid-wipe blocked row must be truncated to a partial reveal, got {shown}"
    );
}

#[test]
fn stream_block_entry_tints_block_detail_but_not_internal_detail() {
    // Entry-level status is Block (one of its checks blocked); it carries
    // both a Block sub-check and an InternalError sub-check. Proves the
    // per-check tint keying: the main row and the blocking check's detail
    // line get the red tint, but the InternalError detail line does not.
    let e = entry(
        Some("src/lib.rs"),
        None,
        "write",
        Status::Block,
        8,
        vec![
            prec("ruff", Status::Block, None),
            prec("types-check", Status::InternalError, Some("timeout")),
        ],
    );
    let lines = stream_lines(&settled(&[e]), None, W);
    let main = &lines[0];
    let block_detail = &lines[1];
    let internal_detail = &lines[2];
    assert!(
        main.spans.iter().all(|s| s.style.bg == Some(RED_REST)),
        "entry blocked -> main row tinted full width"
    );
    assert!(
        block_detail
            .spans
            .iter()
            .all(|s| s.style.bg == Some(RED_REST)),
        "blocking check's own detail line is tinted red"
    );
    assert!(
        internal_detail.spans.iter().all(|s| s.style.bg.is_none()),
        "internal-error detail line keeps its amber text untinted"
    );
    assert!(line_text(internal_detail).contains("check error"));
}

// ── Task 3.2: explorer_lines ──────────────────────────────────────────────

#[test]
fn explorer_summary_line_has_totals_and_pass_pct() {
    let s = LogSummary {
        runs: 159,
        blocks: 4,
        internal: 1,
        pass: 154,
        rollups: vec![],
    };
    let text = all_text(&explorer_lines(&s, 0));
    assert!(text.contains("159 runs"));
    assert!(text.contains("4 blocks"));
    assert!(text.contains("1 internal"));
    assert!(text.contains("97% pass"));
}

#[test]
fn explorer_summary_shows_dash_pass_on_empty_log() {
    let s = LogSummary {
        runs: 0,
        blocks: 0,
        internal: 0,
        pass: 0,
        rollups: vec![],
    };
    let text = all_text(&explorer_lines(&s, 0));
    assert!(text.contains("— pass"));
}

#[test]
fn explorer_lists_blocking_then_divider_then_clean() {
    let s = LogSummary {
        runs: 10,
        blocks: 3,
        internal: 0,
        pass: 7,
        rollups: vec![
            roll("nft", 15, 3, 0, Some(11)),
            roll("no-secrets", 50, 0, 0, Some(3)),
        ],
    };
    let lines = explorer_lines(&s, 0);
    let text = all_text(&lines);
    assert!(text.contains("nft"));
    assert!(text.contains("20%"));
    assert!(text.contains("11ms"));
    assert!(text.contains("NO BLOCKS IN LOG"));
    assert!(text.contains("no-secrets"));
    let nft_idx = lines
        .iter()
        .position(|l| line_text(l).contains("nft"))
        .unwrap();
    let div_idx = lines
        .iter()
        .position(|l| line_text(l).contains("NO BLOCKS IN LOG"))
        .unwrap();
    let sec_idx = lines
        .iter()
        .position(|l| line_text(l).contains("no-secrets"))
        .unwrap();
    assert!(nft_idx < div_idx && div_idx < sec_idx);
}

#[test]
fn explorer_shows_internal_warning_and_dash_p50() {
    let s = LogSummary {
        runs: 1,
        blocks: 0,
        internal: 1,
        pass: 0,
        rollups: vec![roll("types", 1, 0, 1, None)],
    };
    let text = all_text(&explorer_lines(&s, 0));
    assert!(text.contains("⚠ 1"));
    assert!(text.contains("—"));
}

#[test]
fn explorer_omits_divider_when_all_checks_block() {
    let s = LogSummary {
        runs: 1,
        blocks: 1,
        internal: 0,
        pass: 0,
        rollups: vec![roll("only", 1, 1, 0, Some(5))],
    };
    let text = all_text(&explorer_lines(&s, 0));
    assert!(!text.contains("NO BLOCKS IN LOG"));
}

// ── Task 3.3: ui() ───────────────────────────────────────────────────────

#[test]
fn ui_stream_view_renders_feed_and_footer() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let entries = vec![entry(
        Some("src/auth.ts"),
        None,
        "write",
        Status::Pass,
        12,
        vec![prec("lint", Status::Pass, None)],
    )];
    let summary = LogSummary {
        runs: 1,
        blocks: 0,
        internal: 0,
        pass: 1,
        rollups: vec![roll("lint", 1, 0, 0, Some(5))],
    };
    let state = ViewState::default();
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| ui(f, &settled(&entries), &summary, 7, &state, true))
        .unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("stream"));
    assert!(text.contains("explorer"));
    assert!(text.contains("src/auth.ts"));
    assert!(text.contains("7 checks armed"));
    assert!(text.contains("quit"));
}

#[test]
fn ui_explorer_view_renders_table() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let summary = LogSummary {
        runs: 10,
        blocks: 1,
        internal: 0,
        pass: 9,
        rollups: vec![roll("nft", 5, 1, 0, Some(11))],
    };
    let state = ViewState {
        view: View::Explorer,
        selected: 0,
        filter: None,
    };
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| ui(f, &[], &summary, 7, &state, true))
        .unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("RANKED BY BLOCKS"));
    assert!(text.contains("nft"));
}

// ── New ui state tests ────────────────────────────────────────────────────

#[test]
fn ui_config_unavailable_shows_degraded_banner() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let summary = LogSummary {
        runs: 0,
        blocks: 0,
        internal: 0,
        pass: 0,
        rollups: vec![],
    };
    let state = ViewState::default();
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| ui(f, &[], &summary, 0, &state, false))
        .unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("config unavailable"));
}

#[test]
fn ui_cold_start_shows_waiting_hint() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let summary = LogSummary {
        runs: 0,
        blocks: 0,
        internal: 0,
        pass: 0,
        rollups: vec![],
    };
    let state = ViewState::default(); // Stream view, no filter
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| ui(f, &[], &summary, 0, &state, true))
        .unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("waiting for edits"));
}

// ── Task 4: header verdict dot ────────────────────────────────────────────

#[test]
fn header_shows_verdict_dot_not_a_clock() {
    let pass = summary_with(&["ruff"]); // no blocks
    let line = header_line(&ViewState::default(), &pass);
    let text = line_text(&line);
    assert!(text.contains("● PASS"), "got: {text}");
    assert!(!text.contains(':'), "clock should be gone, got: {text}");
}

#[test]
fn header_flips_to_block_when_blocks_present() {
    let mut s = summary_with(&["ruff"]);
    s.blocks = 3;
    let text = line_text(&header_line(&ViewState::default(), &s));
    assert!(text.contains("● BLOCK"), "got: {text}");
}

// ── Task 2: line helpers ─────────────────────────────────────────────────

#[test]
fn truncate_line_keeps_first_n_columns_across_spans() {
    let line = Line::from(vec![
        Span::styled("abc", Style::default().fg(GREEN)),
        Span::styled("defg", Style::default().fg(MUTED)),
    ]);
    assert_eq!(line_text(&truncate_line(line.clone(), 0)), "");

    // Truncate to 2: "ab" keeps first span partial with GREEN style
    let t2 = truncate_line(line.clone(), 2);
    assert_eq!(line_text(&t2), "ab");
    assert_eq!(
        t2.spans[0].style.fg,
        Some(GREEN),
        "partial first span must retain GREEN"
    );

    // Truncate to 3: exact boundary of first span, keeps "abc" whole with GREEN
    let t3 = truncate_line(line.clone(), 3);
    assert_eq!(line_text(&t3), "abc");
    assert_eq!(t3.spans.len(), 1, "exact boundary: only first span kept");
    assert_eq!(t3.spans[0].style.fg, Some(GREEN), "first span keeps GREEN");

    // Truncate to 5: "abcde" splits second span, keeps MUTED on split
    let t5 = truncate_line(line.clone(), 5);
    assert_eq!(line_text(&t5), "abcde");
    assert_eq!(t5.spans.len(), 2, "split second span means two spans");
    assert_eq!(t5.spans[0].style.fg, Some(GREEN), "first span keeps GREEN");
    assert_eq!(
        t5.spans[1].style.fg,
        Some(MUTED),
        "split second span keeps MUTED"
    );

    assert_eq!(line_text(&truncate_line(line, 99)), "abcdefg"); // over-long = whole line
}

#[test]
fn pad_line_fills_to_width_with_style() {
    let line = Line::from(Span::raw("abc"));
    let padded = pad_line(line, 6, Style::default().bg(RED_REST));
    assert_eq!(line_text(&padded), "abc   ");
    // the padding span carries the fill style
    let last = padded.spans.last().unwrap();
    assert_eq!(last.style.bg, Some(RED_REST));
}

#[test]
fn pad_line_noop_when_already_at_width() {
    // Build a 6-char line, pad to 6: exact boundary must be a true no-op
    let line = Line::from(Span::raw("abcdef"));
    let spans_before = line.spans.len();
    let padded = pad_line(line, 6, Style::default().bg(RED_REST));
    assert_eq!(line_text(&padded), "abcdef");
    assert_eq!(
        padded.spans.len(),
        spans_before,
        "width == current width: no extra span appended"
    );

    // Also verify that padding a line shorter than the requested width works
    let line_short = Line::from(Span::raw("abc"));
    let padded_short = pad_line(line_short, 6, Style::default().bg(RED_REST));
    assert_eq!(line_text(&padded_short), "abc   ");
    // the padding span carries the fill style
    let last = padded_short.spans.last().unwrap();
    assert_eq!(last.style.bg, Some(RED_REST));
}
