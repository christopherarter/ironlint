use super::Options;
use anyhow::{anyhow, Result};
use hector_core::adapter::{
    all_harnesses, detect, install, install_skill, uninstall, uninstall_skill, AdapterEnv, Harness,
    InstallResult, Scope,
};
use std::io::{IsTerminal, Write};

pub fn run_hook_phase(env: &AdapterEnv, opts: &Options) -> Result<i32> {
    let scope = if opts.global {
        Scope::Global
    } else {
        Scope::Local
    };
    let names = choose_harnesses(env, opts)?;
    if names.is_empty() {
        return Ok(0);
    }
    let registry = all_harnesses();
    let selected: Vec<&Harness> = names
        .iter()
        .filter_map(|n| registry.iter().find(|h| h.name == n))
        .collect();

    let mut any_ok = false;
    let mut any_fail = false;
    for h in selected {
        // 1. Hook.
        let outcome = if opts.uninstall {
            uninstall(h, env, scope, opts.dry_run)
        } else {
            install(h, env, scope, opts.dry_run)
        };
        match outcome {
            Ok(o) => {
                any_ok = true;
                print_outcome(o.harness, &o.result, o.hint, opts.uninstall);
            }
            Err(e) => {
                any_fail = true;
                println!("  {:<12} failed: {e:#}", h.name);
            }
        }
        // 2. Authoring skill. Uninstall removes every harness's own dir; install
        //    dedups opencode against claude-code's copy.
        let (skill_ok, skill_fail) = run_skill_step(h, env, scope, opts, &names);
        if skill_ok {
            any_ok = true;
        }
        if skill_fail {
            any_fail = true;
        }
    }
    Ok(if any_fail && !any_ok { 3 } else { 0 })
}

/// Resolve the harness set: explicit `--harness`, else detect+confirm.
fn choose_harnesses(env: &AdapterEnv, opts: &Options) -> Result<Vec<String>> {
    if !opts.harnesses.is_empty() {
        return select_harness_names(&opts.harnesses);
    }
    let detected: Vec<String> = detect(env)
        .into_iter()
        .filter(|(_, found)| *found)
        .map(|(n, _)| n.to_string())
        .collect();
    if detected.is_empty() {
        println!(
            "no supported harnesses detected; run `hector init --harness all` to wire all four"
        );
        return Ok(vec![]);
    }
    if opts.yes {
        return Ok(detected);
    }
    if !std::io::stdin().is_terminal() {
        println!(
            "detected: {} — re-run with `--yes` or `--harness <name>` to install",
            detected.join(", ")
        );
        return Ok(vec![]);
    }
    print!("Install hector hooks into {}? [Y/n] ", detected.join(", "));
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(if parse_confirm(&line) {
        detected
    } else {
        vec![]
    })
}

/// Validate explicit `--harness` names; `all` expands to the full registry.
fn select_harness_names(requested: &[String]) -> Result<Vec<String>> {
    let known: Vec<&'static str> = all_harnesses().iter().map(|h| h.name).collect();
    let mut out: Vec<String> = Vec::new();
    for r in requested {
        if r == "all" {
            return Ok(known.iter().map(|s| s.to_string()).collect());
        }
        if !known.contains(&r.as_str()) {
            return Err(anyhow!(
                "unknown harness `{r}` (supported: {})",
                known.join(", ")
            ));
        }
        if !out.contains(r) {
            out.push(r.clone());
        }
    }
    Ok(out)
}

