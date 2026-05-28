use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

#[derive(Clone)]
pub struct ScopeMatcher {
    set: GlobSet,
    /// Raw author pattern for each glob added to `set`, in insertion order. A
    /// bare pattern adds two entries (itself + `**/<pattern>`) that both map to
    /// the same author string, so `matched_pattern` reports what the author
    /// wrote, not the synthesized form.
    patterns: Vec<String>,
}

impl ScopeMatcher {
    pub fn new(globs: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        let mut patterns = Vec::new();
        for g in globs {
            // Bully's matcher is right-anchored: bare `*.py` should match at any depth.
            // globset treats `*.py` as matching `.py` at any depth iff the input
            // is the filename only. We pre-compute by also adding `**/<pattern>`
            // when the pattern has no slash.
            let glob = Glob::new(g).with_context(|| format!("invalid glob: {g}"))?;
            b.add(glob);
            patterns.push(g.clone());
            if !g.contains('/') {
                let prefixed = format!("**/{}", g);
                let glob =
                    Glob::new(&prefixed).with_context(|| format!("invalid glob: {prefixed}"))?;
                b.add(glob);
                patterns.push(g.clone());
            }
        }
        Ok(Self {
            set: b.build()?,
            patterns,
        })
    }

    pub fn matches<P: AsRef<Path>>(&self, path: P) -> bool {
        self.set.is_match(path.as_ref())
    }

    /// The first author-authored pattern (in declaration order) that matches
    /// `path`, or `None`. Single source of truth for "which glob matched",
    /// replacing the runner's hand-rolled re-implementation.
    pub fn matched_pattern<P: AsRef<Path>>(&self, path: P) -> Option<&str> {
        self.set
            .matches(path.as_ref())
            .into_iter()
            .min()
            .map(|i| self.patterns[i].as_str())
    }
}

#[cfg(test)]
mod matched_pattern_tests {
    use super::ScopeMatcher;
    use std::path::Path;

    #[test]
    fn reports_the_author_pattern_for_a_bare_glob_at_depth() {
        let m = ScopeMatcher::new(&["*.py".to_string()]).unwrap();
        // Bare *.py also matches at depth via the **/ form, but the reported
        // pattern must be the author's "*.py", not the synthesized "**/*.py".
        assert_eq!(
            m.matched_pattern(Path::new("src/app/main.py")),
            Some("*.py")
        );
        assert_eq!(m.matched_pattern(Path::new("main.py")), Some("*.py"));
    }

    #[test]
    fn returns_first_matching_pattern_in_declaration_order() {
        let m = ScopeMatcher::new(&["src/**".to_string(), "*.rs".to_string()]).unwrap();
        // src/lib.rs matches both; declaration order wins.
        assert_eq!(m.matched_pattern(Path::new("src/lib.rs")), Some("src/**"));
    }

    #[test]
    fn returns_none_when_nothing_matches() {
        let m = ScopeMatcher::new(&["*.py".to_string()]).unwrap();
        assert_eq!(m.matched_pattern(Path::new("README.md")), None);
    }
}
