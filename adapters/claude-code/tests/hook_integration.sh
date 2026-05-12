#!/usr/bin/env bash
set -euo pipefail

# End-to-end test of the Claude Code adapter hook.
# Simulates Claude Code calling hook.sh with synthetic event payloads.
# Requires: hector binary on PATH, jq on PATH, bash.

ADAPTER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOK="${ADAPTER_DIR}/hooks/hook.sh"

# Create a temp project with .hector.yml.
PROJECT=$(mktemp -d)
trap 'rm -rf "${PROJECT}"' EXIT

cat > "${PROJECT}/.hector.yml" <<'EOF'
schema_version: 2
rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["*.txt"]
    severity: error
    script: "grep -nE 'DEBUG' {file} && exit 1 || exit 0"
EOF
hector trust --config "${PROJECT}/.hector.yml"

# Test 1: PostToolUse on a clean file → exit 0.
cd "${PROJECT}"
echo "clean content" > clean.txt
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/clean.txt", "new_string": "clean content"}}'
if echo "${EVENT}" | "${HOOK}" post-tool-use; then
  echo "PASS: clean file allowed"
else
  echo "FAIL: clean file blocked"
  exit 1
fi

# Test 2: PostToolUse on a dirty file → exit 2.
echo "this has DEBUG in it" > dirty.txt
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/dirty.txt", "new_string": "this has DEBUG in it"}}'
if echo "${EVENT}" | "${HOOK}" post-tool-use; then
  echo "FAIL: dirty file should have been blocked"
  exit 1
else
  EC=$?
  if [[ "${EC}" == "2" ]]; then
    echo "PASS: dirty file blocked with exit 2"
  else
    echo "FAIL: expected exit 2, got ${EC}"
    exit 1
  fi
fi

# Test 3: SessionStart clears stale session.json.
mkdir -p "${PROJECT}/.hector"
echo '{"session_id":"stale","started_at":"t","edits":[]}' > "${PROJECT}/.hector/session.json"
echo '{}' | "${HOOK}" session-start
if [[ -f "${PROJECT}/.hector/session.json" ]]; then
  echo "FAIL: session-start should have cleared session.json"
  exit 1
fi
echo "PASS: session-start clears stale state"

# Test 4: Stop with no session.json → exit 0 (no-op).
echo '{}' | "${HOOK}" stop
echo "PASS: stop with no session.json is a no-op"

echo ""
echo "All hook integration tests passed."
