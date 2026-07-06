//! The bash-gate matcher: a pure classifier for a Bash command string.
//!
//! Decides whether a command an agent wants to run would let it free itself
//! from ironlint's gate — `ironlint trust`, or a Bash write to the policy
//! surface (`.ironlint.yml`, `.ironlint/gates/`). Pure of I/O and state; the
//! `ironlint gate-bash` subcommand and the adapter hooks are thin shims
//! around it. See `docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md`.
//!
//! Threat tier: lazy non-reasoning models. Blocks direct forms + light
//! de-obfuscation. Variable-substitution indirection is a documented known
//! gap (catching it needs real shell evaluation — adversarial tier, out of
//! scope). The test module pins both directions.

#![warn(clippy::cognitive_complexity)]

/// The bash-gate's verdict on one command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Allow the command to proceed.
    Allow,
    /// Block it; the string is the reason shown to the agent.
    Block(String),
}

/// Decide whether `command` may run.
///
/// Pure: no I/O, no state. Returns `Block(reason)` for `ironlint trust` (any
/// args) and Bash writes to the policy surface; `Allow` otherwise, including
/// the documented indirection gap (which is *intentionally* allowed).
/// The reason prefix used for every block. Adapters show the full reason to
/// the agent; keeping a stable prefix makes the contract tests robust to
/// wording changes.
const TRUST_REASON: &str = "ironlint trust must be run by a human, not by an agent";

/// Normalize a command for matching: collapse the light de-obfuscation cases
/// (backtick/`$()` delimiters around tokens, quoted binary names, runs of
/// whitespace) without attempting to evaluate the string. This is string
/// surgery, not a shell parser — variable-substitution indirection
/// (`iron$(echo lint)`) is deliberately untouched (known gap).
fn normalize(command: &str) -> String {
    // 1. Strip backtick and $() DELIMITERS while keeping their contents, so
    //    `ironlint` and $(ironlint) collapse to ironlint. Only the matched
    //    pair delimiters are removed; a stray backtick or unmatched paren is
    //    left alone (it does not denote a completed substitution).
    let mut s = String::with_capacity(command.len());
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            // skip the opening backtick; the closing one is also skipped when reached
            i += 1;
            continue;
        }
        if c == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
            // skip $( opener
            i += 2;
            continue;
        }
        if c == ')' {
            // a closing paren that was part of $() — drop it. A bare ')' in
            // the command (not from $()) is harmless to drop for matching.
            i += 1;
            continue;
        }
        s.push(c);
        i += 1;
    }

    // 2. Strip single/double quotes around the leading binary token, so
    //    'ironlint' trust and "ironlint" trust collapse to ironlint trust.
    //    Only the quote chars are removed; quoted strings otherwise survive.
    s = s.chars().filter(|&c| c != '\'' && c != '"').collect();

    // 3. Collapse runs of whitespace (spaces + tabs) into a single space, so
    //    `ironlint   trust` and `ironlint\ttrust` match the same pattern.
    let mut collapsed = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' || c == '\t' {
            if !prev_space {
                collapsed.push(' ');
            }
            prev_space = true;
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// True if the normalized command is `ironlint trust` (any trailing args).
/// Matches `ironlint trust`, `ironlint trust --config x`, `ironlint trust .`.
/// Does NOT match `ironlint check`, `ironlint doctor`, etc.
fn is_ironlint_trust(normalized: &str) -> bool {
    // `ironlint trust` as a command, possibly with trailing args. Require a
    // word boundary after `trust` so `ironlint trustworthy` does not match.
    // The `echo "ironlint trust"` false-positive guard is handled by the
    // caller checking the command starts with `ironlint trust`, not just
    // contains it — `echo` starts with `echo`, not `ironlint`.
    if normalized == "ironlint trust" || normalized.starts_with("ironlint trust ") {
        return true;
    }
    // `cd .ironlint && trust` and `cd .ironlint/gates && trust`: a bare
    // `trust` after a `cd` into the policy dir (`.ironlint` itself or
    // anything under `.ironlint/`). Match the chained form explicitly — a
    // bare `trust` elsewhere is not a trust invocation. The `cd` target must
    // start the `.ironlint` path component (not `cd .ironlintfoo`), so check
    // for `cd .ironlint` followed by a path separator or whitespace.
    if let Some(rest) = normalized.strip_prefix("cd .ironlint") {
        let boundary = rest
            .chars()
            .next()
            .is_none_or(|c| c == '/' || c == ' ' || c == '\t');
        if boundary && normalized.contains(" trust") {
            return true;
        }
    }
    false
}

