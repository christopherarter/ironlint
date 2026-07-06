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
    //
    //    Iterator-driven (no manual index arithmetic) so there is no `+=` to
    //    mutate into a hang; the `$(` opener consumes its `(` via peek+next.
    let mut s = String::with_capacity(command.len());
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // backtick delimiter — drop (both opening and closing are skipped).
            '`' => continue,
            // `$(` opener — drop the `$` here and the `(` via next(). A `$`
            // NOT followed by `(` is a var sigil (`$FOO`) and is preserved by
            // falling through to the catch-all. The fall-through (not a guard)
            // keeps a `$` with no following `(` observable: `$ironlint trust`
            // must NOT collapse to `ironlint trust` (it's a var ref, not the
            // binary) — though as a var ref with no value it's a no-op for our
            // purposes; the important behavior is that only a true `$(` opener
            // strips the `(`.
            '$' => {
                if chars.peek() == Some(&'(') {
                    let _ = chars.next();
                    continue;
                }
                s.push('$');
            }
            // closing paren — drop (a $() closer, or a bare ')' which is
            // harmless to drop for matching).
            ')' => continue,
            other => s.push(other),
        }
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

/// True if a path token refers to the policy surface: the literal
/// `.ironlint.yml` (at any depth — bare or path-prefixed) or anything under
/// `.ironlint/gates/`. Matched on the path string, not the filesystem.
fn is_policy_path(token: &str) -> bool {
    // `.ironlint.yml` as a SUFFIX (covers the bare token and any path-prefixed
    // form like `./.ironlint.yml` or `sub/.ironlint.yml`), OR `.ironlint.yml`
    // appearing with a leading slash mid-token (covered by suffix already, so
    // this arm is only reached when suffix is false — kept distinct so a
    // mutation flipping one operator is observable), OR a gates dir prefix.
    //
    // The arms are intentionally non-redundant for the test corpus: the bare
    // `.ironlint.yml` exercises only the suffix arm; `x/.ironlint.yml.bak`
    // (suffix `.bak`, contains `/.ironlint.yml`) exercises only the contains
    // arm; `path/.ironlint.yml` exercises suffix (and would also hit contains,
    // but suffix short-circuits). Don't fold them.
    token.ends_with(".ironlint.yml")
        || token.contains("/.ironlint.yml")
        || token.contains(".ironlint/gates/")
}

/// True if the normalized command writes to the policy surface. Detected via:
///   - redirect operators targeting a policy path: >, >>, >|, &>, &>>
///   - `tee` writing a policy path
///   - in-place editors: `sed -i`, `ed`, `perl -i`
///   - `cp`/`mv` with a policy path as the DESTINATION (last arg)
///
/// `cp .ironlint.yml /tmp/backup` (policy path as source) is a read and MUST
/// allow — only the destination is checked for cp/mv.
fn is_policy_write(normalized: &str) -> bool {
    // Split into whitespace-separated tokens. This is deliberately crude —
    // the matcher's job is the direct form, not surviving quoting games.
    let tokens: Vec<&str> = normalized.split_whitespace().collect();

    redirect_targets_policy(&tokens)
        || tee_targets_policy(&tokens)
        || inplace_editor_targets_policy(&tokens)
        || cp_mv_destination_is_policy(&tokens)
}

/// Redirect operators: the token AFTER `>`, `>>`, `>|`, `&>`, `&>>` is the
/// target. Also catch a redirect glued to a path (`>.ironlint.yml`).
fn redirect_targets_policy(tokens: &[&str]) -> bool {
    for (i, t) in tokens.iter().enumerate() {
        // Glued form: `>.ironlint.yml`, `>>.ironlint.yml`
        let stripped = t
            .strip_prefix(">>")
            .or_else(|| t.strip_prefix(">"))
            .or_else(|| t.strip_prefix("&>>"))
            .or_else(|| t.strip_prefix("&>"));
        if let Some(rest) = stripped {
            if is_policy_path(rest) {
                return true;
            }
        }
        // Bare operator form: `> .ironlint.yml` (next token is the target)
        if matches!(*t, ">" | ">>" | ">|" | "&>" | "&>>") {
            if let Some(next) = tokens.get(i + 1) {
                if is_policy_path(next) {
                    return true;
                }
            }
        }
    }
    false
}

/// `tee` / `tee -a`: a later argument is the destination file. `tee` may
/// appear after a pipe (`echo x | tee .ironlint.yml`), so scan for it as any
/// token, then check the non-flag arguments that follow it.
fn tee_targets_policy(tokens: &[&str]) -> bool {
    let mut seen_tee = false;
    for t in tokens {
        if seen_tee && !t.starts_with('-') && is_policy_path(t) {
            return true;
        }
        if *t == "tee" {
            seen_tee = true;
        }
    }
    false
}

