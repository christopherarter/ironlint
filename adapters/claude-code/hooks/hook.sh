#!/usr/bin/env bash
set -euo pipefail

# Claude Code adapter for hector.
#
# Routes lifecycle events to `hector` invocations:
#   - post-tool-use: run `hector session record` to accumulate the edit,
#                    then `hector check --file <changed-file>` to gate the edit.
#                    Exit 2 on block, 0 otherwise.
#   - stop:          run `hector check --session` to evaluate session rules.
#   - session-start: clear stale `.hector/session.json` from a previous run.
#
# Event JSON arrives on stdin. We pipe through jq to extract paths.

MODE="${1:-post-tool-use}"

# Default project root is the CWD where Claude Code is running.
PROJECT_ROOT="$(pwd)"
CONFIG="${PROJECT_ROOT}/.hector.yml"

# Skip silently if hector isn't configured for this project.
if [[ ! -f "${CONFIG}" ]]; then
  exit 0
fi

case "${MODE}" in
  session-start)
    # Clear any stale session.json from a prior aborted session.
    rm -f "${PROJECT_ROOT}/.hector/session.json" 2>/dev/null || true
    exit 0
    ;;

  stop)
    # Evaluate session rules over the accumulated changeset.
    # If session.json doesn't exist (no edits this session), exit 0 without running.
    if [[ ! -f "${PROJECT_ROOT}/.hector/session.json" ]]; then
      exit 0
    fi
    if ! hector check --session --config "${CONFIG}" --format json > /tmp/hector-session-verdict.json; then
      cat /tmp/hector-session-verdict.json >&2
      exit 2
    fi
    # session.json was cleared by hector check --session as a side effect.
    exit 0
    ;;

  post-tool-use|*)
    # Parse the event JSON for the changed file.
    EVENT=$(cat)
    FILE=$(echo "${EVENT}" | jq -r '.tool_input.file_path // .tool_input.path // empty')
    if [[ -z "${FILE}" ]]; then
      # No file in event payload — nothing to check.
      exit 0
    fi

    # Build a synthetic unified diff for session recording.
    # (Claude Code's Edit/Write events don't include a diff; we fake one
    #  from the new_string + old_string pair when available.)
    OLD=$(echo "${EVENT}" | jq -r '.tool_input.old_string // ""')
    NEW=$(echo "${EVENT}" | jq -r '.tool_input.new_string // .tool_input.content // ""')
    DIFF="--- a/${FILE}
+++ b/${FILE}
@@ -1 +1 @@
-${OLD}
+${NEW}"

    # 1. Record the edit into session state (non-blocking).
    hector session record --dir "${PROJECT_ROOT}" --file "${FILE}" --diff "${DIFF}" >/dev/null 2>&1 || true

    # 2. Gate the edit by running checks.
    if ! hector check --file "${FILE}" --config "${CONFIG}" --format json > /tmp/hector-verdict.json; then
      cat /tmp/hector-verdict.json >&2
      exit 2
    fi
    exit 0
    ;;
esac
