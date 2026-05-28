//! Integration coverage for the script-engine `output: parsed | passthrough`
//! mode. Drives the engine end-to-end through a real subprocess so we observe
//! the same `Vec<Violation>` shape the runner sees in production.

use hector_core::config::{Capabilities, EngineKind, OutputMode, Rule, Severity, WritesPolicy};
use hector_core::engine::script::run_script_rule;
use tempfile::tempdir;

fn make_rule(script: &str, output: OutputMode) -> Rule {
    Rule {
        description: "test rule".into(),
        engine: EngineKind::Script,
        scope: vec!["*".into()],
        severity: Severity::Error,
        script: Some(script.into()),
        pattern: None,
        language: None,
        context: None,
        capabilities: Some(Capabilities {
            network: false,
            writes: WritesPolicy::None,
        }),
        fix_hint: None,
        output,
    }
}

#[test]
fn parsed_mode_extracts_line_from_grep_n_output() {
    // `grep -n` emits `<line>:<text>`. Parsed mode must split that into
    // `line=Some(N)` and a clean message, not dump it verbatim with
    // `line: None`.
    let dir = tempdir().unwrap();
    let file = dir.path().join("dirty.txt");
    std::fs::write(&file, "ok line 1\nconsole.log('boom')\nok line 3\n").unwrap();
    let rule = make_rule(
        "grep -nE \"console\\.log\\(\" {file} && exit 1 || exit 0",
        OutputMode::Parsed,
    );
    let vs = run_script_rule("no-console-log", &rule, &file, "", dir.path()).expect("run");
    assert_eq!(vs.len(), 1, "expected one violation, got {vs:?}");
    let v = &vs[0];
    assert_eq!(
        v.line,
        Some(2),
        "parsed mode should extract the grep line number, got {v:?}"
    );
    assert!(
        v.message.contains("console.log"),
        "message should be the matched line text without the `2:` prefix, got {:?}",
        v.message
    );
    assert!(
        !v.message.starts_with("2:"),
        "the `<line>:` prefix should be stripped from the message, got {:?}",
        v.message
    );
}

#[test]
fn passthrough_mode_is_the_default_when_field_omitted() {
    // The default `output:` mode is Passthrough: an unconfigured rule lands
    // the verbatim stream in `message`. A stdout that *would* parse cleanly
    // under Parsed (`42:beta` → `line=42, message=beta`) must instead come
    // through verbatim with `line: None`. A `vs[0].line == Some(42)` here
    // means the default reverted to Parsed and biome-style frames will mangle.
    let dir = tempdir().unwrap();
    let file = dir.path().join("dirty.txt");
    std::fs::write(&file, "alpha\n42:beta\n").unwrap();
    // A `42:beta` line has grep-n shape but, under the passthrough default,
    // must land verbatim rather than being parsed into line + message.
    let rule = make_rule("printf '42:beta\\n' && exit 1", OutputMode::default());
    let vs = run_script_rule("default-mode", &rule, &file, "", dir.path()).expect("run");
    assert_eq!(vs.len(), 1, "expected one violation, got {vs:?}");
    assert_eq!(
        vs[0].line, None,
        "passthrough default must not extract a line number, got {:?}",
        vs[0]
    );
    assert_eq!(
        vs[0].message.trim(),
        "42:beta",
        "passthrough default must preserve the verbatim stream, got {:?}",
        vs[0].message
    );
}

#[test]
fn passthrough_mode_keeps_full_stream_in_message() {
    // Bully parity: `output: passthrough` is the escape hatch for
    // scripts that already format their own diagnostic. Line/column
    // stay None and the chosen stream lands verbatim.
    let dir = tempdir().unwrap();
    let file = dir.path().join("any.txt");
    std::fs::write(&file, "doesn't matter\n").unwrap();
    let rule = make_rule(
        "printf 'something opaque\\nhappened on line 99\\n' >&2 && exit 1",
        OutputMode::Passthrough,
    );
    let vs = run_script_rule("custom-formatter", &rule, &file, "", dir.path()).expect("run");
    assert_eq!(vs.len(), 1);
    let v = &vs[0];
    assert_eq!(v.line, None, "passthrough must not extract structure");
    assert_eq!(v.column, None);
    assert!(
        v.message.contains("something opaque"),
        "message must preserve the full stream verbatim, got {:?}",
        v.message
    );
    assert!(
        v.message.contains("happened on line 99"),
        "message must preserve the second line verbatim, got {:?}",
        v.message
    );
}

#[test]
fn parsed_mode_canonical_line_col_extracts_both() {
    // Real-world shape from ruff / eslint --format compact / clippy
    // --message-format short: `file:line:col: msg`.
    let dir = tempdir().unwrap();
    let file = dir.path().join("any.txt");
    std::fs::write(&file, "x\n").unwrap();
    let rule = make_rule(
        "printf 'src/foo.ts:14:5: missing semicolon\\n' && exit 1",
        OutputMode::Parsed,
    );
    let vs = run_script_rule("canonical", &rule, &file, "", dir.path()).expect("run");
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].file, "src/foo.ts");
    assert_eq!(vs[0].line, Some(14));
    assert_eq!(vs[0].column, Some(5));
    assert_eq!(vs[0].message, "missing semicolon");
}

#[test]
fn passthrough_mode_concatenates_stdout_and_stderr() {
    // Spec (specs/2026-05-12-bully-parity-closures.md:430): passthrough emits
    // combined stdout+stderr as one violation message verbatim. Regression:
    // the engine must combine both streams, not pick one — a script writing
    // to both must not lose half its output. The message must contain *both*
    // tokens.
    let dir = tempdir().unwrap();
    let file = dir.path().join("any.txt");
    std::fs::write(&file, "x\n").unwrap();
    let rule = make_rule(
        "printf 'out\\n' && printf 'err\\n' >&2 && exit 1",
        OutputMode::Passthrough,
    );
    let vs = run_script_rule("combined", &rule, &file, "", dir.path()).expect("run");
    assert_eq!(vs.len(), 1);
    let msg = &vs[0].message;
    assert!(
        msg.contains("out"),
        "passthrough message must include stdout token, got {msg:?}"
    );
    assert!(
        msg.contains("err"),
        "passthrough message must include stderr token, got {msg:?}"
    );
}

#[test]
fn parsed_mode_emits_one_violation_per_canonical_line() {
    // Multiple lints in one file → multiple violations, not one
    // concatenated message.
    let dir = tempdir().unwrap();
    let file = dir.path().join("any.txt");
    std::fs::write(&file, "x\n").unwrap();
    let rule = make_rule(
        "printf 'src/a.rs:1:1: first\\nsrc/b.rs:2:2: second\\n' && exit 1",
        OutputMode::Parsed,
    );
    let vs = run_script_rule("multi", &rule, &file, "", dir.path()).expect("run");
    assert_eq!(vs.len(), 2);
    assert_eq!(vs[0].line, Some(1));
    assert_eq!(vs[1].line, Some(2));
    assert_eq!(vs[1].file, "src/b.rs");
}
