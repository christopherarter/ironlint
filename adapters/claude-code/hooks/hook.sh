#!/usr/bin/env bash
set -euo pipefail

# Claude Code adapter for ironlint.
#
# Gates Write/Edit/MultiEdit/NotebookEdit edits via the PreToolUse hook. Claude
# Code hands us the proposed content *before* the write lands, so this script
# builds it in-process and pipes it to `ironlint check --file <path>
# --content -`:
#   - Write:        tool_input.content is already the post-edit content.
#   - Edit:         old_string -> new_string applied to the current on-disk file.
#   - MultiEdit:    the edits[] array folded sequentially onto the on-disk file
#                   (each edit against the post-previous-edits content), mirroring
#                   Claude Code's own apply semantics so what we gate == what it
#                   would write.
#   - NotebookEdit: the edited cell's new_source (delete removes a cell — nothing
#                   to gate, so it's allowed).
# Exit codes map onto Claude Code's PreToolUse block/allow semantics:
#   - 0 = allow (exit 0 lets the edit proceed),
#   - 2 = deny (verdict JSON on stderr; Claude Code surfaces stderr to the
#         model as the reason the tool call was blocked),
#   - see run_ironlint below for the full exit-code mapping.
#
# Event JSON arrives on stdin:
#   { "tool_name": "Write" | "Edit" | "MultiEdit" | "NotebookEdit",
#     "tool_input": {
#       "file_path": "...",                                   # Write/Edit/MultiEdit
#       "content": "...",                                     # Write
#       "old_string": "...", "new_string": "...",
#       "replace_all": bool,                                  # Edit
#       "edits": [ {old_string, new_string, replace_all?} ],  # MultiEdit
#       "notebook_path": "...", "new_source": "...",
#       "edit_mode": "replace" | "insert" | "delete"          # NotebookEdit
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
# NotebookEdit names the target `notebook_path` rather than `file_path`; include
# it so `$FILE` (and the `.ironlint.yml` skip + the gate's `--file`) work for it.
FILE=$(echo "${EVENT}" | jq -r '.tool_input.file_path // .tool_input.path // .tool_input.notebook_path // empty')
if [[ -z "${FILE}" ]]; then
  # No file in event payload — nothing to check.
  exit 0
fi

# Short-circuit on edits to the policy file itself: the on-disk hash won't
# match the trusted store while the user is mid-edit, so any `ironlint`
# invocation would fail the trust gate and surface a misleading "internal
# error" to the user. Match by basename so the skip works for both relative
# and absolute paths Claude Code may send.
BASENAME="${FILE##*/}"
if [[ "${BASENAME}" == ".ironlint.yml" ]]; then
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
  Edit | MultiEdit)
    # Synthesize post-edit content by folding the tool's edit(s) onto the
    # current on-disk file, mirroring Claude Code's own apply + uniqueness
    # rules so the bytes we gate == the bytes Claude Code would write. Edit is
    # just MultiEdit with a single edit, so one shared python handles both:
    #   - Edit:      one edit from old_string/new_string/replace_all.
    #   - MultiEdit: tool_input.edits[] applied IN ORDER, each against the
    #                post-previous-edits content (uniqueness re-checked each
    #                step). Empty edits[] → nothing to gate (exit 3 → allow).
    # Unless replace_all is set, an edit's old_string must appear exactly once
    # in the current content, else Claude Code itself refuses the edit — fail
    # closed (exit 2) here to match.
    #
    # The event JSON is read INSIDE python via json.loads, never round-tripped
    # through a shell `$(...)` substitution — `$(...)` strips ALL trailing
    # newlines, which would desync the bytes ironlint gates from the bytes
    # written to disk (e.g. a new_string ending in "\n" would silently lose
    # it). The JSON travels only as an env var (never shell-interpolated), so
    # arbitrary substitution payloads stay safe from injection. Python's stdout
    # is redirected straight to a temp file (not captured via `$(...)` either)
    # so the synthesized content's own trailing newline, if any, survives
    # byte-for-byte.
    #
    # python exit codes (local to this branch only):
    #   0 → synthesized content written to stdout
    #   2 → block: an old_string isn't unique/found, or the on-disk file
    #       couldn't be read — mirrors Claude Code's own refusal
    #   3 → nothing to synthesize (Edit with no old_string, or MultiEdit with
    #       an empty edits[] array) — allow
    TMP_PROPOSED=$(mktemp -t ironlint-edit.XXXXXX)
    PY_EC=0
    IRONLINT_EVENT_JSON="${EVENT}" IRONLINT_FILE="${FILE}" python3 -c '
