#!/usr/bin/env bash
#
# Opt-in onboarding feature test for `hector init`.
#
# Builds a Linux `hector` in Docker, runs a bare `hector init --yes` in a clean
# container against seeded harness homes, and asserts the materialized hook
# artifacts + settings patches appear in the gitignored, bind-mounted output
# dirs. Targets the open-source, no-auth harnesses only (reasonix, pi, opencode);
# claude-code is excluded by not seeding ~/.claude.
#
# Usage:   bash tests/e2e/init/run.sh
# Requires Docker. NOT part of `cargo test` and NOT run in PR CI — the first
# build compiles hector inside the image (slow); later runs cache.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
IMAGE="hector-init-e2e:latest"
RUN_ID="$(date +%Y%m%d-%H%M%S)-$$"
OUT="$HERE/runs/$RUN_ID"
HOME_DIR="$OUT/home"
PROJ_DIR="$OUT/project"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found on PATH — install Docker to run this test." >&2
  exit 127
fi

mkdir -p "$HOME_DIR" "$PROJ_DIR"
echo "run dir: $OUT"

echo "== building image (compiles a linux hector; first run is slow) =="
docker build -f "$HERE/Dockerfile" -t "$IMAGE" "$REPO_ROOT"

echo "== running container =="
# Run as the host UID/GID so writes to the bind-mounted dirs are owned correctly
# on native Linux Docker (where the image's baked-in uid 1000 may not match the
# host user). $HOME is bind-mounted, so the container needs no real home dir.
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -v "$HOME_DIR:/home/tester" \
  -v "$PROJ_DIR:/work" \
  -e HOME=/home/tester \
  -w /work \
  "$IMAGE" | tee "$OUT/container.log"

# ---------- host-side assertions (portable: test + grep, no jq) ----------
fail=0
pass() { printf '  ok   %s\n' "$1"; }
miss() { printf '  FAIL %s\n' "$1"; fail=1; }
exists() { if [ -e "$1" ]; then pass "$2"; else miss "$2 -> missing: $1"; fi; }
executable() { if [ -x "$1" ]; then pass "$2"; else miss "$2 -> not executable: $1"; fi; }
# Fixed-string grep so regex metachars in the needle (e.g. the '.' in a path)
# can't widen the match.
contains() {
  if [ -f "$1" ] && grep -qF -- "$2" "$1"; then pass "$3"; else miss "$3 -> grep '$2' in $1"; fi
}

echo "== assertions =="

# reasonix: user-global settings patch + materialized hook under the hector dir.
exists "$HOME_DIR/.reasonix/settings.json" "reasonix settings.json present"
contains "$HOME_DIR/.reasonix/settings.json" "PreToolUse" "reasonix PreToolUse entry"
contains "$HOME_DIR/.reasonix/settings.json" "adapters/reasonix/hook.sh" "reasonix hook command path"
contains "$HOME_DIR/.reasonix/settings.json" "pre-tool-use" "reasonix entry arg"
exists "$HOME_DIR/.config/hector/adapters/reasonix/hook.sh" "reasonix hook.sh materialized"
executable "$HOME_DIR/.config/hector/adapters/reasonix/hook.sh" "reasonix hook.sh is executable"
exists "$HOME_DIR/.config/hector/adapters/reasonix/.hector-adapter.json" "reasonix sidecar present"
contains "$HOME_DIR/.config/hector/adapters/reasonix/.hector-adapter.json" "sha256:" "reasonix sidecar has sha256"

# pi: project-local plugin drop-in.
exists "$PROJ_DIR/.pi/extensions/hector.ts" "pi plugin hector.ts"
exists "$PROJ_DIR/.pi/extensions/.hector-adapter.json" "pi sidecar present"

# opencode: project-local plugin drop-in.
exists "$PROJ_DIR/.opencode/plugins/hector.ts" "opencode plugin hector.ts"
exists "$PROJ_DIR/.opencode/plugins/.hector-adapter.json" "opencode sidecar present"

# Authoring skill: hector init installs hector-config/SKILL.md into each wired
# agent's skills dir (project-local; claude-code excluded so opencode is not
# deduped here).
exists "$PROJ_DIR/.reasonix/skills/hector-config/SKILL.md" "reasonix authoring skill"
exists "$PROJ_DIR/.pi/skills/hector-config/SKILL.md" "pi authoring skill"
exists "$PROJ_DIR/.opencode/skills/hector-config/SKILL.md" "opencode authoring skill"
contains "$PROJ_DIR/.pi/skills/hector-config/SKILL.md" "name: hector-config" "pi skill has frontmatter"
exists "$PROJ_DIR/.pi/skills/hector-config/.hector-adapter.json" "pi skill sidecar"

# init itself: scaffolded + blessed config.
exists "$PROJ_DIR/.hector.yml" "scaffolded .hector.yml"
exists "$HOME_DIR/.config/hector/trust.json" "blessed trust.json"

# claude-code must NOT have been installed (no ~/.claude was seeded).
if [ -e "$HOME_DIR/.config/hector/adapters/claude-code" ]; then
  miss "claude-code must be excluded (open-source-only) but an artifact exists"
else
  pass "claude-code excluded (no artifact)"
fi

# doctor exited cleanly and reports each harness as a *passing* adapter row.
DOCTOR="$PROJ_DIR/doctor.json"
exists "$DOCTOR" "doctor.json captured"
DEXIT="$(cat "$PROJ_DIR/doctor.exit" 2>/dev/null || echo missing)"
if [ "$DEXIT" = "0" ]; then
  pass "doctor exited 0"
else
  miss "doctor exited '$DEXIT'"
  [ -s "$PROJ_DIR/doctor.err" ] && { echo "  --- doctor stderr ---"; sed 's/^/  /' "$PROJ_DIR/doctor.err"; }
fi
# The "status" line immediately follows the "name" line in serde's pretty JSON,
# so grep -A1 asserts per-harness pass without needing jq.
status_pass() {
  if grep -A1 "\"name\": \"$1\"" "$DOCTOR" 2>/dev/null | grep -qF '"status": "pass"'; then
    pass "doctor: $1 status pass"
  else
    miss "doctor: $1 status not pass"
  fi
}
for h in reasonix pi opencode; do status_pass "$h"; done

echo
if [ "$fail" -eq 0 ]; then
  echo "PASS — all onboarding assertions held ($OUT)"
else
  echo "FAIL — see failures above; forensics in $OUT"
fi
exit "$fail"
