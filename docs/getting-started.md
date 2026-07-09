# Getting started

IronLint reads a `.ironlint.yml` from your repo root and checks each file an agent edits against the checks you define. This page takes you from an empty repo to a check that blocks a real edit.

## Install

Build the binary from source:

```bash
cargo build --release
./target/release/ironlint --version
```

Put `./target/release/ironlint` on your `PATH` so the rest of this guide can call `ironlint` directly. (Prebuilt binaries and a one-line installer are in the [project README](../README.md).)

> **Windows:** IronLint runs checks via `sh -c` and requires a POSIX shell. On Windows, run it inside **Git Bash** or **WSL** — a stock PowerShell/CMD environment has no `sh`, so `ironlint check` will exit `1` (a loud config-tier error) and `ironlint doctor` will report a failing `shell` row rather than silently fail-open. A `windows-latest` CI leg is planned (see `plans/readiness-review/`).

## Write your first check

Create a `.ironlint.yml` in your repo root:

```yaml
checks:
  no-debug:
    files: "src/**/*.ts"
    run: "! grep -n 'DEBUG'"
```

A check is two fields. `files` is the glob it watches; `run` is a shell command IronLint runs against each matching file. IronLint reads only the command's exit code: **any nonzero exit (1–125) blocks the edit**, `0` lets it through.

The `run` here negates a grep. `grep` exits `0` when it finds `DEBUG`, so `! grep …` succeeds when the proposed content is clean and fails when it isn't, and the nonzero exit blocks the edit. Grep reads the proposed post-edit content from stdin; `$IRONLINT_FILE` carries the path under check — there is no `{file}` templating.

## Trust the config

IronLint runs the commands in your config, so it refuses to run a config it hasn't been told to trust. Review the file, then bless it:

```bash
ironlint trust
```

This records a hash of the config and its `.ironlint/scripts/` scripts in `~/.config/ironlint/trust.json`. Any later edit to either invalidates the hash, and `ironlint check` refuses to run until you re-bless. See [The trust store](security/trust.md) for why.

## Run a check

Point `ironlint check` at a file:

```bash
ironlint check --file src/app.ts
```

If `src/app.ts` contains a `DEBUG` marker, the check exits nonzero, and `ironlint check` prints the verdict and exits `2`. A clean file exits `0`. Those exit codes are the contract your agent adapter keys off — see [Running checks](operating/running-checks.md) for the full table.

To check the *proposed* content of an edit before it lands on disk, pipe it in:

```bash
printf 'const x = "DEBUG"\n' | ironlint check --file src/app.ts --content -
```

The content arrives on the check's stdin, so a check can inspect the new bytes without them ever touching disk.

## Scaffold and connect your agent

The blank page is optional, and so is wiring your agent by hand. From a fresh project, one command does both:

```bash
ironlint init
```

`ironlint init` detects your stack and writes a starter `.ironlint.yml` (the same shape as the check above, tuned for Rust, Node, or Python), trusts it for you, then detects your installed agents — Claude Code, Codex, pi, OpenCode — and, after you confirm, installs IronLint's edit hook into each. From then on the check runs on every edit the agent makes; you never call `ironlint check` by hand.

Review the generated checks and adjust. If you change the config after init, re-run `ironlint trust`. Target a single agent with `--harness <name>`, wire all four with `--harness all`, or preview the writes with `--dry-run` — see the [CLI reference](reference/cli.md#ironlint-init) for every flag and the [adapter docs](adapters/README.md) for per-agent details.

## Where to go next

- [Anatomy of a check](writing-checks/README.md) — `files`, `run`, and the exit-code contract in depth
- [Check recipes](writing-checks/recipes.md) — grep checks, linters over stdin, whole-tree tools
- [Targeting files](configuring/targeting-files.md) — getting your `files:` globs right
- [Adapters overview](adapters/README.md) — wiring IronLint into your coding agent
