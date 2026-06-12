//! `hector doctor` diagnostic subcommand.
//!
//! Read-only. Walks a fixed list of checks (binary on PATH, config
//! present, config parses, trust verifies, schema version, scope
//! globs, engine availability, adapter presence, runtime state) and
//! prints a checklist by default, or a JSON `Report` under `--format
//! json`. Exit code: 0 on all-pass-or-warn, 1 on any fail.

use crate::cli::OutputFormat;
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One row in the doctor report. `name` is the stable check id used in
/// the JSON output (snake_case, additive-only). `detail` is one short
/// sentence; `remediation` is the actionable hint shown when the
/// status is not `Pass`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

/// JSON payload emitted by `--format json`. Public contract — see
/// `docs/operating/diagnostics.md`. New fields land at the end of the struct with
/// `Option<…>` defaults so the schema stays additive.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub hector_version: String,
    pub checks: Vec<CheckResult>,
}

/// Per-doctor-run inputs shared across every check. Stays small —
/// each check borrows what it needs and pulls anything else from the
/// process environment (env vars, fs).
struct DoctorContext {
    dir: PathBuf,
    config_path: PathBuf,
}

pub fn run(dir: &Path, format: OutputFormat) -> Result<i32> {
    let ctx = DoctorContext {
        dir: dir.to_path_buf(),
        config_path: dir.join(".hector.yml"),
    };
    let checks: Vec<CheckResult> = vec![
        check_binary(),
        check_config_present(&ctx),
        check_config_parses(&ctx),
        check_trust(&ctx),
        check_schema_version(&ctx),
        check_scope_globs(&ctx),
        check_engines(&ctx),
        check_capabilities(),
        check_adapter(),
        check_runtime_state(&ctx),
    ];
    let report = Report {
        hector_version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
    };
    emit(&report, format)?;
    Ok(exit_code(&report))
}

fn exit_code(report: &Report) -> i32 {
    i32::from(report.checks.iter().any(|c| c.status == Status::Fail))
}

