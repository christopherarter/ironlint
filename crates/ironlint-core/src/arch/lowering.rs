use crate::config::types::{Check, Config, Lifecycle};
use anyhow::{bail, Result};

/// Lower an `architecture:` config block into a synthetic `__arch__` check.
///
/// The check's `run` shells out to `ironlint arch check`, which reads the
/// layers file from `$IRONLINT_ARCH_LAYERS` (materialized by the runner in a
/// later task). This keeps the runner pure: architecture enforcement becomes
/// an ordinary check that flows through the same `gate::run_gate` path as any
/// other.
pub fn lower_architecture(cfg: &mut Config) -> Result<()> {
    if cfg.architecture.is_none() {
        return Ok(());
    }
    if cfg.checks.contains_key("__arch__") {
        bail!("reserved check id `__arch__` is reserved for the architecture: block");
    }
    let arch = cfg.architecture.take().expect("checked above");
    arch.validate()?;
    let yaml = serde_yaml::to_string(&arch).unwrap_or_default();
    let run = "ironlint arch check --layers \"$IRONLINT_ARCH_LAYERS\" --root \"$IRONLINT_ROOT\" ${IRONLINT_EVENT:+--event \"$IRONLINT_EVENT\"} ${IRONLINT_FILE:+--file \"$IRONLINT_FILE\"}".to_string();
    cfg.checks.insert(
        "__arch__".to_string(),
        Check {
            files: vec!["**/*".to_string()],
            run: Some(run),
            steps: None,
            on: vec![Lifecycle::Write, Lifecycle::PreCommit],
            name: Some("architecture".to_string()),
        },
    );
    cfg.arch_layers_yaml = Some(yaml);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_architecture_block_returns_ok_without_inserting_check() {
        let mut cfg: Config =
            serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
        lower_architecture(&mut cfg).unwrap();
        assert!(!cfg.checks.contains_key("__arch__"));
        assert!(cfg.arch_layers_yaml.is_none());
    }
}
