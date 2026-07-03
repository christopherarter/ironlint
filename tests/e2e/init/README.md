# `ironlint init` onboarding feature test (Docker)

An **opt-in, clean-room** end-to-end test that the `ironlint init` harness
onboarding actually materializes the right files on a real filesystem — not a
tempdir with injected env, but a fresh container with a real `$HOME` and the
real binary.

## What it does

1. **Builds a Linux `ironlint` in Docker** (multi-stage). The host is usually
   macOS, so a host-built binary can't run in a Linux container — the image
   compiles `ironlint` from source. The build context is the repo root because the
   adapter registry `include_str!`s `adapters/<harness>/`.
2. **Seeds harness homes** (`~/.codex`, `~/.pi`, `~/.config/opencode`) so a
   bare `ironlint init --yes` *detects* them. `~/.claude` is deliberately not
   seeded, so the closed-source claude-code adapter is excluded — this test
   covers the open-source, no-auth harnesses only.
3. **Runs `ironlint init --yes`** in a bind-mounted project dir, then captures
   `ironlint doctor --format json`.
4. **Asserts** (host-side) that the materialized hooks, sidecars, settings
   patches, and scaffolded config landed in the gitignored bind-mounted output.

No harness tools, Node/Bun, jq, or API keys are needed: `ironlint init` writes
files and patches JSON itself, and the test never executes the harnesses.

## Run it

```bash
bash tests/e2e/init/run.sh
```

Requires Docker. The first run compiles `ironlint` inside the image (slow); later
runs reuse the cached build layer. Output and forensics for each run go to
`tests/e2e/init/runs/<timestamp>/` (gitignored):

- `home/` — the container `$HOME` after init (materialized hook artifacts + `~/.config/ironlint/…`)
- `project/` — the init project dir (`.ironlint.yml`, pi/opencode plugins, `doctor.json`)
- `container.log` — stdout from the in-container `drive.sh`

`run.sh` exits non-zero if any assertion fails and prints which one.

## Why opt-in (not in `cargo test` / PR CI)

A Docker e2e harness used to exist and was removed (commit `e203d7a`) in favor of
fast, deterministic cargo-native adapter contract tests. This test is
intentionally **not** wired into `cargo test --workspace` or PR CI; it's a
manual acceptance check for the onboarding install path against a real
filesystem. The cargo-native unit/integration tests remain the fast gate.

## What it asserts

| Harness | Files |
|---|---|
| codex | `<project>/.codex/hooks.json` (PreToolUse + `adapters/codex/hook.sh` + `pre-tool-use`; project-scoped, since a bare `init --yes` is not `--global`); `~/.config/ironlint/adapters/codex/hook.sh` + `.ironlint-adapter.json` (sha256) |
| pi | `<project>/.pi/extensions/ironlint.ts` + `.ironlint-adapter.json` |
| opencode | `<project>/.opencode/plugins/ironlint.ts` + `.ironlint-adapter.json` |
| init | scaffolded `<project>/.ironlint.yml`; blessed `~/.config/ironlint/trust.json` |
| exclusion | no `~/.config/ironlint/adapters/claude-code` (claude-code skipped) |
| doctor | `doctor.json` reports codex, pi, opencode as adapter rows |
