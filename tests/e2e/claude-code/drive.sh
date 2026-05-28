#!/usr/bin/env bash
# Drive script for the claude-code adapter e2e harness.
# Args:  --case=<name>      Required. Loads /work/cases/<name>.json.
#
# Layout:
#   /work/policy/.hector.yml   :ro  canonical policy
#   /work/fixture/             :ro  Node starter project
#   /work/cases/<name>.json    :ro  prompt + target + expected rule
#   /work/runs/                :rw  forensics + workdir for this case
#   /usr/local/bin/hector      :ro  release-build hector binary
#
# Exit codes:
#   0  lifecycle completed (test asserts on RunResult, not this exit code)
#   1  lifecycle broke (preflight failed, validate failed, etc.)

set -uo pipefail

DRIVE_LOG="/work/runs/drive.log"
HARNESS_LOG="/work/runs/harness.log"
mkdir -p /work/runs/.hector

log()  { printf "[%s] %s\n" "$(date -u +%H:%M:%S)" "$*" | tee -a "$DRIVE_LOG"; }
fail() { log "LIFECYCLE FAIL: $*"; exit 1; }

# --- Parse args ---
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

# --- Phase 0: Setup ---
log "phase 0: setup; case=$CASE"
[[ -n "${ANTHROPIC_API_KEY:-}" ]] || fail "ANTHROPIC_API_KEY not in environment"
[[ -x /usr/local/bin/hector  ]] || fail "/usr/local/bin/hector not executable"

PROMPT="$(jq -r '.prompt' "$CASE_FILE")"
TARGET_FILE="$(jq -r '.target_file' "$CASE_FILE")"
EXPECTED_RULE="$(jq -r '.expected_rule' "$CASE_FILE")"
[[ -n "$PROMPT" && -n "$TARGET_FILE" && -n "$EXPECTED_RULE" ]] \
  || fail "case JSON missing required fields"

# --- Phase 1: Install check ---
log "phase 1: install check"
hector --version | tee -a "$DRIVE_LOG"  || fail "hector --version"
claude --version | tee -a "$DRIVE_LOG"  || fail "claude --version"
[[ -d /home/hector/.claude/plugins/hector ]] \
  || fail "plugin not at /home/hector/.claude/plugins/hector"

# --- Phase 2: Onboarding ---
log "phase 2: onboarding"
WORKDIR=/work/runs/workdir
mkdir -p "$WORKDIR" && cd "$WORKDIR" || fail "cd workdir"
git init -q
cp -r /work/fixture/. "$WORKDIR/"
git add -A && git -c user.email=e2e@hector -c user.name=e2e commit -q -m "fixture"

# hector init writes a default config; capture it for forensics, then overlay
# the canonical test policy.
hector init >"$DRIVE_LOG.init.out" 2>&1 || fail "hector init"
cp .hector.yml /work/runs/.hector.yml.from-init 2>/dev/null || true
cp /work/policy/.hector.yml ./.hector.yml
hector trust    | tee -a "$DRIVE_LOG" || fail "hector trust"
hector validate | tee -a "$DRIVE_LOG" || fail "hector validate"

# --- Phase 3: Drive the harness ---
log "phase 3: drive harness with claude --print"
# `--print` makes the CLI non-interactive (prints the response and exits).
# `--model claude-haiku-4-5` keeps cost low and matches the policy's LLM.
# `timeout 120` defends against an unresponsive agent.
timeout 120 claude --print --model claude-haiku-4-5 "$PROMPT" \
  >>"$HARNESS_LOG" 2>&1
HARNESS_EXIT=$?
log "harness exit: $HARNESS_EXIT"

# --- Phase 4: Capture ---
log "phase 4: capture forensics"
if [[ -f "$WORKDIR/.hector/log.jsonl" ]]; then
  cp "$WORKDIR/.hector/log.jsonl" /work/runs/.hector/log.jsonl
fi
# Extract the latest verdict if any (one JSON object per line; take last block).
if [[ -f /work/runs/.hector/log.jsonl ]]; then
  tail -n 50 /work/runs/.hector/log.jsonl \
    | jq -s 'last' >/work/runs/verdict.json 2>/dev/null || true
fi

# --- Phase 5: Done ---
log "phase 5: lifecycle complete"
exit 0
