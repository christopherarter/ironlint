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

# Per-invocation temp files (verdict, synthesized Edit content); cleaned up
# on exit so concurrent Claude Code sessions don't clobber each other.
TMP_VERDICT=""
TMP_PROPOSED=""
cleanup() {
  if [[ -n "${TMP_VERDICT}" && -f "${TMP_VERDICT}" ]]; then
    rm -f "${TMP_VERDICT}"
  fi
  if [[ -n "${TMP_PROPOSED}" && -f "${TMP_PROPOSED}" ]]; then
    rm -f "${TMP_PROPOSED}"
  fi
}
trap cleanup EXIT

# Parse the event JSON for the tool and the changed file.
EVENT=$(cat)

# Guard: an unparseable payload must never crash the hook. Without this,
# `jq`'s parse failure below propagates through `set -e`/`pipefail` and kills
# the script with jq's own exit status (5) plus a raw "jq: parse error: ..."
# dump on stderr — an undocumented exit code Claude Code's PreToolUse runner
# has no defined handling for. Skip gracefully instead: a malformed event
# must not brick the agent.
if ! echo "${EVENT}" | jq empty >/dev/null 2>&1; then
  echo "ironlint: malformed event JSON on stdin — skipping (allow)" >&2
  exit 0
fi

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
#   4 → untrusted config/gates: fail CLOSED (deny) — an untrusted config must
#       never be silently allowed through. Claude Code's PreToolUse block is
#       hook exit 2 (there is no separate "deny" exit code), so this arm
#       exits 2 just like a real policy block, with the trust message on
#       stderr as the reason.
#   1 → config/load error (parse failure, missing file, ...): log loudly but
#       allow — this is a genuine config problem, not a trust decision, and
#       is not the gap Task 3.2 closes.
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
    4)
      echo "ironlint is configured here but not trusted — run 'ironlint trust' to enable checks" >&2
      exit 2    # fail CLOSED: an untrusted config must never be silently allowed
      ;;
    1)
      echo "ironlint: config/load error (exit 1) — see 'ironlint doctor'" >&2
      exit 0    # a genuine config/parse problem, not a trust decision — allow but loud
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
    #
    # old_string/new_string/replace_all are extracted INSIDE python via
    # json.loads on the whole event, never round-tripped through a shell
    # `$(...)` command substitution — `$(...)` strips ALL trailing newlines
    # from the captured value, which would desync the bytes ironlint gates
    # from the bytes Claude Code actually writes to disk (e.g. a
    # new_string ending in "\n" would silently lose it). The event JSON
    # travels only as an env var (never shell-interpolated), so arbitrary
    # substitution payloads stay safe from injection. Python's stdout is
    # redirected straight to a temp file (not captured via `$(...)` either)
    # so the synthesized content's own trailing newline, if any, survives
    # byte-for-byte.
    #
    # python exit codes (local to this branch only):
    #   0 → synthesized content written to stdout
    #   2 → block: old_string isn't unique, or the on-disk file couldn't be
    #       read — mirrors Claude Code's own refusal, so we gate what
    #       Claude Code would actually do
    #   3 → no old_string in the payload — nothing to synthesize, allow
    TMP_PROPOSED=$(mktemp -t ironlint-edit.XXXXXX)
    PY_EC=0
    IRONLINT_EVENT_JSON="${EVENT}" IRONLINT_FILE="${FILE}" python3 -c '
import json, os, sys

ev = json.loads(os.environ["IRONLINT_EVENT_JSON"])
ti = ev.get("tool_input") or {}
old = ti.get("old_string")
if not old:
    sys.exit(3)
new = ti.get("new_string", "")
replace_all = bool(ti.get("replace_all", False))
path = os.environ["IRONLINT_FILE"]

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
' > "${TMP_PROPOSED}" || PY_EC=$?
    case "${PY_EC}" in
      0) : ;;
      3) exit 0 ;;
      *) exit 2 ;;
    esac
    # Redirect stdin from the temp file — byte-exact, including any
    # trailing newline — rather than piping python straight into
    # run_ironlint. This also means run_ironlint's `exit` runs in THIS
    # shell rather than a pipeline subshell.
    run_ironlint "${FILE}" < "${TMP_PROPOSED}"
    ;;
  *)
    # Any other tool_name: nothing to gate.
    exit 0
    ;;
esac
