# `hector-config` authoring skill — design

**Status:** design (brainstormed 2026-06-28). Supersedes nothing; extends the
onboarding feature in `specs/2026-06-27-hector-init-onboarding-design.md`.

## Goal

After `hector init`, a harness has the enforcement hook but no idea how to
*author* a gate. Close that gap: ship one canonical gate-authoring guide,
install it as a real Agent-Skills `SKILL.md` into every detected harness during
`hector init`, and expose it on demand via a new `hector schema` command.

## Problem

`hector init` wires the edit hook but installs no authoring knowledge. The only
authoring skill that exists — claude-code's `hector-author` — ships with the
*plugin* (not `init`), so reasonix/pi/opencode get nothing, and even claude-code
gets it only if the plugin is installed. (Those skills were also pre-0.3 until
`16de991`.) The result: an agent can be *blocked* by a gate but can't be asked
to *write or fix* one, because it was never taught the `{files, run}` schema.

## Key finding that shapes the design

All four supported harnesses implement the Agent Skills spec (`SKILL.md` with
`name` + `description` frontmatter), discovering skills from on-disk directories:

| Harness | Project skills dir | Global skills dir |
|---|---|---|
| claude-code | `.claude/skills/<name>/SKILL.md` | `~/.claude/skills/<name>/SKILL.md` |
| pi | `.pi/skills/<name>/SKILL.md` | `~/.pi/agent/skills/<name>/SKILL.md` |
| opencode | `.opencode/skills/<name>/SKILL.md` | `~/.config/opencode/skills/<name>/SKILL.md` |
| reasonix | `.reasonix/skills/<name>/SKILL.md` | `~/.reasonix/skills/<name>/SKILL.md` |

So the delivery is uniform: a real `SKILL.md` per harness. No AGENTS.md pointer,
no per-harness fallback. **Cross-read caveat:** opencode is Claude-compatible —
it *also* reads `.claude/skills/`. Writing both `.opencode/skills/` and
`.claude/skills/` would make opencode load the same-named skill twice (a
duplicate-name warning). See the dedup rule below.

## Decisions (from brainstorm)

1. **Single source**, embedded in the binary via `include_str!` (same pattern as
   the adapter hook/plugin artifacts in `registry.rs`).
2. **Per-harness delivery is a real `SKILL.md`** installed by `hector init`.
3. **Consolidate** — the shared guide is the sole authoring source; the
   hand-maintained `hector-author` skill is retired (no second copy to drift).
4. **CLI name:** `hector schema`.
5. **Skill name:** `hector-config` (valid Agent-Skills name: lowercase, hyphen,
   ≤64 chars; installed into a dir of the same name).

## Architecture

### Component 1 — the canonical guide

`adapters/shared/hector-config/SKILL.md` — one file, the single source of truth.
Agent-Skills frontmatter (`name: hector-config`, a `description` that triggers on
"add/write/change a hector gate") followed by the harness-agnostic authoring
guide. Content is the generalized form of the just-rewritten `hector-author`
body:

- the `{files, run}` gate model; exit `2` blocks, other codes pass;
- the ABI: `$HECTOR_FILE` (absolute), proposed content on stdin, `$HECTOR_ROOT`,
  `$HECTOR_EVENT`; no path templating;
- file-scope semantics (a bare pattern without `/` also matches `**/<pattern>`);
- the three patterns: grep-ban (the `case $?` form), linter-over-stdin, and the
  multi-line block-scalar (`run: |`) idiom (ties to the parser validation in
  `c199858`);
- the fixture-test loop (`hector check --file … --content - --gate <id>`);
- `hector trust` re-bless after edits.

It must contain no retired-model vocabulary (engines/severity/`{file}`/
`violations`) — enforced by the existing drift guard, extended to this file.

### Component 2 — `hector schema` (CLI)

`include_str!("../../../adapters/shared/hector-config/SKILL.md")` into the
binary. A new clap subcommand `hector schema` prints the guide to stdout and
exits `0` (read-only; no config load, no trust check). It prints the guide
**body with the YAML frontmatter stripped**, so the output reads as
documentation rather than a skill wrapper. This is the universal fallback any
agent reaches by shelling out, and the human-facing way to read the format.

### Component 3 — registry skill specs

Extend the adapter registry (`crates/hector-core/src/adapter/registry.rs`) with,
per harness, the project and global skills directory (the table above) and the
embedded `SKILL.md` bytes. The four harnesses already have entries; add a
`skills_local: fn(&AdapterEnv) -> PathBuf` / `skills_global: fn(&AdapterEnv) ->
PathBuf` (or an analogous `SkillSpec`) alongside the existing hook/plugin specs.
The skill is installed at `<skills_dir>/hector-config/SKILL.md`.

### Component 4 — install / uninstall (`ops.rs`)

A new `install_skill` / `uninstall_skill` pair, reusing the existing
`adapter::materialize` primitives:

