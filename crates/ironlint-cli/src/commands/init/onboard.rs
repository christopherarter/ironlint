use super::render::{render_plan, HarnessPlan, Source};
use super::select;
use super::Options;
use anyhow::{anyhow, Result};
use ironlint_core::adapter::{
    all_harnesses, detect, install, install_skill, plan_install, plan_uninstall, uninstall,
    uninstall_skill, AdapterEnv, Harness, InstallResult, PlanStep, Scope,
};
use std::io::{IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proceed {
    Yes,
    No,
}

pub fn run_hook_phase(env: &AdapterEnv, opts: &Options) -> Result<i32> {
    let scope = if opts.global {
        Scope::Global
    } else {
        Scope::Local
    };
    let mut selected = resolve_harnesses(env, opts)?;
    if selected.is_empty() && !std::io::stdin().is_terminal() {
        println!(
            "no supported harnesses detected; run `ironlint init --harness all` to wire all four"
        );
        return Ok(0);
    }
    let plans = build_plans(&selected, env, scope, opts.uninstall);
    print!(
        "{}",
        render_plan(&plans, opts.uninstall, env, std::io::stdout().is_terminal())
    );
    if opts.dry_run {
        return Ok(0);
    }
    match confirm_gate(opts, &mut selected)? {
        Proceed::No => return Ok(0),
        Proceed::Yes => {}
    }
    // Rebuild plans from the possibly mutated selection before applying; this
    // validates the new set and mirrors the spec contract even though `apply`
    // recomputes its own plan internally.
    let _ = build_plans(&selected, env, scope, opts.uninstall);
    Ok(apply(&selected, env, scope, opts))
}

/// Resolve the harness set and tag each with why it is present. Explicit
/// `--harness` → `requested`; auto-detect → `detected`. No prompting here.
fn resolve_harnesses(env: &AdapterEnv, opts: &Options) -> Result<Vec<(String, Source)>> {
    if !opts.harnesses.is_empty() {
        let names = select_harness_names(&opts.harnesses)?;
        return Ok(names.into_iter().map(|n| (n, Source::Requested)).collect());
    }
    Ok(detect(env)
        .into_iter()
        .filter(|(_, found)| *found)
        .map(|(n, _)| (n.to_string(), Source::Detected))
        .collect())
}

/// Build `SelectItem`s from the resolved harness set. Detected harnesses are
/// pre-checked; undetected harnesses are shown but unchecked.
fn build_items(selected: &[(String, Source)]) -> Vec<select::SelectItem> {
    all_harnesses()
        .iter()
        .map(|h| {
            let is_selected = selected.iter().any(|(n, _)| n == h.name);
            select::SelectItem {
                name: h.name.to_string(),
                detected: is_selected,
                selected: is_selected,
            }
        })
        .collect()
}

/// Reconcile the names returned by the multi-select back into `(name, Source)`
/// pairs, preserving `Detected` for items that were originally detected.
fn reconcile(chosen: Vec<String>, selected: &[(String, Source)]) -> Vec<(String, Source)> {
    let originally_detected: std::collections::HashSet<String> = selected
        .iter()
        .filter(|(_, s)| matches!(s, Source::Detected))
        .map(|(n, _)| n.clone())
        .collect();
    chosen
        .into_iter()
        .map(|n| {
            let src = if originally_detected.contains(&n) {
                Source::Detected
            } else {
                Source::Requested
            };
            (n, src)
        })
        .collect()
}

/// Build the render-ready plan, honoring the opencode-skill dedup for install
/// (opencode reads claude-code's `.claude/skills/` copy).
fn build_plans(
    selected: &[(String, Source)],
    env: &AdapterEnv,
    scope: Scope,
    uninstall_mode: bool,
) -> Vec<HarnessPlan> {
    let registry = all_harnesses();
    let names: Vec<String> = selected.iter().map(|(n, _)| n.clone()).collect();
    selected
        .iter()
        .filter_map(|(name, source)| {
            let h = registry.iter().find(|h| h.name == *name)?;
            let mut steps = if uninstall_mode {
                plan_uninstall(h, env, scope)
            } else {
                plan_install(h, env, scope)
            };
            if !uninstall_mode && !should_install_skill(h.name, &names) {
                steps.retain(|s| !matches!(s, PlanStep::Skill { .. }));
            }
            Some(HarnessPlan {
                name: h.name,
                source: *source,
                steps,
            })
        })
        .collect()
}

/// Decide whether to proceed past the plan. `--yes` and explicit non-TTY
/// proceed; auto-detect non-TTY prints a hint and stops; TTY prompts.
fn confirm_gate(opts: &Options, selected: &mut Vec<(String, Source)>) -> Result<Proceed> {
    confirm_gate_to(opts, selected, &mut std::io::stdout())
}

fn confirm_gate_to<W: Write>(
    opts: &Options,
    selected: &mut Vec<(String, Source)>,
    writer: &mut W,
) -> Result<Proceed> {
    if opts.yes {
        return Ok(Proceed::Yes);
    }
    let explicit = !opts.harnesses.is_empty();
    if !std::io::stdin().is_terminal() {
        if explicit {
            return Ok(Proceed::Yes);
        }
        let names = selected
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            writer,
            "detected: {names} — re-run with `--yes` or `--harness <name>` to proceed"
        )?;
        return Ok(Proceed::No);
    }
    if opts.harnesses.is_empty() {
        let chosen = select::prompt_multi_select(build_items(selected))?;
        if chosen.is_empty() {
            writeln!(writer, "no harnesses selected; nothing to do")?;
            return Ok(Proceed::No);
        }
        *selected = reconcile(chosen, selected);
        return Ok(Proceed::Yes);
    }
    write!(writer, "  Proceed? [Y/n] ")?;
    writer.flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(if parse_confirm(&line) {
        Proceed::Yes
    } else {
        Proceed::No
    })
}