import json, os, sys

ev = json.loads(os.environ["IRONLINT_EVENT_JSON"])
ti = ev.get("tool_input") or {}
tool = ev.get("tool_name")
path = os.environ["IRONLINT_FILE"]

# Build the ordered edit list. Edit == MultiEdit with a single edit.
if tool == "MultiEdit":
    edits = ti.get("edits") or []
    if not edits:
        sys.exit(3)  # empty edits[] — nothing to gate
else:
    old = ti.get("old_string")
    if not old:
        sys.exit(3)  # Edit with no old_string — nothing to synthesize
    edits = [{
        "old_string": old,
        "new_string": ti.get("new_string", ""),
        "replace_all": ti.get("replace_all", False),
    }]

try:
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
except UnicodeDecodeError:
    # UnicodeDecodeError subclasses ValueError, NOT OSError, so the handler
    # below would miss it and Python would dump a raw traceback to stderr
    # before falling to the `*) exit 2` arm. Block (unchanged direction) with a
    # clean, single-line reason instead of the traceback.
    print(
        f"ironlint: cannot decode {path} as UTF-8 — ironlint gates UTF-8 text files only",
        file=sys.stderr,
    )
    sys.exit(2)
except OSError as e:
    print(f"ironlint: cannot read {path}: {e}", file=sys.stderr)
    sys.exit(2)

# Fold edits sequentially: each edit sees the result of the prior ones, so
# uniqueness is judged against the CURRENT content, exactly like Claude Code.
for edit in edits:
    old = edit.get("old_string", "")
    new = edit.get("new_string", "")
    replace_all = bool(edit.get("replace_all", False))
    count = content.count(old)
    if replace_all:
        if count == 0:
            print(
                f"ironlint: refusing edit — old_string not found in {path}",
                file=sys.stderr,
            )
            sys.exit(2)
        content = content.replace(old, new)
    else:
        if count != 1:
            print(
                f"ironlint: refusing edit — old_string appears {count} times in "
                f"{path}; Claude Code requires exactly one match unless "
                "replace_all is set",
                file=sys.stderr,
            )
            sys.exit(2)
        content = content.replace(old, new, 1)

sys.stdout.write(content)
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
  NotebookEdit)
    # A notebook cell edit. The proposed cell source is tool_input.new_source;
    # gate it as-is (byte-exact via `jq -j`, like the Write path) against any
    # check whose files glob matches the notebook path. `$IRONLINT_FILE` is the
    # .ipynb, stdin is the new cell source.
    #   - edit_mode "delete" removes a cell: there is no proposed content to
    #     gate, so allow (exit 0) without invoking ironlint.
    #   - "replace"/"insert" (default replace): gate new_source.
    # Note: a check scoped to `*.py` won't match a `.ipynb`; scope it to
    # `*.ipynb` to gate notebook cell edits (see docs/adapters/claude-code.md).
    EDIT_MODE=$(echo "${EVENT}" | jq -r '.tool_input.edit_mode // "replace"')
    if [[ "${EDIT_MODE}" == "delete" ]]; then
      exit 0
    fi
    echo "${EVENT}" | jq -j '.tool_input.new_source // ""' | run_ironlint "${FILE}"
    ;;
  *)
    # Any tool_name NOT in {Write, Edit, MultiEdit, NotebookEdit}: the hook's
    # registration matcher catches this call, but there is no gating logic for
    # it here — silently allowing it through would be a policy bypass: the tool
    # matched the hook's registration but never reached `ironlint check`. Fail
    # LOUD and CLOSED instead — block and name the tool — so an ungated edit is
    # never mistaken for an allowed one.
    echo "ironlint: tool '${TOOL_NAME}' is not yet gated by ironlint — refusing (it would bypass policy checks). Use Write/Edit." >&2
    exit 2
    ;;
esac
