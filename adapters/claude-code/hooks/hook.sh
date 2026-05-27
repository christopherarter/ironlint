#!/usr/bin/env bash
set -euo pipefail

# Claude Code adapter for hector.
#
# Routes lifecycle events to `hector` invocations:
#   - post-tool-use: run `hector session record` to accumulate the edit,
#                    then `hector check --file <changed-file>` to gate the edit.
#                    Exit 2 on block, 3 on engine internal error (fail-open
#                    by default; HECTOR_FAIL_CLOSED_ON_INTERNAL=1 to block).
#   - stop:          run `hector check --session` to evaluate session rules.
#   - session-start: clear stale `.hector/session.json` from a previous run.
#
# Event JSON arrives on stdin. We pipe through jq to extract paths.

MODE="${1:-post-tool-use}"

# Default project root is the CWD where Claude Code is running.
PROJECT_ROOT="$(pwd)"
CONFIG="${PROJECT_ROOT}/.hector.yml"
HOOK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SYNTHESIZE_DIFF="${HOOK_DIR}/synthesize_diff.sh"

# Per-invocation temp file for verdict JSON; cleaned up on exit so concurrent
# Claude Code sessions don't clobber each other.
TMP_VERDICT=""
cleanup() {
  if [[ -n "${TMP_VERDICT}" && -f "${TMP_VERDICT}" ]]; then
    rm -f "${TMP_VERDICT}"
  fi
}
trap cleanup EXIT

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
    # Detect provider to decide whether to pass --emit-semantic-payload.
    STOP_PROVIDER=$(hector show-resolved-config --config "${CONFIG}" --format json 2>/dev/null \
      | jq -r '.llm.provider // empty' 2>/dev/null || true)
    TMP_VERDICT=$(mktemp -t hector-session-verdict.XXXXXX)
    EC=0
    if [[ "${STOP_PROVIDER}" == "claude-code-subagent" ]]; then
      # B3: subagent mode — emit a deferred envelope instead of requiring
      # an LlmClient. The evaluator subagent will receive the session
      # aggregate in additionalContext.
      hector check --session --config "${CONFIG}" --format json \
        --emit-semantic-payload > "${TMP_VERDICT}" 2>/dev/null || EC=$?
      case "${EC}" in
        0)
          # Either a DeferredVerdict (deferred session envelope on stdout)
          # or a clean direct-LLM pass.
          if jq -e '.deferred == true' < "${TMP_VERDICT}" >/dev/null 2>&1; then
            jq -n --slurpfile p "${TMP_VERDICT}" '{
              hookSpecificOutput: {
                hookEventName: "Stop",
                additionalContext: ("AGENTIC LINT SESSION EVALUATION REQUIRED:\n\n" + ($p[0].payload | tojson))
              }
            }'
          fi
          exit 0
          ;;
        2)
          cat "${TMP_VERDICT}" >&2
          exit 2
          ;;
        3)
          # B7: engine internal error during session check.
          if [[ "${HECTOR_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
            echo "hector: internal error — failing closed (HECTOR_FAIL_CLOSED_ON_INTERNAL=1)" >&2
            [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
            exit 2
          fi
          echo "hector: internal error during session check — allowing; see .hector/log.jsonl" >&2
          exit 0
          ;;
        *)
          echo "hector: internal error during session check (exit ${EC})" >&2
          [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
          exit 1
          ;;
      esac
    else
      # Direct-API mode: dispatch to LLM directly.
      hector check --session --config "${CONFIG}" --format json > "${TMP_VERDICT}" 2>/dev/null || EC=$?
      case "${EC}" in
        0)
          # session.json was cleared by hector check --session as a side effect.
          exit 0
          ;;
        2)
          cat "${TMP_VERDICT}" >&2
          exit 2
          ;;
        3)
          # B7: engine internal error during session check.
          if [[ "${HECTOR_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
            echo "hector: internal error — failing closed (HECTOR_FAIL_CLOSED_ON_INTERNAL=1)" >&2
            [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
            exit 2
          fi
          echo "hector: internal error during session check — allowing; see .hector/log.jsonl" >&2
          exit 0
          ;;
        *)
          echo "hector: internal error during session check (exit ${EC})" >&2
          [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
          exit 1
          ;;
      esac
    fi
    ;;

  post-tool-use|*)
    # Parse the event JSON for the changed file.
    EVENT=$(cat)
    FILE=$(echo "${EVENT}" | jq -r '.tool_input.file_path // .tool_input.path // empty')
    if [[ -z "${FILE}" ]]; then
      # No file in event payload — nothing to check.
      exit 0
    fi

    # R3: short-circuit on edits to the policy file itself. The on-disk
    # sha will not match `trust:` while the user is mid-edit; any `hector`
    # invocation would fail the trust gate and surface a misleading
    # "internal error" to the user. Match by basename so the skip works
    # for both relative and absolute paths Claude Code may send.
    BASENAME="${FILE##*/}"
    if [[ "${BASENAME}" == ".hector.yml" || "${BASENAME}" == ".bully.yml" ]]; then
      exit 0
    fi

    # Build a synthetic unified diff for session recording. Claude Code's
    # Edit/Write events don't carry a real diff, so we fake one from the
    # (old_string, new_string) pair. The synthesizer (P1-8/P1-9 fix):
    #   - Emits correct `@@ -1,N +1,M @@` line counts for multi-line edits.
    #   - Escapes any line in OLD/NEW that looks like a diff header, so
    #     attacker-controlled content can't reframe the diff onto another
    #     file.
    OLD=$(echo "${EVENT}" | jq -r '.tool_input.old_string // ""')
    NEW=$(echo "${EVENT}" | jq -r '.tool_input.new_string // .tool_input.content // ""')
    DIFF=$("${SYNTHESIZE_DIFF}" "${FILE}" "${OLD}" "${NEW}")

    # 1. Record the edit into session state (non-blocking).
    hector session record --dir "${PROJECT_ROOT}" --file "${FILE}" --diff "${DIFF}" >/dev/null 2>&1 || true

    # 2. Detect mode. `hector show-resolved-config --format json` is cheap
    #    (no LLM, no engine dispatch). `.llm.provider // empty` falls through
    #    to the direct-API branch when no `llm:` block is configured.
    PROVIDER=$(hector show-resolved-config --config "${CONFIG}" --format json 2>/dev/null \
      | jq -r '.llm.provider // empty' 2>/dev/null || true)

    # 3. Gate the edit by running checks. Differentiate hector exit codes:
    #    0 = pass/warn (or deferred payload under subagent mode),
    #    2 = block (rule violation),
    #    3 = engine internal error (missing API key, spawn failure, etc.),
    #    1 = config/load error.
    TMP_VERDICT=$(mktemp -t hector-verdict.XXXXXX)
    EC=0
    # Both branches suppress hector's own stderr so the verdict JSON we
    # later cat to stderr on block (exit 2) parses cleanly. The macOS
    # capability sandbox emits a per-process advisory warning that would
    # otherwise prepend to the verdict stream. Real internal errors still
    # surface via the explicit `echo` on the `*)` arm + the verdict
    # contents if hector wrote anything before erroring.
    if [[ "${PROVIDER}" == "claude-code-subagent" ]]; then
      # Subagent mode: ask core to emit a deferred-semantic payload instead
      # of dispatching to an LLM.
      hector check --file "${FILE}" --config "${CONFIG}" --format json \
        --emit-semantic-payload > "${TMP_VERDICT}" 2>/dev/null || EC=$?
      case "${EC}" in
        0)
          # Either a DeferredVerdict (envelope on stdout) or a clean standard
          # verdict (no envelope, no stdout).
          if jq -e '.deferred == true' < "${TMP_VERDICT}" >/dev/null 2>&1; then
            jq -n --slurpfile p "${TMP_VERDICT}" '{
              hookSpecificOutput: {
                hookEventName: "PostToolUse",
                additionalContext: ("AGENTIC LINT SEMANTIC EVALUATION REQUIRED:\n\n" + ($p[0].payload | tojson))
              }
            }'
          fi
          exit 0
          ;;
        2)
          cat "${TMP_VERDICT}" >&2
          exit 2
          ;;
        3)
          # B7: engine internal error (missing API key, spawn failure, etc.).
          # Fail-open by default so a broken gate doesn't block the agent.
          # Opt-in fail-closed: HECTOR_FAIL_CLOSED_ON_INTERNAL=1.
          if [[ "${HECTOR_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
            echo "hector: internal error — failing closed (HECTOR_FAIL_CLOSED_ON_INTERNAL=1)" >&2
            [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
            exit 2
          fi
          echo "hector: internal error during check — allowing edit; see .hector/log.jsonl" >&2
          exit 0
          ;;
        *)
          echo "hector: internal error checking ${FILE} (exit ${EC})" >&2
          [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
          exit 1
          ;;
      esac
    else
      # Direct-API mode (anthropic / openrouter / ollama / no llm at all).
      hector check --file "${FILE}" --config "${CONFIG}" --format json > "${TMP_VERDICT}" 2>/dev/null || EC=$?
      case "${EC}" in
        0) exit 0 ;;
        2)
          cat "${TMP_VERDICT}" >&2
          exit 2
          ;;
        3)
          # B7: engine internal error (missing API key, spawn failure, etc.).
          # Fail-open by default so a broken gate doesn't block the agent.
          # Opt-in fail-closed: HECTOR_FAIL_CLOSED_ON_INTERNAL=1.
          if [[ "${HECTOR_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
            echo "hector: internal error — failing closed (HECTOR_FAIL_CLOSED_ON_INTERNAL=1)" >&2
            [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
            exit 2
          fi
          echo "hector: internal error during check — allowing edit; see .hector/log.jsonl" >&2
          exit 0
          ;;
        *)
          echo "hector: internal error checking ${FILE} (exit ${EC})" >&2
          [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
          exit 1
          ;;
      esac
    fi
    ;;
esac
