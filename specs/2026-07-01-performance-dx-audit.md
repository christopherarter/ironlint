# Performance & DX audit — `ironlint` 0.5.0

**Date:** 2026-07-01
**Status:** findings, no implementation — each item needs its own plan before code
**Touches (if acted on):** `ironlint-cli` (`Cargo.toml`, `commands/{check,doctor,explain,trust,validate}.rs`, `cli.rs`), `ironlint-core` (`runner.rs`, `disable.rs`, `trust.rs`, `config/{parser,extends}.rs`), root `Cargo.toml`
**Method:** two parallel code audits (hot-path performance; CLI/DX surface) plus wall-clock measurements of the installed 0.5.0 binary

## Baseline measurements

Installed binary (`~/.cargo/bin/ironlint`, 8.6 MB, v0.5.0), this repo's config
(3 grep checks, 856 bytes, no `.ironlint/gates/`):

- `ironlint --version`: ~6 ms per invocation (20-run loop)
- `ironlint check --file <rs> --content -` (full pipeline: trust + load + 3
  checks): self-reported `elapsed_ms: 26`, ~33 ms wall

The base overhead is low. The findings below are about (a) dead weight paid on
every hook invocation, (b) redundant work that scales with config/check count,
and (c) DX friction at the surfaces humans actually touch.

---

## Performance findings

Ordered by estimated impact. The hot path is `main` → `commands/check.rs::run`
→ `trust::ensure_trusted` → `IronLintEngine::load` → per-file check dispatch;
it runs on every agent write.

### P1 — Self-updater + TUI dependency trees ride along on every write (HIGH)

`crates/ironlint-cli/Cargo.toml` links `axoupdater` (blocking) and `ratatui`
unconditionally. Only `ironlint update` and `ironlint watch` use them, but they
drag tokio, reqwest, hyper, rustls, ring, mio (axoupdater) and crossterm,
signal-hook (ratatui) into the single binary that `check` fork-execs on every
file write. There is no runtime init cost (main is sync; dispatch is on
demand) — the cost is binary size and page-in for a short-lived process,
which is exactly what cold start is made of.

**Options:**
- Cargo features (`self-update`, `watch`), default-on for the dist build so
  the shipped artifact is unchanged; `cargo install` users can opt out.
- Split `watch` (and possibly `update`) into a second binary so the hot-path
  `ironlint` stays lean. Tradeoff: two artifacts to ship.

### P2 — No release-profile tuning (HIGH)

Root `Cargo.toml` has no `[profile.release]` at all; `[profile.dist]` only
sets `lto = "thin"`. No `strip`, `codegen-units = 1`, or `panic = "abort"`
anywhere — so even the dist binary keeps unwind tables and symbols, and
`cargo install` users get stock release.

**Suggested:**

```toml
[profile.release]
lto = "fat"          # or keep thin to bound compile time
codegen-units = 1
strip = true
panic = "abort"      # main returns Result → process::exit; nothing catches unwinds
```

Runtime is dominated by subprocess spawns, so `opt-level = "s"` is worth
benchmarking — size (cold start) likely matters more than compute.
Tradeoffs: longer compile times; `panic = "abort"` needs a test-profile
override and a sweep for anything relying on `catch_unwind`.

### P3 — Config bytes read 2×, YAML-parsed up to 4× per invocation (HIGH, easy)

Three compounding redundancies, all pure fixed overhead before the first
check spawns:

1. `commands/check.rs::run` calls `trust::ensure_trusted` and then
   `IronLintEngine::load`; each independently reads and parses the whole
   `extends` closure.
2. `trust.rs` `compute_hash` → `config/extends.rs::collect_paths` does a full
   `parse_str` (validating deserialize) per file just to read the `extends:`
   list.
3. `config/parser.rs::parse_str` parses every document twice: once as
   `serde_yaml::Value` for legacy-key sniffing, once as `Config`.

For an N-file extends chain: 2N disk reads, up to 4N serde_yaml parses of the
same bytes. Absolute cost is sub-millisecond for KB configs, but it is 100%
redundant work on the hottest path.

**Fixes, independent and stackable:**
- Sniff legacy keys only on the *error* path (parse as `Config` first; if
  that fails, then look for legacy markers). Halves the parses.
