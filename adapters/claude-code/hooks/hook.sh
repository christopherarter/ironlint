#!/usr/bin/env bash
set -euo pipefail

# Claude Code adapter for ironlint.
#
# Gates Edit/Write edits via the PreToolUse hook. Claude Code hands us the
# proposed content *before* the write lands, so this script builds it
# in-process (tool_input.content for Write; old_string -> new_string applied
# to the current on-disk file for Edit) and pipes it to
# `ironlint check --file <path> --content -`. Exit codes map onto Claude
# Code's PreToolUse block/allow semantics:
#   - 0 = allow (exit 0 lets the edit proceed),
#   - 2 = deny (verdict JSON on stderr; Claude Code surfaces stderr to the
#         model as the reason the tool call was blocked),
#   - see run_ironlint below for the full exit-code mapping.
#
# Event JSON arrives on stdin:
#   { "tool_name": "Write" | "Edit", "tool_input": {
#       "file_path": "...",
#       "content": "...",                                    # Write
#       "old_string": "...", "new_string": "...",
#       "replace_all": bool                                   # Edit
#   } }
#
# A first positional argument (the hook event name) is accepted but ignored.

# Default project root is the CWD where Claude Code is running.
PROJECT_ROOT="$(pwd)"
CONFIG="${PROJECT_ROOT}/.ironlint.yml"

# Skip silently if ironlint isn't configured for this project.
if [[ ! -f "${CONFIG}" ]]; then
  exit 0
fi

# Per-invocation temp file for the verdict; cleaned up on exit so concurrent
# Claude Code sessions don't clobber each other.
TMP_VERDICT=""
cleanup() {
  if [[ -n "${TMP_VERDICT}" && -f "${TMP_VERDICT}" ]]; then
    rm -f "${TMP_VERDICT}"
  fi
}
trap cleanup EXIT

# Parse the event JSON for the tool and the changed file.
EVENT=$(cat)
TOOL_NAME=$(echo "${EVENT}" | jq -r '.tool_name // empty')
FILE=$(echo "${EVENT}" | jq -r '.tool_input.file_path // .tool_input.path // empty')
if [[ -z "${FILE}" ]]; then
  # No file in event payload — nothing to check.
  exit 0
fi

# R3: short-circuit on edits to the policy file itself. The on-disk sha will
# not match `trust:` while the user is mid-edit; any `ironlint` invocation
# would fail the trust gate and surface a misleading "internal error" to the
# user. Match by basename so the skip works for both relative and absolute
# paths Claude Code may send.
BASENAME="${FILE##*/}"
if [[ "${BASENAME}" == ".ironlint.yml" || "${BASENAME}" == ".bully.yml" ]]; then
  exit 0
fi

# Run `ironlint check --file <FILE> --content -` with the proposed content
# piped on stdin. Maps the ironlint exit code onto Claude Code's PreToolUse
# semantics:
#   0 → allow (edit proceeds)
#   2 → deny (verdict JSON on stderr; Claude Code shows stderr to the model
#       as the denial reason)
#   3 → engine internal error: fail-open by default so a broken gate doesn't
#       brick the agent; set IRONLINT_FAIL_CLOSED_ON_INTERNAL=1 to block.
#   1 → config/trust error: log loudly but allow for now (Task 3.2 upgrades
#       exit 1 handling and adds exit 4).
#   anything else → log to stderr and allow (fail-open on internal errors so
#       a misconfigured ironlint install doesn't brick the agent).
run_ironlint() {
  local file=$1
  TMP_VERDICT=$(mktemp -t ironlint-verdict.XXXXXX)
  local ec=0
  ironlint check \
    --file "${file}" \
    --content - \
    --config "${CONFIG}" \
    --format json \
    > "${TMP_VERDICT}" 2>/dev/null || ec=$?
  case "${ec}" in
    0) exit 0 ;;
    2)
      cat "${TMP_VERDICT}" >&2
      exit 2
      ;;
    3)
      [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
      if [[ "${IRONLINT_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
        echo "ironlint: check errored (exit 3) — blocking (fail-closed)" >&2
        exit 2
      fi
      echo "ironlint: check errored (exit 3) — allowing (fail-open default)" >&2
      exit 0
      ;;
    1)
      echo "ironlint: config/trust error (exit 1) — see 'ironlint doctor'" >&2
      exit 0    # (Task 3.2 upgrades exit 1/4 handling; leave allow for now but LOUD)
      ;;
    *)
      echo "ironlint: unexpected ironlint exit ${ec} for ${file}" >&2
      exit 0
      ;;
  esac
}

case "${TOOL_NAME}" in
  Write)
    # tool_input.content IS the post-edit content — no synthesis needed.
    # jq -j (raw, no trailing newline) preserves the exact bytes from the
    # JSON string: jq -r would append an extra \n after the value.
    echo "${EVENT}" | jq -j '.tool_input.content // ""' | run_ironlint "${FILE}"
    ;;
  Edit)
    # Synthesize post-edit content by applying old_string -> new_string
    # in-process. Mirrors Claude Code's own uniqueness rule: unless
    # replace_all is set, old_string must appear exactly once in the file,
    # else Claude Code itself refuses the edit — fail closed (exit 2) here
    # so the content we gate matches what Claude Code would actually write.
    # Env vars (not shell interpolation) keep arbitrary substitution
    # payloads safe.
    OLD=$(echo "${EVENT}" | jq -r '.tool_input.old_string // empty')
    NEW=$(echo "${EVENT}" | jq -r '.tool_input.new_string // ""')
    REPLACE_ALL=$(echo "${EVENT}" | jq -r '(.tool_input.replace_all // false) | tostring')
    if [[ -z "${OLD}" ]]; then
      exit 0
    fi
    PROPOSED=$(
      IRONLINT_FILE="${FILE}" \
      IRONLINT_OLD="${OLD}" \
      IRONLINT_NEW="${NEW}" \
      IRONLINT_REPLACE_ALL="${REPLACE_ALL}" \
      python3 -c '
import os, sys
path = os.environ["IRONLINT_FILE"]
old = os.environ["IRONLINT_OLD"]
new = os.environ.get("IRONLINT_NEW", "")
replace_all = os.environ.get("IRONLINT_REPLACE_ALL", "false") == "true"
try:
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
except OSError as e:
    print(f"ironlint: cannot read {path}: {e}", file=sys.stderr)
    sys.exit(2)
count = content.count(old)
if replace_all:
    if count == 0:
        print(
            f"ironlint: refusing edit — old_string not found in {path}",
            file=sys.stderr,
        )
        sys.exit(2)
    sys.stdout.write(content.replace(old, new))
else:
    if count != 1:
        print(
            f"ironlint: refusing edit — old_string appears {count} times in "
            f"{path}; Claude Code requires exactly one match unless "
            "replace_all is set",
            file=sys.stderr,
        )
        sys.exit(2)
    sys.stdout.write(content.replace(old, new, 1))
' && printf 'X'
    ) || exit 2
    # $(...) strips trailing newlines; the sentinel 'X' (appended only on
    # python success via &&) preserves them. Strip the sentinel to recover
    # byte-exact content including any trailing newline.
    PROPOSED=${PROPOSED%X}
    # printf '%s' avoids the extra \n that `echo` would append after the value.
    printf '%s' "${PROPOSED}" | run_ironlint "${FILE}"
    ;;
  *)
    # Any other tool_name: nothing to gate.
    exit 0
    ;;
esac
