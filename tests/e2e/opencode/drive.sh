#!/usr/bin/env bash
# Drive script for the opencode adapter e2e harness.
# Mirrors tests/e2e/claude-code/drive.sh; differences are noted inline.

set -uo pipefail

DRIVE_LOG="/work/runs/drive.log"
HARNESS_LOG="/work/runs/harness.log"
mkdir -p /work/runs/.hector

log()  { printf "[%s] %s\n" "$(date -u +%H:%M:%S)" "$*" | tee -a "$DRIVE_LOG"; }
fail() { log "LIFECYCLE FAIL: $*"; exit 1; }

CASE=""
for arg in "$@"; do
  case "$arg" in
    --case=*) CASE="${arg#--case=}" ;;
    *) fail "unknown arg: $arg" ;;
  esac
done
[[ -n "$CASE" ]] || fail "missing --case=<name>"

CASE_FILE="/work/cases/$CASE.json"
[[ -f "$CASE_FILE" ]] || fail "case file not found: $CASE_FILE"

log "phase 0: setup; case=$CASE"
[[ -n "${ANTHROPIC_API_KEY:-}" ]] || fail "ANTHROPIC_API_KEY not in environment"
[[ -x /usr/local/bin/hector  ]] || fail "/usr/local/bin/hector not executable"

PROMPT="$(jq -r '.prompt' "$CASE_FILE")"
TARGET_FILE="$(jq -r '.target_file' "$CASE_FILE")"

log "phase 1: install check"
hector --version  | tee -a "$DRIVE_LOG" || fail "hector --version"
opencode --version | tee -a "$DRIVE_LOG" || fail "opencode --version"
[[ -d /home/hector/opencode-plugin ]] || fail "plugin source missing"

log "phase 2: onboarding"
WORKDIR=/work/runs/workdir
mkdir -p "$WORKDIR" && cd "$WORKDIR" || fail "cd workdir"
git init -q
cp -r /work/fixture/. "$WORKDIR/"
git add -A && git -c user.email=e2e@hector -c user.name=e2e commit -q -m "fixture"

# Wire the plugin into the project (per opencode adapter README).
mkdir -p .opencode/plugins
cp /home/hector/opencode-plugin/src/index.ts .opencode/plugins/hector.ts
# If the plugin needs its node_modules, run install. Skip if package.json
# absent at the plugin path.
if [[ -f /home/hector/opencode-plugin/package.json ]]; then
  (cd /home/hector/opencode-plugin && bun install --frozen-lockfile) \
    >>"$DRIVE_LOG" 2>&1 || log "warn: bun install non-fatal"
fi

hector init >"$DRIVE_LOG.init.out" 2>&1 || fail "hector init"
cp .hector.yml /work/runs/.hector.yml.from-init 2>/dev/null || true
cp /work/policy/.hector.yml ./.hector.yml
hector trust    | tee -a "$DRIVE_LOG" || fail "hector trust"
hector validate | tee -a "$DRIVE_LOG" || fail "hector validate"

log "phase 3: drive harness with opencode run"
# Exact flag verified at impl time via `opencode run --help`.
# The model id format follows OpenCode's provider/model convention.
timeout 120 opencode run --model anthropic/claude-haiku-4-5 "$PROMPT" \
  >>"$HARNESS_LOG" 2>&1
HARNESS_EXIT=$?
log "harness exit: $HARNESS_EXIT"

log "phase 4: capture forensics"
if [[ -f "$WORKDIR/.hector/log.jsonl" ]]; then
  cp "$WORKDIR/.hector/log.jsonl" /work/runs/.hector/log.jsonl
fi
if [[ -f /work/runs/.hector/log.jsonl ]]; then
  tail -n 50 /work/runs/.hector/log.jsonl \
    | jq -s 'last' >/work/runs/verdict.json 2>/dev/null || true
fi

log "phase 5: lifecycle complete"
exit 0
