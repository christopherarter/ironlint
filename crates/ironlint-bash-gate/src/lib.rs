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
            // Shell grouping chars — `(`, `)`, `{`, `}`. Dropping them turns
            // `(ironlint trust)` and `{ ironlint trust; }` into `ironlint trust`
            // (the `;` is a separator handled by the caller's segment split).
            // These denote grouping, not tokens; dropping them is safe for
            // matching and does not collapse indirection (a `$(` is already
            // consumed above before its `(` could reach here).
            '(' | ')' | '{' | '}' => continue,
            other => s.push(other),
        }
    }

    // 2. Strip single/double quotes around the leading binary token, so
    //    'ironlint' trust and "ironlint" trust collapse to ironlint trust.
    //    Only the quote chars are removed; quoted strings otherwise survive.
    s = s.chars().filter(|&c| c != '\'' && c != '"').collect();

    // 3. Trim leading/trailing whitespace. (Runs of internal whitespace are NOT
    //    collapsed here — every consumer re-splits on `split_whitespace()`,
    //    which collapses internally, so a collapse step would be dead code that
    //    only generates un-killable mutation survivors. `strip_wrappers` and
    //    `is_policy_write` both `split_whitespace`, so `ironlint   trust` and
    //    `ironlint\ttrust` normalize to the same token list regardless.)
    s.trim().to_string()
}

/// Split a normalized command into independently-checkable segments at the
/// shell command separators `&&`, `||`, `;`, and `|`. A `trust` (or a policy
/// write) in ANY segment blocks — `ironlint check || ironlint trust` is the
/// textbook lazy escape. String surgery on the separators, not shell
/// evaluation; a pipe inside a quoted string would be mis-split, but quotes
/// are stripped in `normalize` and the threat tier is lazy models, not
/// adversarial quoting.
fn segments(normalized: &str) -> Vec<String> {
    // `>|` (clobber redirect) is the only redirect operator containing `|`.
    // Protect it with a NUL sentinel before splitting on `|` (pipe), then
    // restore it in each segment. The two-char separators `&&`/`||` are
    // collapsed to a DISTINCT sentinel (`;`-equivalent) so a lone `|` of a
    // split `||` isn't double-counted. Both sentinels are NUL-free bytes a
    // Bash command can't contain.
    const CLOBBER: &str = "\u{0}";
    const SEP: &str = "\u{1}";
    let protected = normalized.replace(">|", CLOBBER);
    let with_seps = protected.replace("&&", SEP).replace("||", SEP);
    with_seps
        .split([';', '|', '\u{1}'])
        .map(str::trim)
        .filter(|seg| !seg.is_empty())
        .map(|seg| seg.replace(CLOBBER, ">|").to_string())
        .collect()
}

/// The command prefixes that wrap an ironlint invocation without changing
/// its meaning: `nohup`, `env [VAR=val]...`, `exec`, `eval`, `timeout <N>`.
/// A lazy model prepends these to "make sure it runs"; stripping them recovers
/// the direct form. Bounded explicit list — not shell evaluation.
fn strip_wrappers(segment: &str) -> String {
    // Drop leading wrapper prefixes (nohup/env/exec/eval/timeout) one at a
    // time. Uses `split_first` over a slice cursor — no `+=`/`+` index
    // arithmetic to mutate-hang or mutate-survive.
    let all: Vec<&str> = segment.split_whitespace().collect();
    let mut rest = all.as_slice();
    while let Some((first, tail)) = rest.split_first() {
        match *first {
            "nohup" | "exec" | "eval" => rest = tail,
            // env itself + any leading VAR=val assignments it carries.
            "env" => rest = skip_assignments(tail),
            // timeout + its single duration arg.
            "timeout" => rest = tail.split_first().map(|(_, t)| t).unwrap_or(&[]),
            _ => break,
        }
    }
    rest.join(" ")
}