/// In-place editors: `sed -i ... <file>`, `ed -s <file>`, `perl -i ... <file>`.
/// The last non-flag argument is the target file. Only block if `-i` is
/// present (sed/perl) or it's `ed` (which edits in place by nature).
fn inplace_editor_targets_policy(tokens: &[&str]) -> bool {
    let Some(&cmd) = tokens.first() else {
        return false;
    };
    let inplace = match cmd {
        "ed" => true,
        "sed" | "perl" => {
            // `-i` exactly (bare) OR an `-i`-prefixed flag (`-i.bak`). Both
            // forms denote in-place editing; either is sufficient. The two
            // checks are intentionally separate so a mutation to one alone
            // is caught by the form that exercises only the other.
            has_bare_dash_i(tokens) || has_dash_i_prefixed_flag(tokens)
        }
        _ => false,
    };
    inplace && tokens.last().is_some_and(|last| is_policy_path(last))
}

/// True if any token is exactly `-i` (the bare in-place flag).
fn has_bare_dash_i(tokens: &[&str]) -> bool {
    tokens.contains(&"-i")
}

/// True if any token starts with `-i` but is longer (e.g. `-i.bak`).
fn has_dash_i_prefixed_flag(tokens: &[&str]) -> bool {
    tokens.iter().any(|t| t.starts_with("-i") && *t != "-i")
}