fn emit(report: &Report, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
        OutputFormat::Human => {
            println!("hector doctor — version {}", report.hector_version);
            for c in &report.checks {
                let glyph = match c.status {
                    Status::Pass => "ok  ",
                    Status::Warn => "warn",
                    Status::Fail => "fail",
                };
                println!("  [{glyph}] {} — {}", c.name, c.detail);
                if c.status != Status::Pass {
                    if let Some(hint) = &c.remediation {
                        println!("         {}", hint);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Binary on PATH + version. Trivially `pass` once the user reaches us
/// (we're a binary that ran), but report the resolved path and version
/// so the human checklist surfaces "which hector am I talking to".
fn check_binary() -> CheckResult {
    let path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    CheckResult {
        name: "binary",
        status: Status::Pass,
        detail: format!("hector {} at {}", env!("CARGO_PKG_VERSION"), path),
        remediation: None,
    }
}

/// Config file present at `<dir>/.hector.yml`. Hard requirement; without
/// a config Hector has nothing to do.
fn check_config_present(ctx: &DoctorContext) -> CheckResult {
    if ctx.config_path.exists() {
        CheckResult {
            name: "config",
            status: Status::Pass,
            detail: format!("{} exists", ctx.config_path.display()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "config",
            status: Status::Fail,
            detail: format!("{} not found", ctx.config_path.display()),
            remediation: Some("run `hector init` to scaffold a starter config".into()),
        }
    }
}

/// Config parses. We deliberately use the **non-trust-verifying**
/// resolver so a parses-OK-but-untrusted config reports `parses: pass`
/// and `trust: fail`, instead of collapsing both into one fail row.
/// Schema-v1 configs fail here with a clear `hector migrate` hint
/// (the resolver detects v1 before trust verify — see
/// `config/extends.rs`).
fn check_config_parses(ctx: &DoctorContext) -> CheckResult {
    if !ctx.config_path.exists() {
        return CheckResult {
            name: "parses",
            status: Status::Fail,
            detail: "config missing; nothing to parse".into(),
            remediation: Some("run `hector init` first".into()),
        };
    }
    match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(_) => CheckResult {
            name: "parses",
            status: Status::Pass,
            detail: "config parses (extends resolved)".into(),
            remediation: None,
        },
        Err(e) => {
            let msg = format!("{e:#}");
            // Surface the v1-migration hint verbatim if extends::resolve refused on schema_version: 1.
            let hint = if msg.contains("schema_version 1") {
                Some("run `hector migrate` to upgrade `.bully.yml`/v1 config to v2".into())
            } else {
                Some("fix the YAML error above and re-run".into())
            };
            CheckResult {
                name: "parses",
                status: Status::Fail,
                detail: msg,
                remediation: hint,
            }
        }
    }
}

/// Trust fingerprint matches recomputed canonical hash. Skipped (warn)
/// when parses already failed — there's no fingerprint to verify.
fn check_trust(ctx: &DoctorContext) -> CheckResult {
    if !ctx.config_path.exists() {
        return CheckResult {
            name: "trust",
            status: Status::Warn,
            detail: "skipped (no config)".into(),
            remediation: None,
        };
    }
    let raw = match std::fs::read_to_string(&ctx.config_path) {
        Ok(s) => s,
        Err(e) => {
            return CheckResult {
                name: "trust",
                status: Status::Fail,
                detail: format!("read failed: {e}"),
                remediation: Some("ensure the config file is readable".into()),
            };
        }
    };
    match hector_core::trust::verify(&raw) {
        Ok(()) => CheckResult {
            name: "trust",
            status: Status::Pass,
            detail: "fingerprint matches".into(),
            remediation: None,
        },
        Err(e) => CheckResult {
            name: "trust",
            status: Status::Fail,
            detail: format!("{e:#}"),
            remediation: Some(
                "review the diff against the last trusted state, then run `hector trust` to acknowledge".into(),
            ),
        },
    }
}

/// schema_version is one of `SUPPORTED_SCHEMAS`. v1 is `fail` (legacy
/// bully); v2 is `pass`; anything else is `fail` with a "this hector
/// is too old/new" hint.
fn check_schema_version(ctx: &DoctorContext) -> CheckResult {
    let raw = match std::fs::read_to_string(&ctx.config_path) {
        Ok(s) => s,
        Err(_) => {
            return CheckResult {
                name: "schema",
                status: Status::Warn,
                detail: "skipped (no config)".into(),
                remediation: None,
            };
        }
    };
    match hector_core::config::peek_schema_version(&raw) {
        Some(2) => CheckResult {
            name: "schema",
            status: Status::Pass,
            detail: "schema_version: 2".into(),
            remediation: None,
        },
        Some(1) => CheckResult {
            name: "schema",
            status: Status::Fail,
            detail: "schema_version: 1 (legacy bully)".into(),
            remediation: Some("run `hector migrate` to upgrade to schema_version 2".into()),
        },
        Some(n) => CheckResult {
            name: "schema",
            status: Status::Fail,
            detail: format!("schema_version: {n} (unsupported)"),
            remediation: Some(format!(
                "this hector supports {:?}; upgrade or downgrade hector to match",
                hector_core::config::SUPPORTED_SCHEMAS
            )),
        },
        None => CheckResult {
            name: "schema",
            status: Status::Fail,
            detail: "schema_version field missing or unparseable".into(),
            remediation: Some("add `schema_version: 2` at the top of `.hector.yml`".into()),
        },
    }
}

/// Every rule's scope globs construct a valid `ScopeMatcher`. The
/// runner already validates this at load time, but doctor surfaces it
/// as its own row so a globset error doesn't masquerade as a generic
/// parse failure. Skipped (warn) when the config doesn't parse.
fn check_scope_globs(ctx: &DoctorContext) -> CheckResult {
    let cfg = match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "scope_globs",
                status: Status::Warn,
                detail: "skipped (config does not parse)".into(),
                remediation: None,
            };
        }
    };
    let mut bad: Vec<String> = Vec::new();
    for (rule_id, rule) in &cfg.rules {
        if let Err(e) = hector_core::config::scope::ScopeMatcher::new(&rule.scope) {
            bad.push(format!("{rule_id}: {e:#}"));
        }
    }
    if bad.is_empty() {
        CheckResult {
            name: "scope_globs",
            status: Status::Pass,
            detail: format!("{} rule(s) have valid scope", cfg.rules.len()),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "scope_globs",
            status: Status::Fail,
            detail: format!("invalid scope on: {}", bad.join("; ")),
            remediation: Some("fix the listed glob(s) and re-run `hector trust`".into()),
        }
    }
}

/// Engine availability:
///   - Semantic / Session rules → require an `llm:` block whose
///     `api_key_env` resolves to a non-empty value (Ollama is exempt
///     from the api-key requirement, mirroring `llm::build_from_config`).
///   - All-script / all-ast configs → trivially pass.
fn check_engines(ctx: &DoctorContext) -> CheckResult {
    let cfg = match hector_core::config::parse_file_with_extends(&ctx.config_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                name: "engines",
                status: Status::Warn,
                detail: "skipped (config does not parse)".into(),
                remediation: None,
            };
        }
    };
    let needs_llm = cfg.rules.values().any(|r| {
        matches!(
            r.engine,
            hector_core::config::EngineKind::Semantic | hector_core::config::EngineKind::Session
        )
    });
    if !needs_llm {
        return CheckResult {
            name: "engines",
            status: Status::Pass,
            detail: "deterministic engines only (no LLM required)".into(),
            remediation: None,
        };
    }
    llm_block_status(cfg.llm.as_ref())
}

