#!/usr/bin/env bash
set -euo pipefail

# Reasonix adapter for hector.
#
# Wires Reasonix's PreToolUse lifecycle event to `hector check --file
# <path> --content -` so file edits are *blocked* against the project's
# .hector.yml policy before they land on disk. Reasonix's PostToolUse is
# documented as non-gating (exit 2 = warning only); only PreToolUse can
# physically prevent a bad edit. See
# `specs/2026-05-25-reasonix-adapter.md` for the architectural rationale.
#
# Reasonix PreToolUse stdin payload (per docs/configuration.html#hooks):
#   {
#     "event": "PreToolUse",
#     "cwd": "/workspace",
#     "toolName": "write_file" | "edit_file" | "multi_edit" | ...,
#     "toolArgs": { "path": "...", ... },
#     "turn": N
#   }
#
# Per-tool toolArgs (from Reasonix `src/tools/filesystem.ts`):
#   write_file → { path, content }                        — content IS the post-edit text
#   edit_file  → { path, search, replace }                — apply unique substitution
#   multi_edit → { path, edits: [{ search, replace } ...] } — currently not gated; see below
#
# Direct-API mode only — Reasonix has no subscription/subagent split.

MODE="${1:-pre-tool-use}"

EVENT=$(cat)

PROJECT_ROOT=$(echo "${EVENT}" | jq -r '.cwd // empty')
if [[ -z "${PROJECT_ROOT}" ]]; then
  PROJECT_ROOT="$(pwd)"
fi
CONFIG="${PROJECT_ROOT}/.hector.yml"

# Skip silently if hector isn't configured for this project.
if [[ ! -f "${CONFIG}" ]]; then
  exit 0
fi

TMP_VERDICT=""
cleanup() {
  if [[ -n "${TMP_VERDICT}" && -f "${TMP_VERDICT}" ]]; then
    rm -f "${TMP_VERDICT}"
  fi
}
trap cleanup EXIT

# Run `hector check --file <FILE> --content -` with the proposed content
# piped on stdin. Maps the hector exit code onto Reasonix's PreToolUse
# semantics:
#   0 → pass through (edit proceeds)
#   2 → block (Reasonix refuses the tool call)
#   anything else → log to stderr and pass through (fail-open on internal
#                   errors so an agent isn't bricked by a misconfigured
#                   hector install).
run_hector() {
  local file=$1
  TMP_VERDICT=$(mktemp -t hector-verdict.XXXXXX)
  local ec=0
  hector check \
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
    *)
      echo "hector: internal error checking ${file} (exit ${ec})" >&2
      [[ -s "${TMP_VERDICT}" ]] && cat "${TMP_VERDICT}" >&2
      exit 0
      ;;
  esac
}

case "${MODE}" in
  pre-tool-use|*)
    TOOL=$(echo "${EVENT}" | jq -r '.toolName // empty')
    FILE=$(echo "${EVENT}" | jq -r '.toolArgs.path // .toolArgs.file_path // empty')
    if [[ -z "${FILE}" ]]; then
      exit 0
    fi
    if [[ "${FILE}" != /* ]]; then
      FILE="${PROJECT_ROOT}/${FILE}"
    fi

    # Short-circuit edits to the policy file itself — the on-disk sha will
    # not match `trust:` mid-edit and would surface a misleading "internal
    # error". Match by basename to cover both relative and absolute paths.
    BASENAME="${FILE##*/}"
    if [[ "${BASENAME}" == ".hector.yml" || "${BASENAME}" == ".bully.yml" ]]; then
      exit 0
    fi

    case "${TOOL}" in
      write_file)
        # toolArgs.content IS the post-edit content — no synthesis needed.
        # jq -j (raw, no trailing newline) preserves the exact bytes from the
        # JSON string: jq -r would append an extra \n after the value.
        echo "${EVENT}" | jq -j '.toolArgs.content // ""' | run_hector "${FILE}"
        ;;
      edit_file)
        # Synthesize post-edit content by applying the unique substitution
        # in-process. Mirrors Reasonix's own uniqueness check — if `search`
        # appears zero or more-than-once, fail closed (exit 2) so the
        # ambiguous edit is rejected before it lands. Env vars (not shell
        # interpolation) keep arbitrary substitution payloads safe.
        SEARCH=$(echo "${EVENT}" | jq -r '.toolArgs.search // empty')
        REPLACE=$(echo "${EVENT}" | jq -r '.toolArgs.replace // empty')
        if [[ -z "${SEARCH}" ]]; then
          exit 0
        fi
        PROPOSED=$(
          HECTOR_FILE="${FILE}" \
          HECTOR_SEARCH="${SEARCH}" \
          HECTOR_REPLACE="${REPLACE}" \
          python3 -c '
import os, sys
path = os.environ["HECTOR_FILE"]
search = os.environ["HECTOR_SEARCH"]
replace = os.environ.get("HECTOR_REPLACE", "")
try:
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
except OSError as e:
    print(f"hector: cannot read {path}: {e}", file=sys.stderr)
    sys.exit(2)
count = content.count(search)
if count != 1:
    print(
        f"hector: refusing edit_file — search string appears {count} times in {path}; "
        "Reasonix requires exactly one match",
        file=sys.stderr,
    )
    sys.exit(2)
sys.stdout.write(content.replace(search, replace, 1))
' && printf 'X'
        ) || exit 2
        # $(...) strips trailing newlines; the sentinel 'X' (appended only on
        # python success via &&) preserves them. Strip the sentinel to recover
        # byte-exact content including any trailing newline.
        PROPOSED=${PROPOSED%X}
        # printf '%s' avoids the extra \n that `echo` would append after the value.
        printf '%s' "${PROPOSED}" | run_hector "${FILE}"
        ;;
      *)
        # multi_edit and any future tool: currently no-op. multi_edit
        # would need to fold N edits onto N (path, content) pairs and
        # check each; first block aborts the whole tool call. Tracked as
        # follow-up in specs/2026-05-25-reasonix-adapter.md §9.3.
        exit 0
        ;;
    esac
    ;;
esac