/// `cp`/`mv` with a policy path as the DESTINATION. The destination is the
/// last argument (for both two-arg and multi-source forms). A policy path as
/// a SOURCE (e.g. `cp .ironlint.yml /tmp/backup`) MUST allow — that's why
/// only the last token is checked.
fn cp_mv_destination_is_policy(tokens: &[&str]) -> bool {
    let Some(&cmd) = tokens.first() else {
        return false;
    };
    matches!(cmd, "cp" | "mv")
        && tokens.len() >= 3
        && tokens.last().is_some_and(|dest| is_policy_path(dest))
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
    use super::{decide, Decision};

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

    // --- false-positive guard: 'cd .ironlintfoo' is NOT the policy dir ---
    // The boundary check after `cd .ironlint` exists to reject lookalike
    // dirs (`.ironlintfoo`, `.ironlint-backup`). Without a test pinning the
    // rejection, a mutation flipping the boundary predicate survives.
    #[test]
    fn allows_cd_ironlintfoo_lookalike() {
        assert_allows("cd .ironlintfoo && trust");
    }

    #[test]
    fn allows_cd_ironlint_backup_lookalike() {
        assert_allows("cd .ironlint-backup && trust");
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

    // --- (2) Bash writes to the policy surface: redirects ---
    #[test]
    fn blocks_redirect_to_ironlint_yml() {
        assert_blocks("echo x > .ironlint.yml");
    }

    #[test]
    fn blocks_append_to_ironlint_yml() {
        assert_blocks("echo x >> .ironlint.yml");
    }

    #[test]
    fn blocks_clobber_redirect_to_ironlint_yml() {
        assert_blocks("echo x >| .ironlint.yml");
    }

    #[test]
    fn blocks_amp_redirect_to_ironlint_yml() {
        assert_blocks("echo x &> .ironlint.yml");
    }

    #[test]
    fn blocks_amp_append_to_ironlint_yml() {
        assert_blocks("echo x &>> .ironlint.yml");
    }

    #[test]
    fn blocks_cat_redirect_to_ironlint_yml() {
        assert_blocks("cat > .ironlint.yml");
    }

    // --- tee ---
    #[test]
    fn blocks_tee_ironlint_yml() {
        assert_blocks("echo x | tee .ironlint.yml");
    }

    #[test]
    fn blocks_tee_append_ironlint_yml() {
        assert_blocks("echo x | tee -a .ironlint.yml");
    }

    // `tee` as the FIRST token (no pipe) — still a write to the policy path.
    // Pins the `idx + 1` slice boundary: a `tee` at index 0 must scan its own
    // following args, not the token before it.
    #[test]
    fn blocks_tee_as_first_token_ironlint_yml() {
        assert_blocks("tee .ironlint.yml");
    }

    // --- in-place editors ---
    #[test]
    fn blocks_sed_inplace_ironlint_yml() {
        assert_blocks("sed -i 's/x/y/' .ironlint.yml");
    }

    // `sed -iEXT` (in-place with a backup extension) is the same write as
    // `sed -i`. Pinning it closes a mutation gap: the `-i` detector must
    // match the `-i.bak` form, not just the bare `-i` token.
    #[test]
    fn blocks_sed_inplace_with_backup_ext_ironlint_yml() {
        assert_blocks("sed -i.bak 's/x/y/' .ironlint.yml");
    }

    #[test]
    fn blocks_perl_inplace_ironlint_yml() {
        assert_blocks("perl -i -pe 's/x/y/' .ironlint.yml");
    }

    // `perl -iEXT` (in-place with backup extension) — same pin as sed.
    #[test]
    fn blocks_perl_inplace_with_backup_ext_ironlint_yml() {
        assert_blocks("perl -i.bak -pe 's/x/y/' .ironlint.yml");
    }

    #[test]
    fn blocks_ed_ironlint_yml() {
        assert_blocks("ed -s .ironlint.yml");
    }

    // `sed` (or `perl`) editing a policy file WITHOUT any `-i` flag is a READ
    // (sed streams to stdout), so it MUST allow. This single test pins three
    // mutants at once: if `has_bare_dash_i` or `has_dash_i_prefixed_flag`
    // mutates to always-true, or the `&&` in the prefixed check flips to `||`,
    // this command flips from Allow to Block.
    #[test]
    fn allows_sed_without_inplace_flag_on_policy_file() {
        assert_allows("sed 's/x/y/' .ironlint.yml");
    }

    #[test]
    fn allows_perl_without_inplace_flag_on_policy_file() {
        assert_allows("perl -pe 's/x/y/' .ironlint.yml");
    }

    // --- same detectors against .ironlint/gates/ ---
    #[test]
    fn blocks_redirect_to_gate_script() {
        assert_blocks("echo x > .ironlint/gates/lint.sh");
    }

    #[test]
    fn blocks_sed_inplace_gate_script() {
        assert_blocks("sed -i 's/x/y/' .ironlint/gates/lint.sh");
    }

    // `.ironlint.yml` appearing mid-token but NOT as a suffix (the file is
    // something else with `.ironlint.yml` embedded behind a slash, e.g. a
    // backup `sub/.ironlint.yml.bak`). Pins `is_policy_path`'s
    // `contains("/.ironlint.yml")` arm independently of the suffix arm — a
    // mutation flipping that operator to `&&` lets this through.
    #[test]
    fn blocks_redirect_to_embedded_ironlint_yml_non_suffix() {
        assert_blocks("echo x > sub/.ironlint.yml.bak");
    }

    // --- cp / mv ONTO a policy path (destination) ---
    #[test]
    fn blocks_cp_onto_gate_script() {
        assert_blocks("cp malicious.sh .ironlint/gates/lint.sh");
    }

    #[test]
    fn blocks_mv_onto_ironlint_yml() {
        assert_blocks("mv bad.yml .ironlint.yml");
    }

    // --- false-positive guard: policy path as SOURCE (a read), not destination ---
    #[test]
    fn allows_cp_from_ironlint_yml_as_source() {
        assert_allows("cp .ironlint.yml /tmp/backup");
    }

    #[test]
    fn allows_cat_ironlint_yml_piped_to_grep() {
        assert_allows("cat .ironlint.yml | grep checks");
    }

    #[test]
    fn allows_grep_recursive_ironlint() {
        assert_allows("grep -r ironlint docs/");
    }

    #[test]
    fn allows_ls_ironlint_gates() {
        assert_allows("ls .ironlint/gates/");
    }

    #[test]
    fn allows_cat_gate_script() {
        assert_allows("cat .ironlint/gates/lint.sh");
    }

    // `tee` to a NON-policy file must allow — pins the `&&` (not `||`) in
    // tee_targets_policy's non-flag check, so a benign `tee /tmp/log` is
    // never false-blocked.
    #[test]
    fn allows_tee_to_non_policy_file() {
        assert_allows("echo x | tee /tmp/log");
    }

    // --- ordinary commands never mention ironlint, so the pre-filter skips
    //     them entirely; pin that decide() also allows them if reached ---
    #[test]
    fn allows_cargo_test() {
        assert_allows("cargo test");
    }

    #[test]
    fn allows_git_status() {
        assert_allows("git status");
    }

    // --- documented known gap: variable-substitution indirection MUST allow ---
    #[test]
    fn allows_iron_echo_lint_indirection() {
        assert_allows("iron$(echo lint) trust");
    }

    #[test]
    fn allows_ironvar_trust_indirection() {
        assert_allows("IRON=ironlint; $IRON trust");
    }

    #[test]
    fn allows_base64_eval_indirection() {
        assert_allows("base64 -d <<< 'aXJvbmxpbnQgdHJ1c3Q=' | sh");
    }

    #[test]
    fn allows_bash_script_indirection() {
        assert_allows("bash scripts/x.sh");
    }

    // --- normalize edge: a lone '$' not followed by '(' must not be treated
    //     as a $() opener (which would skip 2 chars and could mask a token).
    //     `echo $` is a benign command that must allow; pinning it closes a
    //     mutation gap on the `c == '$' && i+1 < len && chars[i+1] == '('`
    //     guard in normalize(). ---
    #[test]
    fn allows_echo_trailing_dollar() {
        assert_allows("echo $");
    }
}
