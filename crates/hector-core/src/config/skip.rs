//! File-skip patterns: built-in defaults + project `skip:` list + user-global ignore file.
//!
//! Mirrors bully's `src/bully/config/skip.py`. Files matched by any pattern
//! short-circuit `HectorEngine::check` — no rules evaluated, no LLM dispatched.

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

/// Filename loaded from `$HOME` to add user-global skip globs.
pub const USER_GLOBAL_IGNORE_FILENAME: &str = ".hector-ignore";

/// Files we never want to lint — lockfiles, minified bundles, generated code,
/// and the usual build/dependency directories.
pub fn built_in_skip_globs() -> &'static [&'static str] {
    &[
        // Lockfiles
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "poetry.lock",
        "Pipfile.lock",
        // Minified assets
        "*.min.js",
        "*.min.css",
        // Build / dependency directories
        "dist/**",
        "build/**",
        "__pycache__/**",
        "node_modules/**",
        "target/**",
        ".next/**",
        ".nuxt/**",
        // Generated markers
        "*.generated.*",
        "*.pb.go",
        "*.g.dart",
        "*.freezed.dart",
    ]
}

/// Right-anchored skip matcher. A bare pattern like `*.lock` matches at any
/// depth — same convention as [`crate::config::scope::ScopeMatcher`].
pub struct SkipMatcher {
    set: GlobSet,
    /// Raw pattern for each glob added, in insertion order (see ScopeMatcher).
    patterns: Vec<String>,
}

impl SkipMatcher {
    /// Build a matcher from the built-in patterns plus any extras the caller
    /// provides (project `skip:` list, `~/.hector-ignore` entries, etc.).
    pub fn with_built_ins(extras: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        let mut patterns = Vec::new();
        for g in built_in_skip_globs() {
            add_glob(&mut b, g, &mut patterns)?;
        }
        for g in extras {
            add_glob(&mut b, g, &mut patterns)?;
        }
        Ok(Self {
            set: b.build()?,
            patterns,
        })
    }

    pub fn matches<P: AsRef<Path>>(&self, path: P) -> bool {
        self.set.is_match(path.as_ref())
    }

    /// The first skip pattern (in construction order: built-ins then extras)
    /// that matches `path`, or `None`. Single source of truth for "which skip
    /// glob matched".
    pub fn matched_pattern<P: AsRef<Path>>(&self, path: P) -> Option<&str> {
        self.set
            .matches(path.as_ref())
            .into_iter()
            .min()
            .map(|i| self.patterns[i].as_str())
    }
}

fn add_glob(b: &mut GlobSetBuilder, raw: &str, patterns: &mut Vec<String>) -> Result<()> {
    let glob = Glob::new(raw).with_context(|| format!("invalid skip glob: {raw}"))?;
    b.add(glob);
    patterns.push(raw.to_string());
    if !raw.contains('/') {
        let prefixed = format!("**/{raw}");
        let glob =
            Glob::new(&prefixed).with_context(|| format!("invalid skip glob: {prefixed}"))?;
        b.add(glob);
        patterns.push(raw.to_string());
    } else if let Some(prefix) = raw.strip_suffix("/**") {
        // `node_modules/**` should also match `packages/web/node_modules/bar.js`.
        // Mirrors bully's path-component check for `/**`-suffixed patterns.
        if !prefix.is_empty() && !prefix.contains('*') {
            let any_depth = format!("**/{prefix}/**");
            let glob =
                Glob::new(&any_depth).with_context(|| format!("invalid skip glob: {any_depth}"))?;
            b.add(glob);
            patterns.push(raw.to_string());
        }
    }
    Ok(())
}