/// Given the tokens AFTER `env`, drop `env`'s leading `VAR=val` assignments
/// and return the slice that follows them (the actual command). Iterator-
/// driven so a mutation to the skip logic can't hang.
fn skip_assignments<'a>(tail: &'a [&'a str]) -> &'a [&'a str] {
    let end = tail.iter().take_while(|t| t.contains('=')).count();
    &tail[end..]
}

/// True if a token is the ironlint binary: the literal name, or a path ending
/// in `/ironlint` (absolute) or `./ironlint` (relative). Indirection
/// (`$IRON`) is NOT matched — that's the documented known gap.
fn is_ironlint_binary(token: &str) -> bool {
    token == "ironlint" || token.ends_with("/ironlint") || token == "./ironlint"
}

/// Given a tokenized segment with the ironlint binary at `bin_idx`, decide
/// whether `trust` follows as the subcommand. Handles global flags before the
/// subcommand: `ironlint --config x.yml trust`, `ironlint -v trust`. The rule
/// is permissive in the flag tokens but strict on the subcommand: if ANY
/// non-flag token after the binary is a read-only subcommand (`check`,
/// `doctor`), it's not a trust invocation. Otherwise, if `trust` appears as a
/// token after the binary, block.
///
/// `ironlint check --config x` (read-only subcommand THEN a flag) must allow —
/// the read-only subcommand short-circuits. `ironlint --config x.yml trust`
/// blocks (flag, then trust). `ironlint --config x.yml check` allows (flag,
/// then check). `ironlint trust --config x` blocks (trust first).
/// True if `trust` follows the ironlint binary at `tokens[bin_idx]`, after
/// skipping any global flags. The first non-flag token decides: `trust` blocks;
/// a read-only subcommand allows. Iterator-driven (no index arithmetic to
/// mutate-hang or mutate-survive).
fn trust_after_binary(tokens: &[&str], bin_idx: usize) -> bool {
    for t in &tokens[bin_idx + 1..] {
        if *t == "trust" {
            return true;
        }
        if !is_flag_token(t) {
            // A read-only subcommand (or any non-flag non-trust token) — allow.
            return false;
        }
        // A flag — skip and keep scanning.
    }
    false
}

/// True if `token` looks like a flag: a `--long[=val]` or a `-x` short flag.
/// Used to skip global flags between the ironlint binary and its subcommand.
fn is_flag_token(token: &str) -> bool {
    token.starts_with("--") || (token.starts_with('-') && token.len() == 2)
}

/// True if the segment invokes `ironlint trust` — the direct form plus the
/// light de-obfuscation a lazy model reaches for: path-prefixed binary names
/// (`/usr/local/bin/ironlint`, `./ironlint`), wrapper prefixes (`nohup`,
/// `env`, `exec`, `eval`, `timeout`), and global flags before the subcommand
/// (`ironlint --config x.yml trust`). Does NOT match read-only subcommands
/// (`check`, `doctor`, etc.).
///
/// Checks EVERY ironlint binary occurrence in the segment, not just the first
/// token — `ironlint check or ironlint trust` has a second binary (`or` is not
/// a shell operator, so segments() leaves it as one segment) whose `trust`
/// subcommand the first-binary scan misses (it short-circuits on `check`).
/// The first-token-is-binary guard still holds, so `echo ... ironlint trust`
/// (a string argument to echo) stays a non-match.
fn is_ironlint_trust(segment: &str) -> bool {
    let stripped = strip_wrappers(segment);
    let tokens: Vec<&str> = stripped.split_whitespace().collect();
    // The ironlint binary must be the FIRST token of the (wrapper-stripped)
    // segment — `echo ... ironlint trust` is a string argument to echo, not a
    // trust invocation. This is the false-positive guard: `echo "run
    // ironlint trust to bless"` starts with `echo`, not `ironlint`.
    let Some(&first) = tokens.first() else {
        return false;
    };
    if !is_ironlint_binary(first) {
        return false;
    }
    // A second ironlint binary buried in the segment (e.g. after a bare `or`)
    // is a separate invocation whose `trust` subcommand the first-binary scan
    // misses — check each occurrence. `trust_after_binary` itself stays strict:
    // `ironlint check trust` (one binary, `trust` as a stray positional to
    // `check`) still allows, because clap rejects the positional and nothing
    // fires; only a real second `ironlint trust` invocation blocks.
    tokens
        .iter()
        .enumerate()
        .any(|(idx, t)| is_ironlint_binary(t) && trust_after_binary(&tokens, idx))
}

/// `cd .ironlint && trust` and `cd .ironlint/gates && trust`: a bare `trust`
/// in a later segment after a `cd` into the policy dir (`.ironlint` itself
/// or anything under `.ironlint/`). A bare `trust` elsewhere is not a trust
/// invocation. The `cd` target must start the `.ironlint` path component (not
/// `cd .ironlintfoo`). Returns true if some segment cds into the policy dir
/// and a later segment is exactly `trust` (with optional trailing args).
fn cd_into_policy_then_trust(segs: &[String]) -> bool {
    // Find the FIRST segment that cds into the policy dir, then check whether
    // a LATER segment runs `trust`. Scoping the trust check to segments after
    // the cd (not `any` over all segments) makes the bare-`trust` equality
    // observable: under a `== -> !=` mutation, the cd segment itself would
    // satisfy `!= "trust"` and false-block.
    //
    // The `cd_idx + 1` is intrinsic to mutate-kill: the cd segment (`cd
    // .ironlint`) can never itself be `trust`, so including it (`+ -> *` =
    // `cd_idx * 1`) yields the same verdict. Decomposing to avoid the `+ 1`
    // would reintroduce the `==` survivor this structure was built to kill.
    let cd_idx = segs.iter().position(|s| {
        s.strip_prefix("cd .ironlint")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    let Some(cd_idx) = cd_idx else { return false };
    segs[cd_idx + 1..]
        .iter()
        .any(|s| s == "trust" || s.starts_with("trust "))
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

/// True if the normalized segment writes to the policy surface. Detected via:
///   - redirect operators targeting a policy path: >, >>, >|, &>, &>>
///     (bare, start-glued, or end-glued to the preceding arg)
///   - `tee` writing a policy path
///   - in-place editors: `sed -i`, `ed`, `perl -i`
///   - `cp`/`mv`/`install`/`rsync` with a policy path as the DESTINATION
///   - `dd of=<policy path>` / `sponge <policy path>`
///
/// A policy path as a SOURCE (e.g. `cp .ironlint.yml /tmp/backup`,
/// `dd if=.ironlint.yml of=/tmp/backup`) is a read and MUST allow — only the
/// destination is checked.
fn is_policy_write(segment: &str) -> bool {
    let tokens: Vec<&str> = segment.split_whitespace().collect();

    redirect_targets_policy(&tokens)
        || tee_targets_policy(&tokens)
        || inplace_editor_targets_policy(&tokens)
        || cp_mv_destination_is_policy(&tokens)
        || dd_targets_policy(&tokens)
        || sponge_targets_policy(&tokens)
}

/// The redirect operators we gate, longest-first so `>>` is tried before `>`.
const REDIRECT_OPS: &[&str] = &["&>>", ">>", "&>", ">|", ">"];

/// Redirect operators targeting a policy path. Three forms:
///   - bare operator + next token: `> .ironlint.yml`
///   - operator glued to the START of a token: `>.ironlint.yml`
///   - operator glued to the END of a preceding arg: `echo x>.ironlint.yml`
///     (the most common form a model emits — no space before the `>`).
fn redirect_targets_policy(tokens: &[&str]) -> bool {
    for (i, t) in tokens.iter().enumerate() {
        // Bare operator: `> .ironlint.yml` — the NEXT token is the target.
        if REDIRECT_OPS.contains(t) {
            if let Some(next) = tokens.get(i + 1) {
                if is_policy_path(next) {
                    return true;
                }
            }
        }
        // Glued (start OR end): a token that contains a redirect op AND ends
        // with a policy path. `>.ironlint.yml` and `x>.ironlint.yml` both
        // satisfy `ends_with(".ironlint.yml")` and contain a redirect op, so one
        // check covers both — no need to split the token. Skip the bare-op
        // tokens (handled above) to avoid a double-count false signal.
        if !REDIRECT_OPS.contains(t) && contains_redirect_op(t) && is_policy_path(t) {
            return true;
        }
    }
    false
}

/// True if `token` contains any redirect operator (`>`, `>>`, `>|`, `&>`,
/// `&>>`) anywhere in it. Used to distinguish a glued redirect
/// (`x>.ironlint.yml`) from a plain path token (`.ironlint.yml`).
fn contains_redirect_op(token: &str) -> bool {
    REDIRECT_OPS.iter().any(|op| token.contains(op))
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

/// `cp`/`mv`/`install`/`rsync` with a policy path as the DESTINATION. The
/// destination is the last argument (for both two-arg and multi-source forms).
/// A policy path as a SOURCE (e.g. `cp .ironlint.yml /tmp/backup`) MUST
/// allow — that's why only the last token is checked. `install` and `rsync`
/// share the cp/mv destination semantics for our purposes.
fn cp_mv_destination_is_policy(tokens: &[&str]) -> bool {
    let Some(&cmd) = tokens.first() else {
        return false;
    };
    matches!(cmd, "cp" | "mv" | "install" | "rsync")
        && tokens.len() >= 3
        && tokens.last().is_some_and(|dest| is_policy_path(dest))
}

/// `dd of=<policy path>`: dd writes via its `of=` operand, not a positional
/// arg. Block if any token is `of=<policy path>` OR `of` followed by a policy
/// path token. `dd if=.ironlint.yml of=/tmp/backup` (policy as INPUT) MUST
/// allow — only the `of=` destination is checked.
fn dd_targets_policy(tokens: &[&str]) -> bool {
    let is_dd = tokens.first().is_some_and(|c| *c == "dd");
    if !is_dd {
        return false;
    }
    for (i, t) in tokens.iter().enumerate() {
        // Glued: `of=.ironlint.yml`.
        if let Some(rest) = t.strip_prefix("of=") {
            if is_policy_path(rest) {
                return true;
            }
        }
        // Separated: `of .ironlint.yml`.
        if *t == "of" {
            if let Some(next) = tokens.get(i + 1) {
                if is_policy_path(next) {
                    return true;
                }
            }
        }
    }
    false
}

/// `sponge <file>` (from moreutils): writes its stdin to a file. The file is
/// the LAST argument. `echo x | sponge .ironlint.yml` is a policy write.
fn sponge_targets_policy(tokens: &[&str]) -> bool {
    let is_sponge = tokens.first().is_some_and(|c| *c == "sponge");
    is_sponge && tokens.last().is_some_and(|last| is_policy_path(last))
}

/// Decide whether `command` may run.
///
/// Pure: no I/O, no state. Returns `Block(reason)` for `ironlint trust` (any
/// args, in any command segment) and Bash writes to the policy surface;
/// `Allow` otherwise, including the documented indirection gap (which is
/// *intentionally* allowed). A `trust` or policy write in ANY segment of a
/// chained command (`a && ironlint trust`, `check || trust`) blocks — the
/// whole command is denied.
pub fn decide(command: &str) -> Decision {
    let n = normalize(command);
    let segs = segments(&n);

    // `cd .ironlint && trust`: a bare `trust` after a `cd` into the policy
    // dir. The `&&` splits this into two segments, so check across the
    // segment list — does any segment cd into the policy dir, and does a
    // later segment run a bare `trust`?
    if cd_into_policy_then_trust(&segs) {
        return Decision::Block(TRUST_REASON.to_string());
    }
    for seg in &segs {
        if is_ironlint_trust(seg) {
            return Decision::Block(TRUST_REASON.to_string());
        }
        if is_policy_write(seg) {
            return Decision::Block(
                "ironlint policy files must be edited through the Write/Edit tool (which is gated), not via Bash"
                    .to_string(),
            );
        }
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

    // =====================================================================
    // Code-review hardening (2026-07-06): direct-form bypasses a lazy
    // non-reasoning model plausibly emits. These are NOT indirection — the
    // binary is invoked by literal name / the write targets a literal path.
    // =====================================================================

    // --- F1: end-glued redirect (op glued to the PRECEDING arg's tail) ---
    // `echo x>.ironlint.yml` — the MOST common form a model emits (no space
    // before the `>`). All redirect variants must block here too.
    #[test]
    fn blocks_end_glued_redirect_to_ironlint_yml() {
        assert_blocks("echo x>.ironlint.yml");
    }

    #[test]
    fn blocks_end_glued_append_to_ironlint_yml() {
        assert_blocks("echo x>>.ironlint.yml");
    }

    #[test]
    fn blocks_end_glued_clobber_to_ironlint_yml() {
        assert_blocks("echo x>|.ironlint.yml");
    }

    #[test]
    fn blocks_end_glued_amp_redirect_to_ironlint_yml() {
        assert_blocks("echo x&>.ironlint.yml");
    }

    #[test]
    fn blocks_end_glued_amp_append_to_ironlint_yml() {
        assert_blocks("echo x&>>.ironlint.yml");
    }

    // --- F2: chained commands — `ironlint trust` after a separator ---
    // `ironlint check || ironlint trust` is the textbook lazy escape: "the
    // check failed, so just trust it." Each separator form must block.
    #[test]
    fn blocks_chained_ampamp_ironlint_trust() {
        assert_blocks("echo done && ironlint trust");
    }

    #[test]
    fn blocks_chained_oror_ironlint_trust() {
        assert_blocks("ironlint check || ironlint trust");
    }

    #[test]
    fn blocks_chained_semicolon_ironlint_trust() {
        assert_blocks("echo hi; ironlint trust");
    }

    #[test]
    fn blocks_chained_pipe_ironlint_trust() {
        assert_blocks("echo hi | ironlint trust");
    }

    // A chained trust with args (defense against `... && ironlint trust -c x`).
    #[test]
    fn blocks_chained_ironlint_trust_with_args() {
        assert_blocks("true && ironlint trust --config x.yml");
    }

    // `or` is NOT a shell operator (that's `||`), so a lazy model confusing
    // the two writes `ironlint check or ironlint trust`. sh runs `ironlint
    // check`, then `or` (command not found), then `ironlint trust` — trust
    // fires. segments() doesn't split on bare `or` (correctly — it's not a
    // separator), so the whole string is one segment; the fix is to catch
    // `trust` as ANY token after the ironlint binary in a segment, not just
    // the first non-flag one. Safe because no subcommand legitimately takes
    // `trust` as an argument (`explain` takes a file path).
    #[test]
    fn blocks_ironlint_check_or_ironlint_trust() {
        assert_blocks("ironlint check or ironlint trust");
    }

    // --- F3: prefix wrappers (`nohup`, `env`, `exec`, `eval`, `timeout`) ---
    #[test]
    fn blocks_nohup_ironlint_trust() {
        assert_blocks("nohup ironlint trust");
    }

    #[test]
    fn blocks_env_ironlint_trust() {
        assert_blocks("env ironlint trust");
    }

    #[test]
    fn blocks_exec_ironlint_trust() {
        assert_blocks("exec ironlint trust");
    }

    #[test]
    fn blocks_eval_ironlint_trust() {
        assert_blocks("eval ironlint trust");
    }

    #[test]
    fn blocks_timeout_ironlint_trust() {
        assert_blocks("timeout 5 ironlint trust");
    }

    // `env VAR=val ironlint trust` (env with an assignment) — a wrapper with
    // a leading var assignment still wraps a direct invocation.
    #[test]
    fn blocks_env_with_assignment_ironlint_trust() {
        assert_blocks("env IRONLINT_TIMEOUT=10 ironlint trust");
    }

    // --- F4: full-path / relative-path invocation ---
    #[test]
    fn blocks_absolute_path_ironlint_trust() {
        assert_blocks("/usr/local/bin/ironlint trust");
    }

    #[test]
    fn blocks_cargo_bin_path_ironlint_trust() {
        assert_blocks("/Users/me/.cargo/bin/ironlint trust");
    }

    #[test]
    fn blocks_relative_path_ironlint_trust() {
        assert_blocks("./ironlint trust");
    }

    // --- F5: global flags before the `trust` subcommand ---
    // `ironlint -v trust` and `ironlint --verbose trust` (no-value global
    // flags) must block. NOTE: `ironlint --config x.yml trust` is rejected by
    // clap at runtime (`--config` is a per-subcommand flag, not global), so a
    // model emitting it can't actually run trust that way — but the no-value
    // global-flag forms are real and must block.
    #[test]
    fn blocks_global_verbose_flag_before_trust() {
        assert_blocks("ironlint -v trust");
    }

    #[test]
    fn blocks_global_long_verbose_flag_before_trust() {
        assert_blocks("ironlint --verbose trust");
    }

    #[test]
    fn blocks_global_quiet_flag_before_trust() {
        assert_blocks("ironlint -q trust");
    }

    // --- F6: additional write primitives (dd, install, rsync, sponge) ---
    #[test]
    fn blocks_dd_of_ironlint_yml() {
        assert_blocks("dd if=/dev/zero of=.ironlint.yml bs=1 count=1");
    }

    #[test]
    fn blocks_dd_of_gate_script() {
        assert_blocks("dd of=.ironlint/gates/x.sh");
    }

    // `dd of .ironlint.yml` (SEPARATED form — `of` then the path) pins the
    // `tokens.get(i + 1)` next-token lookup. Under a `+ -> -` mutation the
    // check would read the token BEFORE `of` (not the path) and miss it.
    #[test]
    fn blocks_dd_separated_of_ironlint_yml() {
        assert_blocks("dd if=/dev/zero of .ironlint.yml");
    }

    #[test]
    fn blocks_install_onto_ironlint_yml() {
        assert_blocks("install -m 644 bad.yml .ironlint.yml");
    }

    #[test]
    fn blocks_rsync_onto_ironlint_yml() {
        assert_blocks("rsync bad.yml .ironlint.yml");
    }

    #[test]
    fn blocks_sponge_ironlint_yml() {
        assert_blocks("echo x | sponge .ironlint.yml");
    }

    // --- F?: subshell / brace-group grouping around a direct trust ---
    #[test]
    fn blocks_subshell_ironlint_trust() {
        assert_blocks("(ironlint trust)");
    }

    #[test]
    fn blocks_brace_group_ironlint_trust() {
        assert_blocks("{ ironlint trust; }");
    }

    // --- regression guards: the new detectors must NOT over-block reads ---
    // `ironlint --config x.yml check` is a legit read (flag before a read-only
    // subcommand) and must allow — pins that the global-flag-skip only fires
    // for `trust`, not every subcommand.
    #[test]
    fn allows_global_config_flag_before_check() {
        assert_allows("ironlint --config x.yml check");
    }

    // `nohup ironlint check` is a legit read wrapped in nohup — allow.
    #[test]
    fn allows_nohup_ironlint_check() {
        assert_allows("nohup ironlint check");
    }

    // `env ironlint doctor` — allow (read-only subcommand, even wrapped).
    #[test]
    fn allows_env_ironlint_doctor() {
        assert_allows("env ironlint doctor");
    }

    // A chained `ironlint check` after a separator must allow (it's a read).
    #[test]
    fn allows_chained_ironlint_check() {
        assert_allows("echo done && ironlint check");
    }

    // `ironlint xy trust` (a 2-char NON-flag token before `trust`) must ALLOW —
    // `xy` is not a flag, so the first non-flag token short-circuits. Pins
    // `is_flag_token`'s `&&`: under `||`, a 2-char token is mis-flagged, skipped,
    // and `trust` is reached → false block.
    #[test]
    fn allows_two_char_nonflag_before_trust() {
        assert_allows("ironlint xy trust");
    }

    // `ironlint check trust` (a non-flag subcommand before `trust`) must ALLOW
    // — `check` is the subcommand, not a flag, so scanning stops. Pins
    // `is_flag_token -> true`: under that mutant, `check` is mis-flagged,
    // skipped, and `trust` is reached → false block.
    #[test]
    fn allows_nonflag_subcommand_before_trust() {
        assert_allows("ironlint check trust");
    }

    // `cp .ironlint.yml /tmp/backup` via the multi-source form still allows
    // (policy path as SOURCE, not destination) — re-pin after dd/install work.
    #[test]
    fn allows_cp_from_ironlint_yml_still_allows() {
        assert_allows("cp .ironlint.yml /tmp/backup");
    }

    // `dd if=.ironlint.yml of=/tmp/backup` reads the policy file (source) and
    // writes a NON-policy destination — must allow.
    #[test]
    fn allows_dd_from_ironlint_yml_as_source() {
        assert_allows("dd if=.ironlint.yml of=/tmp/backup");
    }
}