/// Inspect the `llm:` block for a config that has at least one
/// semantic/session rule. Returns the engine-row `CheckResult` directly
/// so the caller stays a one-liner.
fn llm_block_status(cfg: Option<&hector_core::config::LlmConfig>) -> CheckResult {
    let Some(llm) = cfg else {
        return CheckResult {
            name: "engines",
            status: Status::Warn,
            detail: "semantic/session rule(s) present but no `llm:` block configured".into(),
            remediation: Some(
                "add an `llm:` block with provider/model/api_key_env (see docs/configuring/llm-providers.md)"
                    .into(),
            ),
        };
    };
    // Ollama needs no API key — `build_from_config` defaults to an empty key.
    if llm.provider == "ollama" {
        return CheckResult {
            name: "engines",
            status: Status::Pass,
            // Ollama requires a model at runtime (`build_from_config`
            // errors when None), so the row surfaces the configured value
            // or "(unset)" rather than masking the misconfiguration.
            detail: format!(
                "provider=ollama, model={}",
                llm.model.as_deref().unwrap_or("(unset)")
            ),
            remediation: None,
        };
    }
    let env_name = match llm.api_key_env.as_deref() {
        Some(n) if !n.is_empty() => n,
        _ => {
            return CheckResult {
                name: "engines",
                status: Status::Warn,
                detail: format!("provider={} but `api_key_env` is unset", llm.provider),
                remediation: Some(
                    "set `api_key_env: <NAME>` in the `llm:` block of `.hector.yml`".into(),
                ),
            };
        }
    };
    if hector_core::llm::api_key_env_present(env_name) {
        CheckResult {
            name: "engines",
            status: Status::Pass,
            // Direct-API providers must set a model; render `(unset)`
            // rather than panicking so the row surfaces the misconfiguration.
            detail: format!(
                "provider={}, model={}, ${env_name} resolves",
                llm.provider,
                llm.model.as_deref().unwrap_or("(unset)")
            ),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "engines",
            status: Status::Warn,
            detail: format!(
                "env var `{env_name}` not set; semantic/session rules will error at evaluation"
            ),
            remediation: Some(format!(
                "export `{env_name}` with a valid {} API key",
                llm.provider
            )),
        }
    }
}

/// Locate `~/.claude/settings.json` (honoring `HOME`/`USERPROFILE`).
/// Returns `None` if the home dir is unresolvable or the file is absent —
/// caller maps that to a `warn` row.
fn load_claude_settings() -> Option<(PathBuf, serde_json::Value)> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = PathBuf::from(home).join(".claude").join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let value = serde_json::from_str(&raw).ok()?;
    Some((path, value))
}

/// Walk the parsed `~/.claude/settings.json` looking for a PostToolUse
/// hook whose `command` references `hector` (the binary) or a Hector
/// adapter `hook.sh`. Returns true on first match.
fn claude_hook_wired(settings: &serde_json::Value) -> bool {
    let Some(post) = settings
        .get("hooks")
        .and_then(|h| h.get("PostToolUse"))
        .and_then(|p| p.as_array())
    else {
        return false;
    };
    post.iter().any(|matcher_block| {
        matcher_block
            .get("hooks")
            .and_then(|hs| hs.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|cmd| cmd.contains("hector") || cmd.contains("hook.sh"))
                })
            })
            .unwrap_or(false)
    })
}

