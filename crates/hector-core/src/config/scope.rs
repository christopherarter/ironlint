use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

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
        Ok(Self { set: b.build()? })
    }

    pub fn matches<P: AsRef<Path>>(&self, path: P) -> bool {
        self.set.is_match(path.as_ref())
    }
}