- Give trust's path collection a minimal `#[derive(Deserialize)] struct
  { #[serde(default)] extends: Vec<String> }` instead of full validation.
- Structural (later): fold trust hashing into the load pass so each file is
  read once, hashed, and parsed together. Tradeoff: couples two currently
  independent modules — trust enforcement deliberately lives at the CLI
  layer, so this needs care to keep `IronLintEngine::load` pure.

### P4 — Disable-directive scan is O(checks × file size) (MEDIUM, easy)

`runner.rs::skip_reason` calls `disable::is_disabled(content, check_id)` once
per check, and each call is a full line-scan of the proposed content. Scan
once per file, collect disabled ids into a `HashSet<String>`, membership-test
per check. No behavior change, no downside.

### P5 — Sequential check dispatch per write (MEDIUM, design-level)

Checks matching a single write run sequentially (~8 ms each for the grep
checks here; far worse for `cargo check`-class checks). AGENTS.md already
flags rayon-across-checks as a pre-commit follow-up; the same applies to
multiple checks matching one write. This is the improvement that matters most
as configs grow, and the one to design deliberately — it touches the
execution model (telemetry ordering, verdict folding, output interleaving).

### P6 — Trust re-hashes the gates dir per invocation (MEDIUM, mostly by design)

`trust.rs::compute_hash` re-reads every file under `.ironlint/gates/` on every
`check`, buffering all bytes into `Vec<(String, Vec<u8>)>` before hashing.
The re-hash itself is the direnv-style guarantee (accepted TOCTOU window) —
don't remove it. Two improvements:
- Stream each file into the `Sha256` hasher instead of buffering everything.
  Free win.
- Optional, opt-in only: memoize the verdict keyed by (mtime, size) of the
  closure + gate files. Weakens the guarantee (mtime is forgeable), so it
  must never be the default.

### P7 — `$IRONLINT_TMPFILE` re-materialized per check (LOW)

`runner.rs::maybe_materialize_tmpfile` writes the identical proposed content
to a fresh temp file for every check that references the token, and
`check_references_tmpfile` re-scans every step string per check. Materialize
lazily once per file, share the path, keep one cleanup guard at file scope.

### P8 — Telemetry micro-costs (LOW, mostly fine)

`telemetry.rs::append` runs `create_dir_all` on every call and does an
open/flock/write/unlock/close cycle per record (one per file in a multi-file
diff). Could create the dir once at load and batch multi-file records.
**Deliberately absent and correct:** no fsync/flush — do not add durability
sync to best-effort telemetry.

### Already right — do not "optimize"

- Timeout uses `wait_timeout::ChildExt` (no polling), kills and reaps on
  expiry (`engine/gate.rs`).
- stdout/stderr drained on dedicated threads, stdin fed from another —
  correct pipe-deadlock avoidance; the thread spawns are noise next to the
  `sh -c` fork/exec.
- Globset matchers built once per load and reused (`runner.rs::load_with`).
- `config_dir_canon` canonicalized once at load.
- Engine loads exactly once per process (`IRONLINT_DEBUG_LOAD_COUNT` assertion
  guards this); `--check` filtering mutates in place rather than reloading.

---

## DX findings

Ordered by estimated impact.

### D0 — README rebrand in flight (note, not a finding)

The working tree has an uncommitted README.md rewrite with 52
`ironlint`/`IronLint`/`IRONLINT_*` occurrences plus an untracked
`iron-lint.png`; the committed README has zero. If the rebrand ships as-is,
the README will reference `ironlint init`, `.ironlint.yml`, `$IRONLINT_FILE`,
and `# ironlint-disable:` while the binary, `docs/`, adapters, and the check
ABI are all still `ironlint`. The rename has to land across the whole surface
(binary name, config filename, env prefix, disable directive, trust-store
path, docs/) in one coordinated change — or the README holds until it does.

### D1 — `check` human output buries the reason (HIGH)

`commands/check.rs:206-233` (`emit`): on block, the check id/file/message go
to **stderr** while **stdout** gets the bare word `block`; on pass, stdout
gets `pass`. Redirect stdout and the *why* is gone. No summary line, no count,
no next-step hint — the flattest output in the tool at the primary human
touchpoint.

**Fix:** consolidated human report on one stream — summary line
(`blocked by 1 check (no-fixme), 2 passed`) plus the messages — keeping
`--format json` as the machine surface. Exit codes stay the contract.

### D2 — Trust mismatch indistinguishable from never-trusted (HIGH)

`trust.rs:184-191`: both cases fall through the same arm →
`` config/gates not trusted — review and run `ironlint trust` ``. The store
already holds the prior `TrustEntry { hash, blessed_at }` but surfaces
neither. Editing one line of your own config produces the same alarming
message as a brand-new untrusted repo.

**Fix:** branch the match — if an entry exists but the hash differs, say
"config or a gate script changed since you trusted it (blessed <date>) —
review your changes, then re-bless". Bonus: name which participating file's
hash moved (config vs a specific gate script). Keep naming the fix command;
that part is already right and tested.

### D3 — `explain` answers "in scope?" but not "will it run?" (HIGH)

`commands/explain.rs` reports per-check `match`/`skip` from the glob only —
it ignores lifecycle (`on: [pre-commit]` never fires on a write) and
`ironlint-disable:` directives. Meanwhile `check --explain` already computes
the richer `ExplainOutcome::{Fire, Pass, Skipped{reason}}`, so ironlint's two
"explain" surfaces disagree.

**Fix:** have `explain` reuse the engine's `ExplainOutcome` vocabulary so
glob + lifecycle + disable are all reflected and both surfaces speak the same
language.

### D4 — `doctor` never checks trust (MEDIUM)

`commands/doctor.rs` checks binary/config/parse/scripts/adapters but
deliberately dropped the trust probe — so it greenlights a setup that
`ironlint check` will immediately reject with exit 1. Untrusted config is the
single most common first-run failure.

**Fix:** add a `trust` row (pass if blessed & current; warn with
`` remediation: run `ironlint trust` `` otherwise). The `remediation:` field
already exists to hang it on.

### D5 — No shell completions (MEDIUM)

No `clap_complete`, no `completions` subcommand. Add
`ironlint completions <bash|zsh|fish|...>`; dynamic completion of `--check` ids
from the resolved config would be a nice touch.

### D6 — No color, no `NO_COLOR` (MEDIUM)

Only the watch TUI has color. Blocked verdicts and doctor `[fail]` rows
render identically to passes. Colorize block/error red, pass green in human
mode, gated on `is_terminal()` **and** `NO_COLOR` / `--color=auto|always|never`;
reuse the watch palette for consistency.

### D7 — Raw YAML syntax errors get no ironlint framing (MEDIUM)

`config/parser.rs` curates *semantic* errors beautifully (legacy markers,
run-xor-steps, empty run, unknown field) but a bad indent / unclosed quote
surfaces raw serde_yaml text under `parsing ironlint config`. Append a hint
(`` run `ironlint schema` for the format, or `ironlint validate` to re-check ``)
on the syntax-error path — the abrupt quality drop is the friction.

### D8 — Single-check dry-run exists but is undiscoverable (MEDIUM)

The affordance is real — `ironlint check --file X --check <id> --force`, or
`--content -` for proposed bytes — but it's a flag stack with coupled
requirements (`--force` needs `--check`; `--content` needs `--file`) and
nothing in `--help` frames it. Add a thin `ironlint test <check-id> [<file>]`
alias (maps to the flag stack, prints the check's verbatim output), or at
minimum a "Testing a check" example in `check --help`.

### D9 — Small polish (LOW)

- `--file` and `--diff` have empty help strings in `check --help`
  (`cli.rs`).
- `--config` is re-declared on five subcommands; hoist it (plus
  `--color`/`-q`) to global args on `Cli`.
- Bare `ironlint check` errors with `provide exactly one of --file or --diff`;
  consider defaulting to staged files (mirrors the pre-commit hook) or at
  least pointing the error at an example invocation.
- `validate` (`ok: N check(s)`) and `trust` (`trusted: <path>`) are dead
  ends — append next-step nudges (validate → "now run `ironlint trust`",
  trust → "try `ironlint check --file <f>`").

### Already-good DX — preserve, don't re-do

- Curated legacy-config rejection naming the found key and pointing at the
  checks spec; `gates:` → `checks:` rename hint.
- Config guardrails: run XOR steps, empty/comment-only `run` rejected with
  block-scalar guidance, unknown fields are hard errors naming the field,
  location-aware (`check `id` step N`).
- Trust failure names its own fix and is tested to.
- `doctor`'s actionable `remediation:` field.
- Read-only commands intentionally run against unblessed configs so you can
  debug before trusting.
- `--event` constrained at the arg layer so typos never reach
  `$IRONLINT_EVENT`.
- `--content -` stdin dry-run, documented in
  `docs/operating/running-checks.md`.
- `docs/reference/cli.md` is comprehensive (flags, exit codes, env vars) —
  the doc gaps live only in the README draft, not `docs/`.
- `update`'s non-installer failure path prints the exact
  `cargo install --git … --force` command instead of a dead error.
- `init` plan-and-confirm onboarding (dry-run, scope honesty, non-TTY
  behavior, idempotent re-runs, foreign-hook-safe git hook) is shipped and
  solid.

### Env-var inventory (code-backed)

`IRONLINT_TIMEOUT` (runner.rs), `IRONLINT_FAIL_CLOSED_ON_INTERNAL` (adapters
only — hook scripts, not the core binary), `IRONLINT_EVENT` / `IRONLINT_FILE` /
`IRONLINT_FILES` / `IRONLINT_ROOT` / `IRONLINT_TMPFILE` (check ABI),
`IRONLINT_DEBUG_LOAD_COUNT` (undocumented debug assertion hook),
`XDG_CONFIG_HOME`/`HOME` (trust-store location). No `NO_COLOR` support
anywhere yet (see D6).

---

## Suggested sequencing

1. **Small, high-value, test-friendly:** P4 (disable-scan HashSet), D1
   (check output rework), D2 (trust-message split), D9 (help-text /
   next-step polish).
2. **Biggest measurable win:** P1 + P2 together (binary diet + profile
   tuning) — verify with before/after size and cold-start numbers.
3. **Easy hot-path cleanups:** P3 (parse dedup), P7 (tmpfile share), P6
   streaming hash.
4. **Design deliberately:** P5 (parallel dispatch per write) — touches the
   execution model; D3 (explain unification) — touches the ExplainOutcome
   surface shared with `check --explain`.