/// `<dir>/.hector/` is writable. Probes by creating the dir if absent,
/// writing a marker file, then deleting it. We DO create `.hector/` —
/// that's the same kind of side effect as `init` writing `.hector.yml`,
/// and "doctor never modifies state" is about *policy* state (configs,
/// baselines, telemetry), not about the run-state directory itself.
///
/// Also reports current sizes of `baseline.json`, `session.json`, and
/// `log.jsonl` if present, so the human checklist surfaces "your
/// telemetry log has grown to 200MB" without forcing the user to
/// `du -sh .hector/`.
fn check_runtime_state(ctx: &DoctorContext) -> CheckResult {
    let hector_dir = ctx.dir.join(".hector");
    if let Err(e) = std::fs::create_dir_all(&hector_dir) {
        return CheckResult {
            name: "runtime_state",
            status: Status::Fail,
            detail: format!("cannot create {}: {e}", hector_dir.display()),
            remediation: Some(format!(
                "ensure {} is writable (chmod / ownership)",
                ctx.dir.display()
            )),
        };
    }
    let probe = hector_dir.join(".doctor-write-probe");
    if let Err(e) = std::fs::write(&probe, b"ok") {
        return CheckResult {
            name: "runtime_state",
            status: Status::Fail,
            detail: format!("cannot write to {}: {e}", hector_dir.display()),
            remediation: Some(format!("chmod u+w {}", hector_dir.display())),
        };
    }
    let _ = std::fs::remove_file(&probe);

    let mut sizes: Vec<String> = Vec::new();
    for name in ["baseline.json", "session.json", "log.jsonl"] {
        if let Ok(meta) = std::fs::metadata(hector_dir.join(name)) {
            sizes.push(format!("{name}={}b", meta.len()));
        }
    }
    let detail = if sizes.is_empty() {
        format!("{} writable (empty)", hector_dir.display())
    } else {
        format!("{} writable ({})", hector_dir.display(), sizes.join(", "))
    };
    CheckResult {
        name: "runtime_state",
        status: Status::Pass,
        detail,
        remediation: None,
    }
}

/// Surfaces the capability-sandbox story for the running platform.
///
/// Linux enforces `network: false` via `CLONE_NEWNET` (best-effort
/// fallback on EPERM, which still warns at runtime because the user
/// asked for isolation and didn't get it). macOS and other non-Linux
/// targets have no equivalent — they run script rules unrestricted.
///
/// Status is `warn` (not `fail`) on best-effort platforms because the
/// limitation is platform reality, not a misconfiguration the user can
/// remediate. Status is `pass` on Linux.
fn check_capabilities() -> CheckResult {
    match hector_core::engine::capability::platform_capability_status() {
        None => CheckResult {
            name: "capabilities",
            status: Status::Pass,
            detail: "namespace isolation available (CLONE_NEWNET enforces `network: false`)"
                .into(),
            remediation: None,
        },
        Some(msg) => CheckResult {
            name: "capabilities",
            status: Status::Warn,
            detail: format!("{msg} (see docs/security/capabilities.md)"),
            remediation: Some(
                "for adversarial workloads run hector under Linux where namespace isolation enforces `network: false`"
                    .into(),
            ),
        },
    }
}

