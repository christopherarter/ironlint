//! Pretty-printer for the `ironlint init` onboarding plan. Pure: takes structured
//! `PlanStep`s and returns the tree string. Color is TTY-gated by the caller and
//! passed in as `color`; when false, output contains no ANSI escapes.
use ironlint_core::adapter::{AdapterEnv, PlanStep};
use std::path::Path;

#[derive(Clone, Copy)]
pub enum Source {
    Detected,
    Requested,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::Requested => "requested",
        }
    }
}

pub struct HarnessPlan {
    pub name: &'static str,
    pub source: Source,
    pub steps: Vec<PlanStep>,
}

/// Minimal ANSI wrapper. `on == false` returns the input unchanged, so non-TTY
/// output is plain text.
struct Paint {
    on: bool,
}
impl Paint {
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\u{1b}[{code}m{s}\u{1b}[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
}

pub fn render_plan(
    plans: &[HarnessPlan],
    uninstall: bool,
    env: &AdapterEnv,
    color: bool,
) -> String {
    let p = Paint { on: color };
    let mut out = String::new();
    let title = if uninstall {
        "ironlint · uninstall"
    } else {
        "ironlint · onboarding"
    };
    out.push_str(&format!("\n  {}\n", p.bold(title)));
    out.push_str(&format!(
        "  {}\n\n",
        p.dim(&"─".repeat(title.chars().count()))
    ));
    for hp in plans {
        render_harness(&mut out, hp, env, &p);
    }
    out
}

fn render_harness(out: &mut String, hp: &HarnessPlan, env: &AdapterEnv, p: &Paint) {
    out.push_str(&format!(
        "  {}  {}\n",
        p.bold(&format!("{:<12}", hp.name)),
        p.green(hp.source.label())
    ));
    let last = hp.steps.len().saturating_sub(1);
    for (i, step) in hp.steps.iter().enumerate() {
        let branch = if i == last { "└" } else { "├" };
        let (kind, path) = step_parts(step);
        out.push_str(&format!(
            "    {} {} {}{}\n",
            branch,
            p.dim(&format!("{kind:<7}")),
            p.dim(&short_path(path, env)),
            patch_suffix(step, p),
        ));
    }
    out.push('\n');
}

fn step_parts(step: &PlanStep) -> (&'static str, &Path) {
    match step {
        PlanStep::Hook { path } => ("hook", path),
        PlanStep::Plugin { path } => ("plugin", path),
        PlanStep::Patch { path, .. } => ("patch", path),
        PlanStep::Skill { path } => ("skill", path),
    }
}

fn patch_suffix(step: &PlanStep, p: &Paint) -> String {
    match step {
        PlanStep::Patch { key, .. } => format!("  {} {}", p.dim("›"), p.cyan(key)),
        _ => String::new(),
    }
}

/// Project-relative (`./…`) first (more specific — a project under `$HOME`
/// should read `./`), then home-relative (`~/…`), else absolute.
fn short_path(path: &Path, env: &AdapterEnv) -> String {
    if let Ok(rel) = path.strip_prefix(&env.project_root) {
        return format!("./{}", rel.display());
    }
    if let Ok(rel) = path.strip_prefix(&env.home) {
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironlint_core::adapter::{AdapterEnv, PlanStep};
    use std::path::PathBuf;

    fn env() -> AdapterEnv {
        AdapterEnv {
            home: PathBuf::from("/home/u"),
            config_home: PathBuf::from("/home/u/.config"),
            project_root: PathBuf::from("/home/u/proj"),
        }
    }

    fn claude_plan() -> HarnessPlan {
        HarnessPlan {
            name: "claude-code",
            source: Source::Detected,
            steps: vec![
                PlanStep::Hook {
                    path: PathBuf::from("/home/u/.config/ironlint/adapters/claude-code/hook.sh"),
                },
                PlanStep::Patch {
                    path: PathBuf::from("/home/u/proj/.claude/settings.json"),
                    key: "PreToolUse",
                },
                PlanStep::Skill {
                    path: PathBuf::from("/home/u/proj/.claude/skills/ironlint-config/SKILL.md"),
                },
            ],
        }
    }

    #[test]
    fn renders_header_tag_and_tree_without_color() {
        let out = render_plan(&[claude_plan()], false, &env(), false);
        assert!(out.contains("ironlint · onboarding"), "header:\n{out}");
        assert!(out.contains("claude-code"), "harness name:\n{out}");
        assert!(out.contains("detected"), "source tag:\n{out}");
        assert!(out.contains("hook"), "hook label:\n{out}");
        assert!(out.contains("patch"), "patch label:\n{out}");
        assert!(out.contains("PreToolUse"), "patch key:\n{out}");
        assert!(out.contains("skill"), "skill label:\n{out}");
        // path shortening
        assert!(
            out.contains("~/.config/ironlint/adapters/claude-code/hook.sh"),
            "home-relative:\n{out}"
        );
        assert!(
            out.contains("./.claude/settings.json"),
            "project-relative:\n{out}"
        );
        // tree glyphs
        assert!(
            out.contains('├') && out.contains('└'),
            "tree glyphs:\n{out}"
        );
    }

    #[test]
    fn no_color_output_has_no_escape_bytes() {
        let out = render_plan(&[claude_plan()], false, &env(), false);
        assert!(
            !out.contains('\u{1b}'),
            "must be plain when color off:\n{out:?}"
        );
    }

    #[test]
    fn color_output_has_escape_bytes() {
        let out = render_plan(&[claude_plan()], false, &env(), true);
        assert!(out.contains('\u{1b}'), "must emit ANSI when color on");
    }

    #[test]
    fn requested_tag_and_uninstall_header() {
        let plan = HarnessPlan {
            name: "pi",
            source: Source::Requested,
            steps: vec![PlanStep::Plugin {
                path: PathBuf::from("/home/u/proj/.pi/extensions/ironlint.ts"),
            }],
        };
        let out = render_plan(&[plan], true, &env(), false);
        assert!(
            out.contains("ironlint · uninstall"),
            "uninstall header:\n{out}"
        );
        assert!(out.contains("requested"), "requested tag:\n{out}");
        assert!(out.contains("plugin"), "plugin label:\n{out}");
        assert!(
            out.contains("./.pi/extensions/ironlint.ts"),
            "project path:\n{out}"
        );
    }

    #[test]
    fn absolute_path_fallback_when_outside_home_and_project() {
        let plan = HarnessPlan {
            name: "opencode",
            source: Source::Requested,
            steps: vec![PlanStep::Plugin {
                path: PathBuf::from("/opt/elsewhere/ironlint.ts"),
            }],
        };
        let out = render_plan(&[plan], false, &env(), false);
        assert!(
            out.contains("/opt/elsewhere/ironlint.ts"),
            "absolute fallback:\n{out}"
        );
    }
}
