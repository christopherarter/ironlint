# Hector 0.3 — the "gates" redesign

**Status:** design, approved direction (2026-06-15)
**Supersedes:** the `engine`/`script`/`ast` rule model from `specs/2026-05-11-hector-plan-and-0.1-design.md`
**Breaking:** total. No migration path — hector is not yet distributed, so the old config shape is simply unsupported.

## 1. Thesis

A bash script plus a `case` statement is enough enforcement for one harness, one
repo, one person. Hector earns its place only when the *same* enforcement pattern
must survive across harnesses (Claude Code, opencode, pi, reasonix, aider, …) and
projects without every install becoming bespoke shell glue.

So hector owns the boring, portable substrate and **knows nothing about any tool**:

- **Harness normalization** — every adapter collapses a different hook payload into
  one stable ABI (env vars + stdin + cwd). The same gate script runs unchanged
  everywhere. *This is the moat.*
- **Dispatch** — match touched files to gates, run the right commands.
- **Trust** — never execute repo shell until the config and its gate scripts have
  been reviewed and blessed *on this machine*.
- **A hard-block contract** — stable exit codes, stable output, no "warning"
  ambiguity.
- **Proof** — `hector verify` proves an intentional violation actually blocks
  through the *real* harness wiring. `hector doctor` reports static wiring
  ("hook installed? gate ever reached?").
- **Telemetry** — what ran, what blocked, how long it took.
- **Safe execution defaults** — timeouts, cwd anchoring, env setup.

What hector explicitly does **not** own: knowledge that biome is stdin-capable,
that depcruise needs a truthful tree, a taxonomy of check kinds, or a model of any
CLI. The *scripts* own tool behavior. Hector owns the guarantees that make those
scripts reliable instead of folklore.

## 2. The config language (the whole thing)

```yaml
# .hector.yml
extends: []          # optional; existing cycle-checked DFS, local gates win on collision
execution:           # optional
  timeout_secs: 30   # per-gate wall-clock; default 30
gates:
  biome:
    files: ["**/*.{ts,tsx}"]
    run: ".hector/gates/biome.sh"
  depcruise:
    files: ["src/**/*.{ts,tsx}"]
    run: ".hector/gates/depcruise.sh"
  no-console:
    files: ["**/*.ts"]
    run: "! grep -nH 'console.log' \"$HECTOR_FILE\" || exit 2"
```

A **gate** is exactly two fields:

- `files` — one glob or a list of globs. Scope semantics are unchanged from today
  (`config/scope.rs`): a bare pattern without `/` also matches at any depth, so
  `*.py` ≡ `**/*.py`. Do not "fix" this; it is deliberate bully parity.
- `run` — a shell command string, handed to the shell **verbatim**. Hector does
  **no** string templating — no `{file}`, no `{path}`. Everything a gate needs
  arrives through the ABI (§4). `run` may be an inline command or a path to a
  script under `.hector/gates/`; the shell makes no distinction.

Removed from the language entirely: `engine`, `script`, `pattern`, `language`,
`severity`, `description`, `capabilities`, `output`, `trust:`, `schema_version`,
`baseline`.

## 3. The verdict contract (the gate owns the verdict)

Hector runs `run` and reads **only its exit code**:

| gate exit code               | meaning                          | contributes to hector verdict |
| ---------------------------- | -------------------------------- | ----------------------------- |
| `2`                          | **Block**                        | Block                         |
| `0`, `1`, `3`–`125`          | **Pass**                         | —                             |
| `126`, `127`                 | not executable / not found       | InternalError                 |
| `≥128` (killed by signal)    | gate crashed                     | InternalError                 |
| wall-clock timeout           | gate hung                        | InternalError                 |

The gate decides. A tool that exits 1 on findings (phpstan, eslint, grep-match) is
treated as a **pass** unless the script remaps it to 2 — `phpstan … || exit 2`,
`biome check --error-on-warnings …`, `grep -q PATTERN && exit 2`. This is the
flexibility point: a gate can be `claude -p "did this violate policy?"` that exits
2 or 0 on its own judgement, with no special support from hector.

"Exit 2 to count" deliberately makes blocking **opt-in per gate** and removes the
warning tier: a check either blocks or it doesn't. There is no `severity`.

A *broken* gate is never a silent pass. Command-not-found, a wall-clock timeout
(`execution.timeout_secs`), or death by signal map to **InternalError**, not Pass.

### Output