fn parse_confirm(line: &str) -> bool {
    let a = line.trim().to_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

/// Pure formatter for a per-harness outcome line (or lines, for dry-run).
fn format_outcome(
    harness: &str,
    result: &InstallResult,
    hint: &str,
    uninstalling: bool,
) -> Vec<String> {
    match result {
        InstallResult::Installed if uninstalling => vec![format!("  {harness:<12} removed")],
        InstallResult::Installed => vec![format!("  {harness:<12} installed — {hint}")],
        InstallResult::Updated => vec![format!("  {harness:<12} updated — {hint}")],
        InstallResult::AlreadyPresent => vec![format!("  {harness:<12} already present")],
        InstallResult::Skipped(why) => vec![format!("  {harness:<12} skipped: {why}")],
        InstallResult::Failed(why) => vec![format!("  {harness:<12} failed: {why}")],
        InstallResult::DryRun(plan) => {
            let mut lines = vec![format!("  {harness:<12} dry-run:")];
            lines.extend(plan.iter().map(|l| format!("      {l}")));
            lines
        }
    }
}

fn print_outcome(harness: &str, result: &InstallResult, hint: &str, uninstalling: bool) {
    for line in format_outcome(harness, result, hint, uninstalling) {
        println!("{line}");
    }
}

/// opencode is Claude-compatible and also reads `.claude/skills/`; when
/// claude-code is in the same install set, skip opencode's own skill write so
/// opencode doesn't load the same-named skill twice. Dedup applies to install
/// only.
fn should_install_skill(name: &str, selected: &[String]) -> bool {
    !(name == "opencode" && selected.iter().any(|n| n == "claude-code"))
}

fn format_skill_outcome(harness: &str, result: &InstallResult, uninstalling: bool) -> Vec<String> {
    match result {
        InstallResult::Installed if uninstalling => vec![format!("  {harness:<12} skill removed")],
        InstallResult::Installed => vec![format!("  {harness:<12} skill installed")],
        InstallResult::Updated => vec![format!("  {harness:<12} skill updated")],
        InstallResult::AlreadyPresent => vec![format!("  {harness:<12} skill already present")],
        InstallResult::Skipped(why) => vec![format!("  {harness:<12} skill skipped: {why}")],
        InstallResult::Failed(why) => vec![format!("  {harness:<12} skill failed: {why}")],
        InstallResult::DryRun(plan) => {
            let mut lines = vec![format!("  {harness:<12} skill dry-run:")];
            lines.extend(plan.iter().map(|l| format!("      {l}")));
            lines
        }
    }
}

fn print_skill_outcome(harness: &str, result: &InstallResult, uninstalling: bool) {
    for line in format_skill_outcome(harness, result, uninstalling) {
        println!("{line}");
    }
}

/// Run the authoring-skill install or uninstall for one harness.
/// Returns `(any_ok, any_fail)` so the caller can fold into its accumulators.
/// Returns `(false, false)` when the skill step is skipped by dedup.
fn run_skill_step(
    h: &Harness,
    env: &AdapterEnv,
    scope: Scope,
    opts: &Options,
    names: &[String],
) -> (bool, bool) {
    let do_skill = opts.uninstall || should_install_skill(h.name, names);
    if !do_skill {
        return (false, false);
    }
    let s = if opts.uninstall {
        uninstall_skill(h, env, scope, opts.dry_run)
    } else {
        install_skill(h, env, scope, opts.dry_run)
    };
    match s {
        Ok(o) => {
            print_skill_outcome(o.harness, &o.result, opts.uninstall);
            (true, false)
        }
        Err(e) => {
            println!("  {:<12} skill failed: {e:#}", h.name);
            (false, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_confirm_defaults_yes_on_empty() {
        assert!(parse_confirm(""));
        assert!(parse_confirm("\n"));
        assert!(parse_confirm("y"));
        assert!(parse_confirm("YES"));
    }
    #[test]
    fn parse_confirm_no() {
        assert!(!parse_confirm("n"));
        assert!(!parse_confirm("no"));
        assert!(!parse_confirm("x"));
    }

    #[test]
    fn select_explicit_all_returns_every_harness() {
        let names = select_harness_names(&["all".to_string()]).unwrap();
        assert_eq!(names, vec!["claude-code", "reasonix", "pi", "opencode"]);
    }
    #[test]
    fn select_explicit_unknown_errors() {
        assert!(select_harness_names(&["bogus".to_string()]).is_err());
    }
    #[test]
    fn select_explicit_dedup_and_order() {
        let names = select_harness_names(&["pi".to_string(), "pi".to_string()]).unwrap();
        assert_eq!(names, vec!["pi"]);
    }

    #[test]
    fn format_outcome_covers_every_variant() {
        use hector_core::adapter::InstallResult::*;
        assert!(format_outcome("reasonix", &Installed, "h", false)[0].contains("installed"));
        assert!(format_outcome("reasonix", &Installed, "h", true)[0].contains("removed"));
        assert!(format_outcome("pi", &Updated, "h", false)[0].contains("updated"));
        assert!(format_outcome("pi", &AlreadyPresent, "h", false)[0].contains("already present"));
        assert!(
            format_outcome("pi", &Skipped("x".to_string()), "h", false)[0].contains("skipped: x")
        );
        assert!(
            format_outcome("pi", &Failed("y".to_string()), "h", false)[0].contains("failed: y")
        );
        let dr = format_outcome("pi", &DryRun(vec!["write a".to_string()]), "h", false);
        assert_eq!(dr.len(), 2);
        assert!(dr[0].contains("dry-run"));
        assert!(dr[1].contains("write a"));
    }

    #[test]
    fn dedup_skips_opencode_skill_when_claude_present() {
        let sel = vec!["claude-code".to_string(), "opencode".to_string()];
        assert!(!should_install_skill("opencode", &sel));
        assert!(should_install_skill("claude-code", &sel));
        assert!(should_install_skill("pi", &sel));
    }

    #[test]
    fn dedup_installs_opencode_skill_when_claude_absent() {
        let sel = vec!["opencode".to_string(), "pi".to_string()];
        assert!(should_install_skill("opencode", &sel));
    }

    #[test]
    fn format_skill_outcome_covers_variants() {
        use hector_core::adapter::InstallResult::*;
        assert!(format_skill_outcome("pi", &Installed, false)[0].contains("skill installed"));
        assert!(format_skill_outcome("pi", &Installed, true)[0].contains("skill removed"));
        assert!(format_skill_outcome("pi", &Updated, false)[0].contains("skill updated"));
        assert!(
            format_skill_outcome("pi", &AlreadyPresent, false)[0].contains("skill already present")
        );
        let dr = format_skill_outcome("pi", &DryRun(vec!["write a".to_string()]), false);
        assert_eq!(dr.len(), 2);
        assert!(dr[0].contains("skill dry-run"));
        assert!(dr[1].contains("write a"));
    }
}
