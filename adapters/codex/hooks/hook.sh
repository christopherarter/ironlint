#!/usr/bin/env bash
set -euo pipefail

# Codex adapter for ironlint. Gates `apply_patch` edits via Codex's PreToolUse
# hook — see specs/2026-07-02-drop-reasonix-add-codex-adapter-design.md.
#
# Codex's block contract is unlike the exit-code hooks:
#   ALLOW = exit 0 with EMPTY stdout.
#   BLOCK = exit 0 with a permissionDecision:"deny" JSON on stdout.
# An exit code NEVER blocks; malformed stdout FAILS OPEN (the edit lands). So
# every block path emits well-formed deny JSON (via jq, with a static fallback)
# before exiting, and the script never dies between "decided to block" and
# "emitted".
#
# stdin: {"tool_name":"apply_patch","cwd":"...",
#         "tool_input":{"command":"*** Begin Patch ... *** End Patch"}}
# arg1 (the hook event name) is accepted and ignored.

WORKDIR=""
cleanup() {
  if [[ -n "${WORKDIR}" && -d "${WORKDIR}" ]]; then
    rm -rf "${WORKDIR}"
  fi
}
trap cleanup EXIT

# Emit a Codex deny verdict and exit 0. Blocking rides on this JSON, never the
# exit code. jq builds it from an arbitrary reason; if jq fails, a static,
# guaranteed-valid deny is printed so a block is NEVER silently dropped.
deny() {
  local reason=$1
  local out
  if out=$(jq -cn --arg r "${reason}" \
      '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}' \
      2>/dev/null); then
    printf '%s\n' "${out}"
  else
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"ironlint blocked this edit"}}'
  fi
  exit 0
}

EVENT=$(cat)

# Malformed stdin must never brick the agent: allow gracefully.
if ! printf '%s' "${EVENT}" | jq empty >/dev/null 2>&1; then
  echo "ironlint: malformed event JSON on stdin — skipping (allow)" >&2
  exit 0
fi

CWD=$(printf '%s' "${EVENT}" | jq -r '.cwd // empty')
PROJECT_ROOT="${CWD:-$(pwd)}"
CONFIG="${PROJECT_ROOT}/.ironlint.yml"

# Skip silently if ironlint isn't configured for this project.
if [[ ! -f "${CONFIG}" ]]; then
  exit 0
fi

TOOL_NAME=$(printf '%s' "${EVENT}" | jq -r '.tool_name // empty')

# Only file edits (apply_patch) are gated. Codex reports tool_name:"apply_patch"
# for file edits even when the matcher aliased Edit/Write. Anything else that
# reaches here (matcher over-broad) is allowed — ironlint gates file edits only.
if [[ "${TOOL_NAME}" != "apply_patch" ]]; then
  exit 0
fi

PATCH=$(printf '%s' "${EVENT}" | jq -r '.tool_input.command // empty')
if [[ -z "${PATCH}" ]]; then
  exit 0
fi

WORKDIR=$(mktemp -d -t ironlint-codex.XXXXXX)

# Parse the apply_patch envelope → synthesize each touched file's post-edit
# content into WORKDIR, printing a "<abs-path>\t<content-tmpfile>" manifest.
# Fail CLOSED (python exit != 0) on an unparseable / non-applying patch: an
# un-gated edit must never be mistaken for an allowed one.
PY_EC=0
IRONLINT_EVENT_JSON="${EVENT}" \
IRONLINT_PROJECT_ROOT="${PROJECT_ROOT}" \
IRONLINT_WORKDIR="${WORKDIR}" \
python3 -c '
import json, os, sys

root = os.environ["IRONLINT_PROJECT_ROOT"]
workdir = os.environ["IRONLINT_WORKDIR"]
ev = json.loads(os.environ["IRONLINT_EVENT_JSON"])
cmd = (ev.get("tool_input") or {}).get("command") or ""

def fail(msg):
    print(msg, file=sys.stderr)
    sys.exit(2)

b = cmd.find("*** Begin Patch")
e = cmd.find("*** End Patch")
if b == -1 or e == -1 or e < b:
    fail("no apply_patch envelope in tool_input.command")
lines = cmd[b:e].splitlines()[1:]  # drop the "*** Begin Patch" marker line

manifest = []
_n = [0]
def emit(relpath, content):
    _n[0] += 1
    tf = os.path.join(workdir, "content-%d" % _n[0])
    with open(tf, "w", encoding="utf-8") as f:
        f.write(content)
    ap = os.path.normpath(os.path.join(root, relpath))
    manifest.append("%s\t%s" % (ap, tf))

def find_block(hay, needle):
    if not needle:
        return None
    for i in range(0, len(hay) - len(needle) + 1):
        if hay[i:i+len(needle)] == needle:
            return i
    return None

