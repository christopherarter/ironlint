#!/usr/bin/env bash
#
# Runs INSIDE the container. $HOME is bind-mounted to a host dir, and the cwd
# (/work) is bind-mounted to another, so everything written below lands on the
# host for the run.sh assertions.
#
# Flow: seed the harness home dirs (so detection finds the open-source, no-auth
# harnesses) -> `ironlint init --yes` (bare = detect-then-install) -> capture
# `ironlint doctor` output.
set -euo pipefail

echo "== seeding harness homes for detection =="
# detect() checks: codex -> ~/.codex, pi -> ~/.pi,
# opencode -> $XDG_CONFIG_HOME/opencode (here ~/.config/opencode).
# ~/.claude is intentionally NOT created: claude-code is closed-source and
# excluded from this test, so init must skip it.
mkdir -p "$HOME/.codex" "$HOME/.pi" "$HOME/.config/opencode"

echo "== ironlint init --yes (detect + confirm-skipped + install) =="
cd /work
ironlint init --yes

echo "== ironlint doctor --format json (feature verifying itself) =="
set +e
ironlint doctor --format json >/work/doctor.json 2>/work/doctor.err
echo "$?" >/work/doctor.exit
set -e

echo "== drive.sh complete =="