- `install_skill`: resolve `<skills_dir>/hector-config/`, `atomic_write` the
  `SKILL.md`, write a `.hector-adapter.json` sidecar (sha256 + version). Idempotent
  — content-equal on disk → `AlreadyPresent`; changed → `Updated`; absent →
  `Installed`. (Mirrors `install_plugin`.)
- `uninstall_skill`: remove `<skills_dir>/hector-config/` (the `SKILL.md` + sidecar).
- Wired into `onboard::run_hook_phase`: for each selected harness, after the hook
  install, also install the skill. Outcomes fold into the same per-harness
  reporting and `any_ok`/`any_fail` exit logic. `--dry-run` previews the skill
  write; `--uninstall` removes it. Because skill install lives in the hook phase,
  it follows the same flags as the hook: `--no-hook` skips it (config only),
  `--hook-only` still installs it.

**opencode/claude dedup rule:** in the onboard phase, if the selected set
contains *both* `claude-code` and `opencode`, skip opencode's skill write —
opencode reads the `.claude/skills/` copy. This holds at the active scope:
opencode reads claude's project dir (`.claude/skills/`) and global dir
(`~/.claude/skills/`) alike, so the dedup applies whether init runs project-local
or `--global`. (claude-code is the only dir opencode cross-reads among the four
native namespaces, so this is the only special case; pi and reasonix namespaces
are read by no other harness.)

### Component 5 — consolidation

- Move the authoring content out of
  `adapters/claude-code/skills/hector-author/SKILL.md` into
  `adapters/shared/hector-config/SKILL.md`, generalized to be harness-agnostic.
- **Delete** `adapters/claude-code/skills/hector-author/` — retired, superseded
  by the init-installed `hector-config` skill + `hector schema`. `hector-init`,
  `hector-review`, and the runtime `hector` skill are untouched.
- Update the docs that name `hector-author`:
  `docs/adapters/README.md` ("Managing policy from inside the agent" lists the
  three skills) and `docs/adapters/claude-code.md`. Reframe authoring around the
  `hector-config` skill that `hector init` installs (for *every* agent now, not
  just claude-code) and `hector schema`.
- Extend the drift guard (`crates/hector-cli/tests/skills_gates_model.rs`) to
  scan `adapters/shared/hector-config/SKILL.md`.

## Data flow

```
adapters/shared/hector-config/SKILL.md   (the one source)
        │ include_str!
        ▼
   hector binary
     ├─ `hector schema`            → prints the guide (frontmatter stripped)
     └─ `hector init` (per detected harness):
          ├─ install hook            (existing)
          └─ install hector-config skill → <harness skills dir>/hector-config/SKILL.md
                                          (+ .hector-adapter.json sidecar)
        ▼
   harness loads the skill on-demand → agent authors/fixes gates against the real schema
```

## Error handling

- Skill-install failures surface as a per-harness `Failed`/outcome line, same as
  hook installs; one harness failing doesn't abort the others. Exit `3` only if
  every attempted install failed (existing `run_hook_phase` semantics).
- Re-runs are idempotent (content-equality short-circuit).
- `--dry-run` writes nothing and lists the intended skill path.

## Testing

- **Drift guard** extends to `adapters/shared/hector-config/SKILL.md`: no retired
  vocabulary; gates-model anchors (`$HECTOR_FILE`, `run:`, `files:`, `exit 2`)
  present.
- **`hector schema`**: prints the guide, exit `0`, output contains `$HECTOR_FILE`
  and `exit 2`, and does *not* leak the YAML frontmatter (no leading `---`).
- **Install/uninstall round-trips** (per harness, in `ops.rs` unit tests +
  `cli_init`): the `SKILL.md` lands at the correct path with a sidecar; idempotent
  re-install → `AlreadyPresent`; `--uninstall` removes the dir.
- **Dedup**: with both claude-code and opencode selected, opencode's own
  `.opencode/skills/` is *not* written; the `.claude/skills/` copy exists.
- **Region coverage ≥80%** on new `src/` (the `schema` command + skill ops).
- **Docker e2e** (`tests/e2e/init/`): extend the in-container assertions to verify
  the `hector-config/SKILL.md` lands for reasonix, pi, and opencode.

## Out of scope (YAGNI)

- AGENTS.md pointer / any non-skill fallback — dropped; all four harnesses have
  real skill discovery.
- `--format` on `hector schema`.
- An opt-out flag for skill install (it rides the existing init confirm/`--yes`).
- Porting `hector-init` / `hector-review` to portable skills — separate concern.

## Locked parameters

- Canonical source: `adapters/shared/hector-config/SKILL.md`
- Skill name / dir: `hector-config`
- CLI command: `hector schema`
- Skills dirs: per the table above; reasonix = `.reasonix/skills/` (project),
  `~/.reasonix/skills/` (global)