/// Install or uninstall the resolved set, printing per-harness result lines.
/// Returns the phase exit code: 3 only if every harness failed.
fn apply(selected: &[(String, Source)], env: &AdapterEnv, scope: Scope, opts: &Options) -> i32 {
    let registry = all_harnesses();
    let names: Vec<String> = selected.iter().map(|(n, _)| n.clone()).collect();
    let mut any_ok = false;
    let mut any_fail = false;
    for name in &names {
        let Some(h) = registry.iter().find(|h| h.name == *name) else {
            continue;
        };
        let outcome = if opts.uninstall {
            uninstall(h, env, scope)
        } else {
            install(h, env, scope)
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
        let (skill_ok, skill_fail) = run_skill_step(h, env, scope, opts, &names);
        any_ok |= skill_ok;
        any_fail |= skill_fail;
    }
    if any_fail && !any_ok {
        3
    } else {
        0
    }
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
        uninstall_skill(h, env, scope)
    } else {
        install_skill(h, env, scope)
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
        assert_eq!(names, vec!["claude-code", "codex", "pi", "opencode"]);
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
        use ironlint_core::adapter::InstallResult::*;
        assert!(format_outcome("codex", &Installed, "h", false)[0].contains("installed"));
        assert!(format_outcome("codex", &Installed, "h", true)[0].contains("removed"));
        assert!(format_outcome("pi", &Updated, "h", false)[0].contains("updated"));
        assert!(format_outcome("pi", &AlreadyPresent, "h", false)[0].contains("already present"));
        assert!(
            format_outcome("pi", &Skipped("x".to_string()), "h", false)[0].contains("skipped: x")
        );
        assert!(
            format_outcome("pi", &Failed("y".to_string()), "h", false)[0].contains("failed: y")
        );
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
        use ironlint_core::adapter::InstallResult::*;
        assert!(format_skill_outcome("pi", &Installed, false)[0].contains("skill installed"));
        assert!(format_skill_outcome("pi", &Installed, true)[0].contains("skill removed"));
        assert!(format_skill_outcome("pi", &Updated, false)[0].contains("skill updated"));
        assert!(
            format_skill_outcome("pi", &AlreadyPresent, false)[0].contains("skill already present")
        );
        assert!(
            format_skill_outcome("pi", &Skipped("x".to_string()), false)[0]
                .contains("skill skipped: x")
        );
        assert!(
            format_skill_outcome("pi", &Failed("y".to_string()), false)[0]
                .contains("skill failed: y")
        );
    }

    #[test]
    fn build_items_detected_are_selected() {
        let selected = vec![
            ("claude-code".to_string(), Source::Detected),
            ("codex".to_string(), Source::Detected),
        ];
        let items = build_items(&selected);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["claude-code", "codex", "pi", "opencode"]);
        assert!(items[0].detected && items[0].selected, "claude-code");
        assert!(items[1].detected && items[1].selected, "codex");
        assert!(!items[2].detected && !items[2].selected, "pi");
        assert!(!items[3].detected && !items[3].selected, "opencode");
    }

    #[test]
    fn build_items_none_detected_yields_four_unchecked() {
        let items = build_items(&[]);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["claude-code", "codex", "pi", "opencode"]);
        for item in &items {
            assert!(
                !item.detected && !item.selected,
                "{} should be undetected and unchecked",
                item.name
            );
        }
    }

    #[test]
    fn reconcile_preserves_detected() {
        let selected = vec![
            ("claude-code".to_string(), Source::Detected),
            ("codex".to_string(), Source::Detected),
        ];
        let chosen = vec!["claude-code".to_string(), "pi".to_string()];
        let result = reconcile(chosen, &selected);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "claude-code");
        assert!(matches!(result[0].1, Source::Detected));
        assert_eq!(result[1].0, "pi");
        assert!(matches!(result[1].1, Source::Requested));
    }

    #[test]
    fn confirm_gate_yes_bypasses() {
        let opts = Options {
            harnesses: vec![],
            global: false,
            yes: true,
            no_hook: false,
            hook_only: false,
            uninstall: false,
            dry_run: false,
        };
        let mut selected = vec![("claude-code".to_string(), Source::Detected)];
        let mut buf: Vec<u8> = Vec::new();
        let before = selected.clone();
        assert_eq!(
            confirm_gate_to(&opts, &mut selected, &mut buf).unwrap(),
            Proceed::Yes
        );
        assert_eq!(selected.len(), before.len());
        for ((a_name, a_src), (b_name, b_src)) in selected.iter().zip(before.iter()) {
            assert_eq!(a_name, b_name);
            assert!(matches!(
                (a_src, b_src),
                (Source::Detected, Source::Detected) | (Source::Requested, Source::Requested)
            ));
        }
    }

    #[test]
    fn confirm_gate_non_tty_auto_detect_prints_hint() {
        let opts = Options {
            harnesses: vec![],
            global: false,
            yes: false,
            no_hook: false,
            hook_only: false,
            uninstall: false,
            dry_run: false,
        };
        let mut selected = vec![("claude-code".to_string(), Source::Detected)];
        let mut buf: Vec<u8> = Vec::new();
        assert_eq!(
            confirm_gate_to(&opts, &mut selected, &mut buf).unwrap(),
            Proceed::No
        );
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("re-run with"), "hint missing: {out}");
    }

    #[test]
    fn confirm_gate_non_tty_explicit_proceeds() {
        let opts = Options {
            harnesses: vec!["codex".to_string()],
            global: false,
            yes: false,
            no_hook: false,
            hook_only: false,
            uninstall: false,
            dry_run: false,
        };
        let mut selected = vec![("codex".to_string(), Source::Requested)];
        let mut buf: Vec<u8> = Vec::new();
        assert_eq!(
            confirm_gate_to(&opts, &mut selected, &mut buf).unwrap(),
            Proceed::Yes
        );
    }
}
