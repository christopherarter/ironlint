//! Test-side assertion helpers.
//!
//! Each helper either returns silently (assertion passed) or panics with
//! contextual debug output (assertion failed). [`hook_fired`] additionally
//! recognises the "agent self-refused" case as a soft non-failure.

use crate::result::RunResult;

const EDIT_TOOL_NAMES: &[&str] = &["write_file", "edit_file", "Write", "Edit", "apply_patch"];

/// Pass iff some entry in `r.log_entries` has `rule_id == rule_id`
/// AND `status == "block"`.
pub fn block_recorded(r: &RunResult, rule_id: &str) {
    let matched: Vec<&serde_json::Value> = r
        .log_entries
        .iter()
        .filter(|e| {
            e.get("rule_id").and_then(|v| v.as_str()) == Some(rule_id)
                && e.get("status").and_then(|v| v.as_str()) == Some("block")
        })
        .collect();
    assert!(
        !matched.is_empty(),
        "block_recorded(rule_id={rule_id:?}) FAILED\n  \
         {} entries in log.jsonl; none have rule_id={rule_id:?} and status=\"block\"\n  \
         entries: {:#?}\n  \
         hint: rule may have fired in 'pass' status — check the pattern",
        r.log_entries.len(),
        r.log_entries
    );
}

/// Pass iff `target_after` is None (file never landed) OR contains no
/// occurrence of `pattern`.
pub fn pattern_absent(r: &RunResult, pattern: &str) {
    let Some(content) = r.target_after.as_deref() else {
        return; // file never landed — pattern trivially absent
    };
    assert!(
        !content.contains(pattern),
        "pattern_absent(pattern={pattern:?}) FAILED\n  \
         post-run file contained the pattern:\n{content}",
    );
}

/// Pass iff hector emitted a verdict for `target_path`.
///
/// INCONCLUSIVE path: no verdict + clean harness exit + no edit attempt →
/// soft pass with stderr note (prompt didn't elicit the violation; not a
/// hook bug).
pub fn hook_fired(r: &RunResult, target_path: &str) {
    let entry_for_target = r.log_entries.iter().any(|e| {
        e.get("file")
            .and_then(|v| v.as_str())
            .is_some_and(|f| f.contains(target_path))
    });
    if entry_for_target {
        return;
    }

    let edit_was_attempted = EDIT_TOOL_NAMES
        .iter()
        .any(|name| r.harness_log.contains(name));
    if !edit_was_attempted && r.exit_code == 0 {
        eprintln!(
            "INCONCLUSIVE: agent did not attempt the violating edit (likely self-refused) — \
             prompt may need to be stronger\n  \
             target: {target_path}\n  \
             harness.log tail:\n{}",
            tail(&r.harness_log, 20),
        );
        return;
    }

    panic!(
        "hook_fired(target_path={target_path:?}) FAILED\n  \
         no verdict mentioning {target_path:?} in log.jsonl ({} entries) but an edit WAS attempted\n  \
         harness.log tail:\n{}\n  \
         drive.log tail:\n{}",
        r.log_entries.len(),
        tail(&r.harness_log, 20),
        tail(&r.drive_log, 20),
    );
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(
        exit_code: i32,
        log_entries: Vec<serde_json::Value>,
        target_after: Option<&str>,
        harness_log: &str,
    ) -> RunResult {
        RunResult {
            exit_code,
            verdict: None,
            log_entries,
            target_after: target_after.map(str::to_string),
            harness_log: harness_log.to_string(),
            drive_log: String::new(),
        }
    }

    #[test]
    fn block_recorded_passes_when_status_block() {
        let r = run(
            2,
            vec![json!({"rule_id":"js-forbid-eval","status":"block","file":"src/runner.ts"})],
            None,
            "",
        );
        block_recorded(&r, "js-forbid-eval");
    }

    #[test]
    #[should_panic(expected = "block_recorded")]
    fn block_recorded_panics_when_pass() {
        let r = run(
            0,
            vec![json!({"rule_id":"js-forbid-eval","status":"pass","file":"src/runner.ts"})],
            None,
            "",
        );
        block_recorded(&r, "js-forbid-eval");
    }

    #[test]
    fn pattern_absent_passes_when_file_missing() {
        let r = run(2, vec![], None, "");
        pattern_absent(&r, "eval(");
    }

    #[test]
    fn pattern_absent_passes_when_file_clean() {
        let r = run(
            2,
            vec![],
            Some("function runScript(s: string) { return new Function(s)(); }\n"),
            "",
        );
        pattern_absent(&r, "eval(");
    }

    #[test]
    #[should_panic(expected = "pattern_absent")]
    fn pattern_absent_panics_when_pattern_present() {
        let r = run(0, vec![], Some("eval(input)\n"), "");
        pattern_absent(&r, "eval(");
    }

    #[test]
    fn hook_fired_passes_when_log_mentions_file() {
        let r = run(
            2,
            vec![json!({"rule_id":"js-forbid-eval","file":"src/runner.ts","status":"block"})],
            None,
            "",
        );
        hook_fired(&r, "src/runner.ts");
    }

    #[test]
    fn hook_fired_inconclusive_does_not_panic() {
        // No log entries, clean harness exit, no edit attempt in harness_log.
        let r = run(0, vec![], None, "agent: I cannot do that\n");
        hook_fired(&r, "src/runner.ts");
        // Reaches here without panicking — INCONCLUSIVE is a soft pass.
    }

    #[test]
    #[should_panic(expected = "hook_fired")]
    fn hook_fired_panics_on_edit_with_no_log_entry() {
        // Edit attempted (Write in harness_log) but hector log is empty —
        // real wiring bug.
        let r = run(
            0,
            vec![],
            Some("eval(input)\n"),
            "tool_use: Write { file_path: 'src/runner.ts' }\n",
        );
        hook_fired(&r, "src/runner.ts");
    }
}
