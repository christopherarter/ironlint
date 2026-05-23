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
  # R2 (2026-05-23): model: omitted intentionally — the subagent
  # provider does not read it. Pre-R2 this was required-but-ignored.
  provider: claude-code-subagent
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
  no-todo-comment:
    description: "no TODO comments left in committed content"
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
# R6 (2026-05-23): the deferred semantic rules (`prose-quality`,
# `no-todo-comment`) must surface on the verdict via `deferred_rules`
# even though the deterministic block fired. Pre-R6, they vanished and
# the user had no way to know their semantic rules were even alive.
DEFERRED_IDS=$(jq -r '.deferred_rules // [] | sort_by(.rule_id) | map(.rule_id) | join(",")' < "${ERR}")
if [[ "${DEFERRED_IDS}" != "no-todo-comment,prose-quality" ]]; then
  echo "FAIL test 2: expected deferred_rules to include both semantic rule ids; got ${DEFERRED_IDS}"
  cat "${ERR}"
  exit 1
fi
if ! jq -e '.deferred_rules[0].reason | length > 0' < "${ERR}" >/dev/null; then
  echo "FAIL test 2: deferred_rules entries must carry a non-empty reason"
  cat "${ERR}"
  exit 1
fi
echo "PASS test 2: deterministic block under subagent mode exits 2 with verdict on stderr (carries deferred_rules)"
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

# ----------------------------------------------------------------------
# Test 5 (R3): editing the policy file itself (.hector.yml) must short-circuit.
# The on-disk sha will not match the `trust:` field while the file is
# mid-edit; running `hector check` against it would fail the trust gate
# and surface a misleading "internal error" to the user. Hook should
# exit 0 with empty stdout/stderr and never invoke `hector`.
# ----------------------------------------------------------------------
# Deliberately break the trust hash to mimic the mid-edit state: any
# `hector` invocation against this config will fail with exit 1 and a
# scary "internal error" message. The basename short-circuit must run
# before we ever reach that codepath.
echo "# user is iterating on policy" >> "${PROJECT}/.hector.yml"
# Replace the trust block with a wrong sha so trust verify fails hard.
sed -i.bak 's/sha256:.*/sha256:0000000000000000000000000000000000000000000000000000000000000000/' "${PROJECT}/.hector.yml"
rm -f "${PROJECT}/.hector.yml.bak"
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/.hector.yml", "new_string": "anything"}}'
OUT=$(mktemp); ERR=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2> "${ERR}" || EC=$?
if [[ "${EC}" -ne 0 ]]; then
  echo "FAIL test 5: expected exit 0 on .hector.yml self-edit, got ${EC}"
  cat "${ERR}"
  exit 1
fi
if [[ -s "${OUT}" ]]; then
  echo "FAIL test 5: self-edit of .hector.yml must not emit on stdout"
  cat "${OUT}"
  exit 1
fi
if [[ -s "${ERR}" ]]; then
  echo "FAIL test 5: self-edit of .hector.yml must not emit on stderr"
  cat "${ERR}"
  exit 1
fi
echo "PASS test 5: hook skips self-check of .hector.yml (absolute path)"
rm -f "${OUT}" "${ERR}"

# ----------------------------------------------------------------------
# Test 6 (R3): relative path to .hector.yml — basename match also covers
# events where Claude Code sends the bare filename.
# ----------------------------------------------------------------------
EVENT='{"tool_input": {"file_path": ".hector.yml", "new_string": "anything"}}'
OUT=$(mktemp); ERR=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2> "${ERR}" || EC=$?
if [[ "${EC}" -ne 0 ]]; then
  echo "FAIL test 6: expected exit 0 on relative .hector.yml self-edit, got ${EC}"
  cat "${ERR}"
  exit 1
fi
if [[ -s "${OUT}" || -s "${ERR}" ]]; then
  echo "FAIL test 6: relative .hector.yml self-edit must be silent"
  cat "${OUT}" "${ERR}"
  exit 1
fi
echo "PASS test 6: hook skips self-check of .hector.yml (relative path)"
rm -f "${OUT}" "${ERR}"

# ----------------------------------------------------------------------
# Test 7 (R3): .bully.yml (migration source) also short-circuits.
# ----------------------------------------------------------------------
EVENT='{"tool_input": {"file_path": "'"${PROJECT}"'/.bully.yml", "new_string": "anything"}}'
OUT=$(mktemp); ERR=$(mktemp)
EC=0
echo "${EVENT}" | "${HOOK}" post-tool-use > "${OUT}" 2> "${ERR}" || EC=$?
if [[ "${EC}" -ne 0 ]]; then
  echo "FAIL test 7: expected exit 0 on .bully.yml self-edit, got ${EC}"
  cat "${ERR}"
  exit 1
fi
if [[ -s "${OUT}" || -s "${ERR}" ]]; then
  echo "FAIL test 7: .bully.yml self-edit must be silent"
  cat "${OUT}" "${ERR}"
  exit 1
fi
echo "PASS test 7: hook skips self-check of .bully.yml"
rm -f "${OUT}" "${ERR}"

echo ""
echo "All subagent-mode hook tests passed."