/// True if the command writes to the policy surface (`.ironlint.yml` or
/// anything under `.ironlint/gates/`). Detected via redirect operators,
/// `tee`, in-place editors, and `cp`/`mv` with a policy path as destination.
/// Implemented in Task 3; returns false here so Task 2's tests focus on trust.
fn is_policy_write(_normalized: &str) -> bool {
    false
}

/// Decide whether `command` may run.
///
/// Pure: no I/O, no state. Returns `Block(reason)` for `ironlint trust` (any
/// args) and Bash writes to the policy surface; `Allow` otherwise, including
/// the documented indirection gap (which is *intentionally* allowed).
pub fn decide(command: &str) -> Decision {
    let n = normalize(command);

    if is_ironlint_trust(&n) {
        return Decision::Block(TRUST_REASON.to_string());
    }
    if is_policy_write(&n) {
        // reason set in Task 3
        return Decision::Block(
            "ironlint policy files must be edited through the Write/Edit tool (which is gated), not via Bash"
                .to_string(),
        );
    }
    Decision::Allow
}

#[cfg(test)]
mod tests {
    use super::{Decision, decide};

    /// Assert `decide(cmd)` blocks; the reason is checked loosely (caller
    /// cares that it blocked, not the exact wording).
    fn assert_blocks(cmd: &str) {
        match decide(cmd) {
            Decision::Block(_) => {}
            Decision::Allow => panic!("expected Block for {cmd:?}, got Allow"),
        }
    }

    /// Assert `decide(cmd)` allows.
    fn assert_allows(cmd: &str) {
        match decide(cmd) {
            Decision::Allow => {}
            Decision::Block(r) => panic!("expected Allow for {cmd:?}, got Block({r:?})"),
        }
    }

    // --- (1) ironlint trust, any args ---
    #[test]
    fn blocks_bare_ironlint_trust() {
        assert_blocks("ironlint trust");
    }

    #[test]
    fn blocks_ironlint_trust_with_config_flag() {
        assert_blocks("ironlint trust --config shared/base.yml");
    }

    #[test]
    fn blocks_ironlint_trust_with_dot_arg() {
        assert_blocks("ironlint trust .");
    }

    // --- light de-obfuscation: backtick / $() around the binary name ---
    #[test]
    fn blocks_backtick_ironlint_trust() {
        assert_blocks("`ironlint` trust");
    }

    #[test]
    fn blocks_dollar_paren_ironlint_trust() {
        assert_blocks("$(ironlint) trust");
    }

    // --- light de-obfuscation: quoted binary name ---
    #[test]
    fn blocks_single_quoted_ironlint_trust() {
        assert_blocks("'ironlint' trust");
    }

    #[test]
    fn blocks_double_quoted_ironlint_trust() {
        assert_blocks("\"ironlint\" trust");
    }

    // --- light de-obfuscation: whitespace around the binary token ---
    #[test]
    fn blocks_ironlint_trust_extra_spaces() {
        assert_blocks("ironlint   trust");
    }

    #[test]
    fn blocks_ironlint_trust_tab_separated() {
        assert_blocks("ironlint\ttrust");
    }

    // --- cd .ironlint && trust (bare trust after cd into policy dir) ---
    #[test]
    fn blocks_cd_ironlint_then_trust() {
        assert_blocks("cd .ironlint && trust");
    }

    #[test]
    fn blocks_cd_ironlint_gates_then_trust() {
        assert_blocks("cd .ironlint/gates && trust");
    }

    // --- false-positive guard: read-only ironlint subcommands MUST allow ---
    #[test]
    fn allows_ironlint_check() {
        assert_allows("ironlint check");
    }

    #[test]
    fn allows_ironlint_doctor() {
        assert_allows("ironlint doctor");
    }

    #[test]
    fn allows_ironlint_validate() {
        assert_allows("ironlint validate");
    }

    #[test]
    fn allows_ironlint_explain() {
        assert_allows("ironlint explain");
    }

    #[test]
    fn allows_ironlint_show_resolved_config() {
        assert_allows("ironlint show-resolved-config");
    }

    #[test]
    fn allows_ironlint_init() {
        assert_allows("ironlint init");
    }

    // --- false-positive guard: 'ironlint trust' as a STRING, not a command ---
    #[test]
    fn allows_echo_quoting_ironlint_trust() {
        assert_allows("echo \"run ironlint trust to bless\"");
    }
}