On Block, the gate's combined stdout+stderr is passed through **verbatim** as the
block message (today's `passthrough`, now the only behavior). No parsing, no
`file:line:col` extraction, no format zoo. If both streams are empty, the message
is `"<gate-id> blocked"`.

### Hector's own (outer) exit codes — unchanged, adapters depend on them

- `0` — Pass (no gate blocked)
- `1` — config / load error (untrusted, parse failure, missing file)
- `2` — Block (≥1 gate exited 2)
- `3` — InternalError (≥1 gate crashed: 127 / timeout / signal). Adapters fail-open
  by default; `HECTOR_FAIL_CLOSED_ON_INTERNAL=1` flips to fail-closed.

## 4. The ABI (locked stability surface)

This is the contract every adapter must satisfy and every gate script may rely on.
Treat it like the verdict JSON: a public surface, versioned, not broken casually.

| channel        | value                                                        |
| -------------- | ----------------------------------------------------------- |
| `$HECTOR_FILE` | absolute path to the file under check                       |
| `$HECTOR_ROOT` | project root (directory containing `.hector.yml`)           |
| `$HECTOR_EVENT`| trigger: `edit` \| `write` \| `pre-commit` \| `manual`      |
| **stdin**      | the proposed post-edit content of the file (may be empty)   |
| **cwd**        | `$HECTOR_ROOT`                                               |

A gate that wants to check the *proposed* edit before it lands reads stdin
(`biome check --stdin-file-path "$HECTOR_FILE"`). A gate that needs a truthful
on-disk tree ignores stdin and reads `$HECTOR_FILE` / scans `$HECTOR_ROOT`
(depcruise). The path travels only as an env value, never spliced into command
text — no quoting or metacharacter footguns.

## 5. Execution model — per file

One `run` invocation per matching file. In the PostToolUse hook there is always
exactly one changed file, so this is the only path that matters there. In a batch
run (`hector check` over a tree, CI over a PR diff) a whole-project gate like
depcruise re-runs once per changed file; that is redundant but **correct**, because
such gates are idempotent. Batch de-duplication is explicitly out of scope for
0.3 (YAGNI) — revisit only if it bites.

## 6. Trust — the direnv model

The previous trust gate stored a sha256 of the YAML *inside that same YAML*. That
is theater against an adversary (the malicious repo author just writes the correct
hash) and it never covered the gate scripts — where the dangerous code now lives.
Replace it with a real, out-of-repo allow-list:

- **Trust store:** `~/.config/hector/trust.json` (XDG-respecting), keyed by the
  canonical absolute path of `.hector.yml`.
- **Trust hash:** sha256 over the normalized `.hector.yml` bytes **plus the bytes
  of every file under `.hector/gates/`** (sorted by relative path). Hashing the
  whole gates directory is conservative and needs no shell parsing: any change to
  any gate script invalidates trust. *Limitation, documented:* trust covers the
  config and `.hector/gates/`; a `run` that sources scripts elsewhere is the
  author's responsibility.
- **On check:** recompute the hash; compare to the blessed entry. Match → run.
  Mismatch or missing → **fail closed**: refuse to run any gate, exit `1`,
  message `config/gates not trusted — review and run \`hector trust\``. No
  interactive auto-prompt (the hook has no human); blessing is always the explicit
  `hector trust` action.
- **`hector trust`** recomputes and writes the blessed hash to the store.
  **`hector init`** auto-blesses the config it just wrote.
- **Property for free:** a fork PR (or an agent) that edits a gate script
  invalidates trust, so it cannot silently execute or weaken gates until a human
  re-blesses. This is the security story hector did not actually have before.

## 7. Proof — `hector verify` (the flagship) + `hector doctor`

The #1 failure mode of agent hooks is silent non-execution: the hook isn't
installed, the payload field moved, the gate was never reached. Two commands close
that gap:

- **`hector verify`** — *dynamic* proof. For each gate, synthesize a known-bad
  input and confirm the gate exits 2, **driven through the installed harness
  adapter** (not just by calling the binary). Reports per gate:
  `blocks ✓ / does not block ✗ / never reached ✗ / hook not installed ✗`.
  Gates supply their fixture via convention: an executable
  `.hector/gates/<gate>.bad` (or a `# hector:bad-fixture <path>` hint); gates
  without a fixture report `unproven` rather than passing.
- **`hector doctor`** — *static* diagnostics (extends today's command): is each
  adapter's hook installed and pointing at this hector? Is `.hector.yml` parseable
  and trusted? Does every `run` script exist and is it executable?

## 8. CLI surface for 0.3

| command            | fate                                                                 |
| ------------------ | ------------------------------------------------------------------- |
| `check`            | core; keep `--file`, `--content`/stdin, `--config`, `--format`, `--explain`, `--allow-external-paths`. Drop `--diff`-specific rule semantics that assumed parsing. |
| `trust`            | reworked: write blessed hash to the out-of-repo store (no longer writes the YAML). |
| `verify`           | **new** — dynamic proof (§7).                                        |
| `doctor`           | keep + expand (§7).                                                  |
| `init`             | keep — scaffold `.hector.yml` + `.hector/gates/` stubs; auto-bless.  |
| `validate`         | keep — parse + lint config (now trivially small).                   |
| `explain` / `show-resolved-config` / `guide` | keep, adjusted to the gate model.         |
| `migrate`          | **deleted** — no install base.                                       |
| `baseline`         | **deleted** — incompatible with opaque exit-2; grandfathering moves into the gate script. |

## 9. Verdict JSON (SCHEMA_VERSION bump)

Still emitted for adapters; simplified to the gate model. Treat as a locked surface.

```json
{
  "schema": 4,
  "status": "pass" | "block" | "internal_error",
  "blocks": [
    { "gate": "biome", "file": "src/x.ts", "message": "<verbatim stdout+stderr>" }
  ],
  "errors": [
    { "gate": "depcruise", "file": "src/y.ts", "reason": "timeout" | "not_found" | "signal:9" }
  ]
}
```

`Severity`, `Engine`, the `OutputMode`/parsed-record types, and the `Capabilities`
types are deleted from the public surface.

## 10. Telemetry & disable directives

- **Telemetry** (`.hector/log.jsonl`, append-only) survives, keyed to the gate
  model: one record per gate×file with `gate`, `file`, `exit_code`, `verdict`,
  `duration_ms`, `event`.
- **Inline disable** survives, renamed to the gate vocabulary:
  `hector-disable: <gate-id>` in a checked file suppresses that gate for that file
  (one gate per directive; directive ends at whitespace/`*`/`/`, as today).

## 11. Code impact (orientation, not a plan)

Deleted: `engine/ast.rs`, `engine/output.rs`, `engine/capability.rs`, the
`EngineKind`/`Severity`/`Capabilities`/`OutputMode` types, `baseline.rs`,
`commands/migrate.rs`, `commands/baseline.rs`, the `ast-grep-core` dependency, and
the v1/v2 schema apparatus (`SUPPORTED_SCHEMAS`, `is_legacy`,
`peek_schema_version`).

Reshaped: `config/types.rs` → `Config { extends, execution?, gates }` +
`Gate { files, run }`; `engine/script.rs` → a single `run_gate` (spawn, timeout,
read exit code, passthrough on 2); `runner.rs` → load → trust-verify (new store) →
scope-match → run gate per file → telemetry; `trust.rs` → out-of-repo store +
gates-dir hashing; `verdict.rs` → the §9 shape.

Kept ~as-is: `config/scope.rs`, `config/extends.rs`, `disable.rs`, `telemetry.rs`
(retargeted), the adapters (must now speak the §4 ABI exactly).

## 12. Testing

- Unit: gate exit-code → verdict mapping (2/0/1/127/timeout/signal); timeout kill;
  stdin delivery; passthrough message assembly; scope matching unchanged.
- Trust: bless → run; edit a gate script → fail closed; edit config → fail closed;
  unblessed → exit 1.
- E2E (`assert_cmd` against the compiled binary): the three example gates above,
  block vs pass via exit code, `--content`/stdin pre-write gating, `hector verify`
  green/red, `hector doctor`.
- Coverage gate (≥80% region per file) and cognitive-complexity cap (15) still
  apply to every touched file.

## 13. Resolved decisions

1. **Timeout** — `execution.timeout_secs` defaults to **30**, overridable per-run by
   the `HECTOR_TIMEOUT` env var (env wins over config).
2. **`hector verify` fixtures** — convention is an executable
   `.hector/gates/<gate>.bad` per gate; a gate with no fixture reports `unproven`
   rather than passing. No `verify:` YAML field (keeps a gate to two fields).
3. **Sandboxing** — dropped for 0.3. The per-gate timeout is the only execution
   rail. Global network-off may return as an execution default in a later phase.