/// Parse the contents of `~/.hector-ignore` into a list of globs.
/// Blank lines and `#` comments are dropped; lines are trimmed.
pub fn parse_user_global_ignore(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn built_in_skip_globs_contains_lockfiles() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"Cargo.lock"));
        assert!(globs.contains(&"package-lock.json"));
        assert!(globs.contains(&"yarn.lock"));
        assert!(globs.contains(&"pnpm-lock.yaml"));
        assert!(globs.contains(&"poetry.lock"));
        assert!(globs.contains(&"bun.lock"));
        assert!(globs.contains(&"Pipfile.lock"));
    }

    #[test]
    fn built_in_skip_globs_contains_minified_assets() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"*.min.js"));
        assert!(globs.contains(&"*.min.css"));
    }

    #[test]
    fn built_in_skip_globs_contains_build_dirs() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"dist/**"));
        assert!(globs.contains(&"build/**"));
        assert!(globs.contains(&"__pycache__/**"));
        assert!(globs.contains(&"node_modules/**"));
        assert!(globs.contains(&"target/**"));
        assert!(globs.contains(&".next/**"));
        assert!(globs.contains(&".nuxt/**"));
    }

    #[test]
    fn built_in_skip_globs_contains_generated_markers() {
        let globs = built_in_skip_globs();
        assert!(globs.contains(&"*.generated.*"));
        assert!(globs.contains(&"*.pb.go"));
        assert!(globs.contains(&"*.g.dart"));
        assert!(globs.contains(&"*.freezed.dart"));
    }

    #[test]
    fn matcher_skips_cargo_lock_at_root() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(m.matches(Path::new("Cargo.lock")));
    }

    #[test]
    fn matcher_skips_cargo_lock_in_subdir() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(m.matches(Path::new("crates/hector-core/Cargo.lock")));
    }

    #[test]
    fn matcher_skips_node_modules_dir_recursively() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(m.matches(Path::new("node_modules/foo/index.js")));
        assert!(m.matches(Path::new("packages/web/node_modules/bar.js")));
    }

    #[test]
    fn matcher_does_not_skip_normal_source() {
        let m = SkipMatcher::with_built_ins(&[]).unwrap();
        assert!(!m.matches(Path::new("src/main.rs")));
        assert!(!m.matches(Path::new("crates/hector-core/src/runner.rs")));
        assert!(!m.matches(Path::new("README.md")));
    }

    #[test]
    fn matcher_honors_extra_user_globs() {
        let m = SkipMatcher::with_built_ins(&["*.snap".into(), "fixtures/**".into()]).unwrap();
        assert!(m.matches(Path::new("tests/foo.snap")));
        assert!(m.matches(Path::new("crates/x/tests/bar.snap")));
        assert!(m.matches(Path::new("fixtures/large.json")));
    }

    #[test]
    fn matched_pattern_reports_author_glob() {
        let m = SkipMatcher::with_built_ins(&["fixtures/**".into()]).unwrap();
        assert_eq!(
            m.matched_pattern(Path::new("Cargo.lock")),
            Some("Cargo.lock")
        );
        assert_eq!(
            m.matched_pattern(Path::new("crates/x/Cargo.lock")),
            Some("Cargo.lock")
        );
        assert_eq!(
            m.matched_pattern(Path::new("fixtures/large.json")),
            Some("fixtures/**")
        );
        assert_eq!(m.matched_pattern(Path::new("src/main.rs")), None);
    }

    #[test]
    fn parse_user_global_ignore_strips_blanks_and_comments() {
        let raw = "\
# my ignore file
*.snap

# blank lines above and below allowed

  *.bak
fixtures/**
";
        let globs = parse_user_global_ignore(raw);
        assert_eq!(
            globs,
            vec![
                "*.snap".to_string(),
                "*.bak".to_string(),
                "fixtures/**".to_string()
            ]
        );
    }

    #[test]
    fn parse_user_global_ignore_empty_input_is_empty_vec() {
        assert!(parse_user_global_ignore("").is_empty());
        assert!(parse_user_global_ignore("\n\n#only comments\n\n").is_empty());
    }
}
