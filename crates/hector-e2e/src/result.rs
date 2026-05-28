//! Forensics captured from one container run.

use std::path::Path;

#[derive(Debug, Default)]
pub struct RunResult {
    pub exit_code: i32,
    pub verdict: Option<serde_json::Value>,
    pub log_entries: Vec<serde_json::Value>,
    pub target_after: Option<String>,
    pub harness_log: String,
    pub drive_log: String,
}

impl RunResult {
    /// Load forensics from a host-side run dir. Missing files degrade
    /// gracefully — a lifecycle-broken run still produces a usable struct
    /// (its `drive_log` carries the failure context).
    pub fn from_run_dir(run_dir: &Path, exit_code: i32, target_file: &str) -> anyhow::Result<Self> {
        let drive_log = read_or_empty(&run_dir.join("drive.log"));
        let harness_log = read_or_empty(&run_dir.join("harness.log"));
        let verdict = read_optional(&run_dir.join("verdict.json"))
            .map(|s| serde_json::from_str::<serde_json::Value>(&s))
            .transpose()?;
        let log_entries = parse_jsonl(&run_dir.join(".hector/log.jsonl"))?;
        let target_after = read_optional(&run_dir.join("workdir").join(target_file));

        Ok(Self {
            exit_code,
            verdict,
            log_entries,
            target_after,
            harness_log,
            drive_log,
        })
    }
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn parse_jsonl(path: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let Some(text) = read_optional(path) else {
        return Ok(Vec::new());
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn from_run_dir_parses_all_artifacts() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("drive.log"), "phase 0 ok\nphase 1 ok\n").unwrap();
        fs::write(root.join("harness.log"), "agent: writing src/runner.ts\n").unwrap();
        fs::write(
            root.join("verdict.json"),
            r#"{"status":"block","violations":[]}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join(".hector")).unwrap();
        fs::write(
            root.join(".hector/log.jsonl"),
            r#"{"rule_id":"js-forbid-eval","status":"block"}
{"rule_id":"js-forbid-eval","status":"block"}
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("workdir/src")).unwrap();
        fs::write(
            root.join("workdir/src/runner.ts"),
            "function runScript(){}\n",
        )
        .unwrap();

        let r = RunResult::from_run_dir(root, 2, "src/runner.ts").unwrap();
        assert_eq!(r.exit_code, 2);
        assert_eq!(r.drive_log, "phase 0 ok\nphase 1 ok\n");
        assert_eq!(r.harness_log, "agent: writing src/runner.ts\n");
        assert!(r.verdict.is_some());
        assert_eq!(r.log_entries.len(), 2);
        assert_eq!(r.target_after.as_deref(), Some("function runScript(){}\n"),);
    }

    #[test]
    fn from_run_dir_degrades_gracefully_on_partial_run() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("drive.log"), "phase 0 ok\n").unwrap();
        // harness.log, verdict.json, log.jsonl, target_after all absent

        let r = RunResult::from_run_dir(root, 1, "src/runner.ts").unwrap();
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.drive_log, "phase 0 ok\n");
        assert_eq!(r.harness_log, "");
        assert!(r.verdict.is_none());
        assert!(r.log_entries.is_empty());
        assert!(r.target_after.is_none());
    }
}
