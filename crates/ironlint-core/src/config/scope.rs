use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

#[derive(Clone)]
pub struct ScopeMatcher {
    set: GlobSet,
}

impl ScopeMatcher {
    pub fn new(globs: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for g in globs {
            // Bully's matcher is right-anchored: bare `*.py` should match at any depth.
            // globset treats `*.py` as matching `.py` at any depth iff the input
            // is the filename only. We pre-compute by also adding `**/<pattern>`
            // when the pattern has no slash.
            let glob = Glob::new(g).with_context(|| format!("invalid glob: {g}"))?;
            b.add(glob);
            if !g.contains('/') {
                let prefixed = format!("**/{}", g);
                let glob =
                    Glob::new(&prefixed).with_context(|| format!("invalid glob: {prefixed}"))?;
                b.add(glob);
            }
        }
        let set = b.build()?;
        Ok(Self { set })
    }

    pub fn matches<P: AsRef<Path>>(&self, path: P) -> bool {
        self.set.is_match(path.as_ref())
    }
}

// Property-based tests over `ScopeMatcher`'s glob-matching invariants.
//
// The design (docs/superpowers/specs/2026-07-04-phase-5-code-heavy-design.md
// §5.26) originally listed a fourth "negation correctness" property (a
// negated glob `!vendor/` excludes what its positive form includes). That
// property was DROPPED: `ScopeMatcher` has no negation support — it only ORs
// globs into a single `GlobSet`, so `!vendor/` would be built as a literal
// glob matching paths that literally start with `!vendor/`. Testing negation
// would either fail against current behavior or tempt adding an unrequested
// feature. It is replaced below by an extension-/name-specificity property
// that instead pins non-match soundness (the matcher doesn't over-match).
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// A single path/glob component: `[a-z]{1,8}`. Restricted to a safe
    /// alphabet (no `.`, `/`, `*`, or other glob metacharacters) so every
    /// generated string is simultaneously a valid path component and a valid
    /// literal glob.
    fn component() -> impl Strategy<Value = String> {
        "[a-z]{1,8}"
    }

    /// A file extension: `[a-z]{1,5}`. Same safe alphabet as `component`, so
    /// `name.ext` always has exactly one, unambiguous extension.
    fn extension() -> impl Strategy<Value = String> {
        "[a-z]{1,5}"
    }

    /// A directory prefix of 0..5 components, to be joined with `/`.
    fn dir_prefix() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(component(), 0..5)
    }

    /// Joins a directory prefix and a leaf name into a `/`-separated relative
    /// path (no leading `/`, no `.`/`..` segments).
    fn join_path(dirs: &[String], leaf: &str) -> String {
        let mut parts = dirs.to_vec();
        parts.push(leaf.to_string());
        parts.join("/")
    }

    proptest! {
        // Property 1 (design #1) — reflexivity: a matcher built from a
        // relative path used verbatim as a literal glob matches that exact
        // path, at any depth.
        #[test]
        fn reflexivity_literal_self_match(
            dirs in dir_prefix(),
            name in component(),
            ext in extension(),
        ) {
            let leaf = format!("{name}.{ext}");
            let path = join_path(&dirs, &leaf);
            let matcher = ScopeMatcher::new(std::slice::from_ref(&path)).unwrap();
            prop_assert!(matcher.matches(&path));
        }

        // Property 2 (design #2) — bare-pattern depth invariance: a bare
        // (no `/`) glob matches its own leaf name under any directory prefix
        // (0..=4 components). This is the property that exercises the
        // documented `**/<glob>` augmentation in `ScopeMatcher::new` — see
        // the RED/GREEN note in the task report.
        //
        // NOTE: the glob here is the literal leaf name itself (no `*`
        // wildcard), not a wildcard-extension pattern like `*.{ext}`. A
        // wildcard glob would pass this property regardless of the
        // augmentation, because globset's `Glob` defaults to
        // `literal_separator: false` — a bare `*` already crosses `/` on its
        // own (verified directly against globset 0.4.18, the version pinned
        // in Cargo.lock: `Glob::new("*.py").compile_matcher().is_match("a/b/x.py")`
        // is `true` even with the augmentation removed). A *literal* bare
        // pattern has no metacharacter able to cross `/`, so it depends
        // entirely on `ScopeMatcher::new` adding the `**/<glob>` variant —
        // making it the generator that actually pins the documented
        // divergence and produces a genuine RED when that augmentation is
        // disabled.
        #[test]
        fn bare_pattern_depth_invariance(
            dirs in dir_prefix(),
            name in component(),
            ext in extension(),
        ) {
            let leaf = format!("{name}.{ext}");
            let path = join_path(&dirs, &leaf);
            let matcher = ScopeMatcher::new(&[leaf]).unwrap();
            prop_assert!(matcher.matches(&path));
        }

        // Property 3 (design #4) — determinism / idempotence: matching is a
        // pure function of (globs, path). The same matcher returns the same
        // answer across repeated calls, and two matchers independently built
        // from identical globs agree.
        #[test]
        fn determinism_idempotence(
            dirs in dir_prefix(),
            name in component(),
            ext in extension(),
        ) {
            let glob = format!("*.{ext}");
            let leaf = format!("{name}.{ext}");
            let path = join_path(&dirs, &leaf);

            let matcher = ScopeMatcher::new(std::slice::from_ref(&glob)).unwrap();
            let first = matcher.matches(&path);
            let second = matcher.matches(&path);
            prop_assert_eq!(first, second);

            let other = ScopeMatcher::new(&[glob]).unwrap();
            prop_assert_eq!(first, other.matches(&path));
        }

        // Property 4 — extension-/name-specificity (replaces the dropped
        // negation property): the matcher does not over-match.
        //   - a bare `*.{e2}` glob does not match a `name.{e1}` file for
        //     disjoint extensions e1 != e2.
        //   - a literal glob for single-component name `a` does not match a
        //     distinct single-component name `b` (the `**/a` augmentation
        //     still can't match a bare `b`).
        #[test]
        fn extension_and_name_specificity(
            name in component(),
            e1 in extension(),
            e2 in extension(),
            a in component(),
            b in component(),
        ) {
            prop_assume!(e1 != e2);
            let file_e1 = format!("{name}.{e1}");
            let matcher = ScopeMatcher::new(&[format!("*.{e2}")]).unwrap();
            prop_assert!(!matcher.matches(&file_e1));

            prop_assume!(a != b);
            let matcher = ScopeMatcher::new(&[a]).unwrap();
            prop_assert!(!matcher.matches(&b));
        }
    }
}