def apply_update(relpath, hunk):
    """Splice apply_patch context sub-hunks into the on-disk file. None on miss."""
    try:
        with open(os.path.join(root, relpath), "r", encoding="utf-8") as f:
            cur = f.read()
    except OSError as ex:
        fail("cannot read %s for update: %s" % (relpath, ex))
    final_nl = cur.endswith("\n")
    cur_lines = cur.split("\n")
    if final_nl:
        cur_lines = cur_lines[:-1]
    subs, buf = [], []
    for hl in hunk:
        if hl.startswith("@@"):
            if buf:
                subs.append(buf); buf = []
        else:
            buf.append(hl)
    if buf:
        subs.append(buf)
    for sub in subs:
        before, after = [], []
        for hl in sub:
            if hl[:1] in (" ", "+", "-"):
                tag, text = hl[0], hl[1:]
            else:
                tag, text = " ", hl
            if tag in (" ", "-"):
                before.append(text)
            if tag in (" ", "+"):
                after.append(text)
        idx = find_block(cur_lines, before)
        if idx is None:
            return None
        cur_lines = cur_lines[:idx] + after + cur_lines[idx+len(before):]
    out = "\n".join(cur_lines)
    if final_nl:
        out += "\n"
    return out

i, N = 0, len(lines)
while i < N:
    ln = lines[i]
    if ln.startswith("*** Add File: "):
        rel = ln[len("*** Add File: "):].strip(); i += 1
        added = []
        while i < N and not lines[i].startswith("*** "):
            s = lines[i]
            added.append(s[1:] if s.startswith("+") else s)
            i += 1
        emit(rel, ("\n".join(added) + "\n") if added else "")
    elif ln.startswith("*** Update File: "):
        rel = ln[len("*** Update File: "):].strip(); i += 1
        dest = rel
        if i < N and lines[i].startswith("*** Move to: "):
            dest = lines[i][len("*** Move to: "):].strip(); i += 1
        hunk = []
        while i < N and not lines[i].startswith("*** "):
            hunk.append(lines[i]); i += 1
        content = apply_update(rel, hunk)
        if content is None:
            fail("apply_patch hunk did not apply cleanly to %s" % rel)
        emit(dest, content)
    elif ln.startswith("*** Delete File: "):
        i += 1  # nothing to gate on a deletion
    else:
        i += 1

sys.stdout.write("\n".join(manifest))
if manifest:
    sys.stdout.write("\n")
sys.exit(0)
' > "${WORKDIR}/manifest" 2>"${WORKDIR}/pyerr" || PY_EC=$?

if [[ "${PY_EC}" -ne 0 ]]; then
  deny "ironlint: could not gate apply_patch — $(tr '\n' ' ' < "${WORKDIR}/pyerr")"
fi

# Run ironlint per touched file; first block wins. No manifest lines (e.g. a
# delete-only patch) → nothing to gate → allow. Reading from a file (not a
# pipe) keeps the loop in this shell, so `deny`'s exit ends the whole script.
while IFS=$'\t' read -r ABSPATH CONTENTFILE; do
  if [[ -z "${ABSPATH}" ]]; then
    continue
  fi
  # Skip edits to the policy file itself: its on-disk sha won't match trust
  # mid-edit, so any check would fail the trust gate misleadingly.
  BASENAME="${ABSPATH##*/}"
  if [[ "${BASENAME}" == ".ironlint.yml" || "${BASENAME}" == ".bully.yml" ]]; then
    continue
  fi
  EC=0
  ironlint check --file "${ABSPATH}" --content - --config "${CONFIG}" --format json \
    > "${WORKDIR}/verdict" 2>/dev/null < "${CONTENTFILE}" || EC=$?
  case "${EC}" in
    0) : ;;
    2) deny "$(cat "${WORKDIR}/verdict")" ;;
    4) deny "ironlint is configured here but not trusted — run 'ironlint trust' to enable checks" ;;
    3)
      if [[ "${IRONLINT_FAIL_CLOSED_ON_INTERNAL:-0}" == "1" ]]; then
        deny "ironlint: check errored (exit 3) for ${ABSPATH} — blocking (fail-closed)"
      fi
      echo "ironlint: check errored (exit 3) for ${ABSPATH} — allowing (fail-open default)" >&2
      ;;
    1) echo "ironlint: config/load error (exit 1) — see 'ironlint doctor'" >&2 ;;
    *) echo "ironlint: unexpected ironlint exit ${EC} for ${ABSPATH}" >&2 ;;
  esac
done < "${WORKDIR}/manifest"

exit 0   # every file passed (or fail-open) → allow, empty stdout
