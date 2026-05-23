#!/usr/bin/env bash
set -euo pipefail

# End-to-end test for Claude Code adapter subagent mode (H3).
# Exercises the three branches of the subagent-mode path:
#   1. Deferred semantic payload → hookSpecificOutput.additionalContext envelope on stdout, exit 0
#   2. Deterministic block → verdict JSON on stderr, exit 2 (no envelope)
#   3. No semantic + no block → exit 0, nothing on stdout
# Also re-asserts that the direct-API path (provider: anthropic) is unchanged.

ADAPTER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOK="${ADAPTER_DIR}/hooks/hook.sh"

PROJECT=$(mktemp -d)
trap 'rm -rf "${PROJECT}"' EXIT

# ----------------------------------------------------------------------
# Fixture: subagent-mode config with one deterministic + one semantic rule.
# ----------------------------------------------------------------------
cat > "${PROJECT}/.hector.yml" <<'YAML'
schema_version: 2
llm:
  provider: claude-code-subagent
  model: subagent
rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["*.txt"]
    severity: error
    script: "grep -nE 'DEBUG' {file} && exit 1 || exit 0"
  prose-quality:
    description: "files should read clearly"
    engine: semantic
    scope: ["*.txt"]
    severity: warning
YAML
hector trust --config "${PROJECT}/.hector.yml" >/dev/null

cd "${PROJECT}"

# ----------------------------------------------------------------------
# Test 1: clean file with surviving semantic rule → envelope on stdout, exit 0.
# ----------------------------------------------------------------------
echo "clean content" > clean.txt
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/clean.txt", "new_string": "clean content"}}'
OUT=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2>/dev/null || EC=$?
if [[ "${EC}" -ne 0 ]]; then
  echo "FAIL test 1: expected exit 0, got ${EC}"
  exit 1
fi
if ! jq -e '.hookSpecificOutput.hookEventName == "PostToolUse"' < "${OUT}" >/dev/null; then
  echo "FAIL test 1: stdout is not a hookSpecificOutput envelope"
  cat "${OUT}"
  exit 1
fi
CTX=$(jq -r '.hookSpecificOutput.additionalContext' < "${OUT}")
if [[ "${CTX}" != "AGENTIC LINT SEMANTIC EVALUATION REQUIRED:"* ]]; then
  echo "FAIL test 1: additionalContext does not start with the AGENTIC LINT preamble"
  echo "got: ${CTX}"
  exit 1
fi
PAYLOAD_JSON="${CTX#AGENTIC LINT SEMANTIC EVALUATION REQUIRED:}"
if ! echo "${PAYLOAD_JSON}" | jq -e '.file and .diff and .evaluate and ._evaluator_input' >/dev/null; then
  echo "FAIL test 1: payload missing required fields"
  echo "${PAYLOAD_JSON}"
  exit 1
fi
echo "PASS test 1: subagent mode emits hookSpecificOutput envelope on deferred payload"
rm -f "${OUT}"

# ----------------------------------------------------------------------
# Test 2: deterministic block → verdict on stderr, exit 2 (no envelope).
# ----------------------------------------------------------------------
echo "this has DEBUG in it" > dirty.txt
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/dirty.txt", "new_string": "this has DEBUG"}}'
OUT=$(mktemp); ERR=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2> "${ERR}" || EC=$?
if [[ "${EC}" -ne 2 ]]; then
  echo "FAIL test 2: expected exit 2, got ${EC}"
  cat "${ERR}"
  exit 1
fi
if [[ -s "${OUT}" ]]; then
  echo "FAIL test 2: deterministic block must not emit on stdout"
  cat "${OUT}"
  exit 1
fi
if ! jq -e '.status == "block"' < "${ERR}" >/dev/null; then
  echo "FAIL test 2: stderr is not a verdict JSON with status=block"
  cat "${ERR}"
  exit 1
fi
echo "PASS test 2: deterministic block under subagent mode exits 2 with verdict on stderr"
rm -f "${OUT}" "${ERR}"

# ----------------------------------------------------------------------
# Test 3: nothing semantic, nothing blocked → exit 0, empty stdout.
# (Out-of-scope file: rule scope is *.txt, this is *.md.)
# ----------------------------------------------------------------------
echo "no rules apply" > other.md
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/other.md", "new_string": "no rules apply"}}'
OUT=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2>/dev/null || EC=$?
if [[ "${EC}" -ne 0 ]]; then
  echo "FAIL test 3: expected exit 0, got ${EC}"
  exit 1
fi
if [[ -s "${OUT}" ]]; then
  echo "FAIL test 3: no deferred payload means no stdout output"
  cat "${OUT}"
  exit 1
fi
echo "PASS test 3: no semantic + no block exits 0 silently"
rm -f "${OUT}"

# ----------------------------------------------------------------------
# Test 4: direct-API mode (provider: anthropic) is unchanged.
# Swap the config to anthropic + re-trust. Run a clean file. Expect exit 0
# with NO hookSpecificOutput envelope on stdout — the direct-API branch
# never emits an envelope.
# ----------------------------------------------------------------------
cat > "${PROJECT}/.hector.yml" <<'YAML'
schema_version: 2
llm:
  provider: anthropic
  model: claude-3-5-sonnet-20241022
rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["*.txt"]
    severity: error
    script: "exit 0"
YAML
hector trust --config "${PROJECT}/.hector.yml" >/dev/null
echo "clean content" > direct.txt
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/direct.txt", "new_string": "clean content"}}'
OUT=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2>/dev/null || EC=$?
if [[ "${EC}" -ne 0 ]]; then
  echo "FAIL test 4: direct-API mode expected exit 0, got ${EC}"
  exit 1
fi
if [[ -s "${OUT}" ]] && jq -e '.hookSpecificOutput' < "${OUT}" >/dev/null 2>&1; then
  echo "FAIL test 4: direct-API mode must not emit hookSpecificOutput envelope"
  cat "${OUT}"
  exit 1
fi
echo "PASS test 4: direct-API mode unchanged"
rm -f "${OUT}"

echo ""
echo "All subagent-mode hook tests passed."
