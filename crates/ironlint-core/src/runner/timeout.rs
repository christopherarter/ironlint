use crate::config::Config;
use std::time::Duration;

/// Resolve the per-check timeout: `IRONLINT_TIMEOUT` (secs) overrides the config
/// value, which defaults to 30. An ambient override below
/// [`IRONLINT_TIMEOUT_FLOOR_SECS`] is raised to the floor and a loud warning is
/// emitted, so a prompt-injected short timeout cannot force every check to time
/// out (-> exit 3 -> fail-open). The base `>= 1` clamp (see
/// [`resolve_timeout_secs`]) still applies.
pub(crate) fn resolve_timeout(config: &Config) -> Duration {
    let env_val = std::env::var("IRONLINT_TIMEOUT").ok();
    let (secs, shortened) = resolve_timeout_with_floor(env_val.as_deref(), config.timeout_secs());
    if shortened {
        eprintln!(
            "ironlint: IRONLINT_TIMEOUT={} is below the {}s floor (config timeout_secs={}); \
             raising to {}s so a real check is not forced to time out. Set execution.timeout_secs \
             in .ironlint.yml to silence this for a trusted, shorter budget.",
            env_val.as_deref().unwrap_or(""),
            IRONLINT_TIMEOUT_FLOOR_SECS,
            config.timeout_secs(),
            IRONLINT_TIMEOUT_FLOOR_SECS,
        );
    }
    Duration::from_secs(secs)
}

/// Minimum seconds an ambient `IRONLINT_TIMEOUT` override is allowed to
/// impose. A maliciously short ambient value (e.g. `IRONLINT_TIMEOUT=1`
/// from a prompt-injected agent or a repo `.envrc`) would force every real
/// check to time out -> exit 3 -> fail-open; flooring it at this value keeps
/// real checks runnable. Applies ONLY to ambient overrides — an explicit
/// `execution.timeout_secs: N` in the (trusted) config is the operator's
/// choice and is not raised.
pub(crate) const IRONLINT_TIMEOUT_FLOOR_SECS: u64 = 10;

/// Pure resolver behind [`resolve_timeout`]: `env_val` (the raw
/// `IRONLINT_TIMEOUT` string, if any) wins when it parses as a `u64`;
/// otherwise `config_default` is used. The result is always clamped to
/// `>= 1`, regardless of which source it came from. Extracted as a pure
/// function (no env access) so the override + clamp behavior is unit-testable
/// without mutating process-global env state.
pub(crate) fn resolve_timeout_secs(env_val: Option<&str>, config_default: u64) -> u64 {
    env_val
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(config_default)
        .max(1)
}

/// Resolve the timeout with the ambient-override floor applied. Pure (no env
/// access) so the floor behavior is unit-testable without mutating
/// process-global env. Returns `(secs, shortened)`:
/// - `secs`: when `env_val` parses as a `u64`, the clamped seconds (via
///   [`resolve_timeout_secs`], which keeps its own `>= 1` floor) raised to
///   [`IRONLINT_TIMEOUT_FLOOR_SECS`] if below it; otherwise the parsed value.
///   When `env_val` is `None` or unparseable, `config_default` is used as-is
///   (still subject to `resolve_timeout_secs`' `>= 1` clamp) and NOT floored.
/// - `shortened`: `true` iff an ambient override was present, parseable, and
///   below the floor — the one case the caller should log loudly. Surfaced
///   as a return value so the log decision is testable without stderr capture.
pub(crate) fn resolve_timeout_with_floor(
    env_val: Option<&str>,
    config_default: u64,
) -> (u64, bool) {
    let secs = resolve_timeout_secs(env_val, config_default);
    let shortened = matches!(
        env_val.and_then(|s| s.parse::<u64>().ok()),
        Some(parsed) if parsed < IRONLINT_TIMEOUT_FLOOR_SECS
    );
    let secs = if shortened {
        secs.max(IRONLINT_TIMEOUT_FLOOR_SECS)
    } else {
        secs
    };
    (secs, shortened)
}

#[cfg(test)]
mod resolve_timeout_secs_tests {
    use super::{resolve_timeout_secs, resolve_timeout_with_floor, IRONLINT_TIMEOUT_FLOOR_SECS};

    #[test]
    fn valid_env_override_wins_over_config_default() {
        assert_eq!(resolve_timeout_secs(Some("5"), 30), 5);
    }

    #[test]
    fn zero_env_override_is_clamped_to_one() {
        assert_eq!(resolve_timeout_secs(Some("0"), 30), 1);
    }

    #[test]
    fn unparseable_env_override_falls_back_to_config_default() {
        assert_eq!(resolve_timeout_secs(Some("notanumber"), 30), 30);
    }

    #[test]
    fn unset_env_override_uses_config_default() {
        assert_eq!(resolve_timeout_secs(None, 30), 30);
    }

    #[test]
    fn config_default_itself_is_clamped_to_one() {
        // A configured `timeout_secs: 0` must also be clamped — the clamp
        // applies to whichever source (env or config) supplied the value.
        assert_eq!(resolve_timeout_secs(None, 0), 1);
    }

    #[test]
    fn floor_raises_short_ambient_override_to_minimum() {
        // An ambient IRONLINT_TIMEOUT=1 (the prompt-injection attack) is
        // floored up to IRONLINT_TIMEOUT_FLOOR_SECS so a real check is not
        // forced to time out -> exit 3 -> fail-open. `shortened` flags the
        // log should fire.
        let (secs, shortened) = resolve_timeout_with_floor(Some("1"), 30);
        assert_eq!(secs, IRONLINT_TIMEOUT_FLOOR_SECS);
        assert!(shortened, "a shortened ambient override must flag the log");
    }

    #[test]
    fn floor_passes_through_large_ambient_override() {
        // A legitimate ambient override at or above the floor passes through
        // unchanged — the floor is a minimum, not a cap — and does not flag.
        let (secs, shortened) = resolve_timeout_with_floor(Some("60"), 30);
        assert_eq!(secs, 60);
        assert!(!shortened);
    }

    #[test]
    fn floor_does_not_touch_the_config_default_path() {
        // No ambient override => the operator's config_default is used as-is
        // (still subject to the .max(1) floor inside resolve_timeout_secs,
        // but NOT raised to IRONLINT_TIMEOUT_FLOOR_SECS — that would silently
        // widen an explicit, trusted operator choice) and does not flag.
        let (secs, shortened) = resolve_timeout_with_floor(None, 5);
        assert_eq!(secs, 5);
        assert!(!shortened);
    }
}