/// Adapter presence is best-effort: missing `~/.claude/settings.json`
/// is `warn` (not every user runs Claude Code); present-without-hector
/// is `warn`; wired is `pass`. Never `fail` — hector is editor-agnostic
/// and the CLI is fully usable without an adapter.
fn check_adapter() -> CheckResult {
    let Some((path, settings)) = load_claude_settings() else {
        return CheckResult {
            name: "adapter",
            status: Status::Warn,
            detail: "Claude Code adapter not detected (~/.claude/settings.json missing)".into(),
            remediation: Some(
                "if you use Claude Code, install the adapter — see docs/adapters/claude-code.md"
                    .into(),
            ),
        };
    };
    if claude_hook_wired(&settings) {
        CheckResult {
            name: "adapter",
            status: Status::Pass,
            detail: format!(
                "Claude Code PostToolUse hook references hector ({})",
                path.display()
            ),
            remediation: None,
        }
    } else {
        CheckResult {
            name: "adapter",
            status: Status::Warn,
            detail: format!("{} present but no PostToolUse hook references hector", path.display()),
            remediation: Some(
                "install the adapter or add a PostToolUse entry calling hector — see docs/adapters/claude-code.md".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_is_zero_when_all_pass_or_warn() {
        let report = Report {
            hector_version: "0".into(),
            checks: vec![
                CheckResult {
                    name: "a",
                    status: Status::Pass,
                    detail: "".into(),
                    remediation: None,
                },
                CheckResult {
                    name: "b",
                    status: Status::Warn,
                    detail: "".into(),
                    remediation: None,
                },
            ],
        };
        assert_eq!(exit_code(&report), 0);
    }

    #[test]
    fn exit_code_is_one_when_any_fail() {
        let report = Report {
            hector_version: "0".into(),
            checks: vec![
                CheckResult {
                    name: "a",
                    status: Status::Pass,
                    detail: "".into(),
                    remediation: None,
                },
                CheckResult {
                    name: "b",
                    status: Status::Fail,
                    detail: "boom".into(),
                    remediation: Some("fix it".into()),
                },
            ],
        };
        assert_eq!(exit_code(&report), 1);
    }

    #[test]
    fn check_binary_reports_running_version() {
        let r = check_binary();
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains(env!("CARGO_PKG_VERSION")));
    }

    use std::fs;
    use tempfile::tempdir;

    fn ctx_with(dir: &std::path::Path) -> DoctorContext {
        DoctorContext {
            dir: dir.to_path_buf(),
            config_path: dir.join(".hector.yml"),
        }
    }

    #[test]
    fn config_present_pass_when_file_exists() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
            "schema_version: 2\nrules: {}\n",
        )
        .unwrap();
        let r = check_config_present(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn config_present_fail_when_file_missing() {
        let d = tempdir().unwrap();
        let r = check_config_present(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.unwrap().contains("hector init"));
    }

    #[test]
    fn parses_fail_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_config_parses(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn schema_pass_on_v2() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
            "schema_version: 2\nrules: {}\n",
        )
        .unwrap();
        assert_eq!(
            check_schema_version(&ctx_with(d.path())).status,
            Status::Pass
        );
    }

    #[test]
    fn schema_fail_on_v1_with_migrate_hint() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
            "schema_version: 1\nrules: {}\n",
        )
        .unwrap();
        let r = check_schema_version(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.unwrap().contains("hector migrate"));
    }

    #[test]
    fn schema_fail_on_unsupported_version() {
        let d = tempdir().unwrap();
        fs::write(
            d.path().join(".hector.yml"),
            "schema_version: 99\nrules: {}\n",
        )
        .unwrap();
        let r = check_schema_version(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn schema_fail_on_missing_version() {
        let d = tempdir().unwrap();
        fs::write(d.path().join(".hector.yml"), "rules: {}\n").unwrap();
        let r = check_schema_version(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn trust_warn_when_config_missing() {
        let d = tempdir().unwrap();
        let r = check_trust(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn engines_pass_when_no_llm_rules() {
        let d = tempdir().unwrap();
        let trusted = hector_core::trust::write_trust_block(
            "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n",
        ).unwrap();
        fs::write(d.path().join(".hector.yml"), trusted).unwrap();
        let r = check_engines(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn engines_pass_for_ollama_without_key() {
        let cfg = hector_core::config::LlmConfig {
            provider: "ollama".into(),
            model: Some("llama3".into()),
            evaluator_model: None,
            api_key_env: None,
            base_url: None,
        };
        let r = llm_block_status(Some(&cfg));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn engines_warn_when_api_key_env_unset() {
        let cfg = hector_core::config::LlmConfig {
            provider: "anthropic".into(),
            model: Some("claude".into()),
            evaluator_model: None,
            api_key_env: Some("HECTOR_DOCTOR_TEST_DEFINITELY_UNSET_AAA".into()),
            base_url: None,
        };
        std::env::remove_var("HECTOR_DOCTOR_TEST_DEFINITELY_UNSET_AAA");
        let r = llm_block_status(Some(&cfg));
        assert_eq!(r.status, Status::Warn);
        assert!(r
            .remediation
            .unwrap()
            .contains("HECTOR_DOCTOR_TEST_DEFINITELY_UNSET_AAA"));
    }

    #[test]
    fn engines_pass_when_api_key_env_set() {
        let cfg = hector_core::config::LlmConfig {
            provider: "anthropic".into(),
            model: Some("claude".into()),
            evaluator_model: None,
            api_key_env: Some("HECTOR_DOCTOR_TEST_PRESENT_KEY".into()),
            base_url: None,
        };
        std::env::set_var("HECTOR_DOCTOR_TEST_PRESENT_KEY", "x");
        let r = llm_block_status(Some(&cfg));
        std::env::remove_var("HECTOR_DOCTOR_TEST_PRESENT_KEY");
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn engines_warn_when_api_key_env_field_missing() {
        let cfg = hector_core::config::LlmConfig {
            provider: "anthropic".into(),
            model: Some("claude".into()),
            evaluator_model: None,
            api_key_env: None,
            base_url: None,
        };
        let r = llm_block_status(Some(&cfg));
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn scope_globs_pass_on_clean_config() {
        let d = tempdir().unwrap();
        let trusted = hector_core::trust::write_trust_block(
            "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: error\n    script: \"true\"\n",
        ).unwrap();
        fs::write(d.path().join(".hector.yml"), trusted).unwrap();
        let r = check_scope_globs(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn hook_wired_finds_hector_command() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"hector check"}]}]}}"#,
        ).unwrap();
        assert!(claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_finds_adapter_hook_sh() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"$ROOT/hooks/hook.sh post"}]}]}}"#,
        ).unwrap();
        assert!(claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_unrelated_command() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        assert!(!claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_missing_post_tool_use() {
        let v: serde_json::Value = serde_json::from_str(r#"{"hooks":{}}"#).unwrap();
        assert!(!claude_hook_wired(&v));
    }

    #[test]
    fn hook_wired_rejects_empty_object() {
        let v: serde_json::Value = serde_json::from_str(r"{}").unwrap();
        assert!(!claude_hook_wired(&v));
    }

    // The `capabilities` row reflects the platform's sandbox story.
    // Linux passes (CLONE_NEWNET enforces network: false); non-Linux warns
    // (sandbox is best-effort, runtime never blocks the script).
    #[test]
    fn capabilities_pass_on_linux_warn_elsewhere() {
        let r = check_capabilities();
        assert_eq!(r.name, "capabilities");
        #[cfg(target_os = "linux")]
        {
            assert_eq!(r.status, Status::Pass);
            assert!(r.remediation.is_none(), "pass rows carry no remediation");
            assert!(
                r.detail.contains("namespace"),
                "linux detail should mention namespaces: {}",
                r.detail
            );
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(
                r.status,
                Status::Warn,
                "non-linux platforms must warn (sandbox is best-effort)"
            );
            assert!(
                r.detail.contains("best-effort"),
                "non-linux detail should mention best-effort: {}",
                r.detail
            );
            assert!(
                r.detail.contains("docs/security/capabilities.md"),
                "non-linux detail should point at the security doc: {}",
                r.detail
            );
            assert!(
                r.remediation.is_some(),
                "warn rows must carry a remediation hint"
            );
        }
    }

    #[test]
    fn runtime_state_pass_creates_hector_dir() {
        let d = tempdir().unwrap();
        let r = check_runtime_state(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
        assert!(
            d.path().join(".hector").is_dir(),
            "hector dir created by probe"
        );
    }

    #[test]
    fn runtime_state_reports_existing_state_files_sizes() {
        let d = tempdir().unwrap();
        let h = d.path().join(".hector");
        fs::create_dir_all(&h).unwrap();
        fs::write(h.join("baseline.json"), "[]").unwrap();
        fs::write(h.join("log.jsonl"), "{}\n").unwrap();
        let r = check_runtime_state(&ctx_with(d.path()));
        assert_eq!(r.status, Status::Pass);
        assert!(r.detail.contains("baseline.json"));
        assert!(r.detail.contains("log.jsonl"));
    }
}
