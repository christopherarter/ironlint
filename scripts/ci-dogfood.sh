#!/usr/bin/env bash
set -euo pipefail

# Run `hector check` against Hector's own Rust source. Used by CI to dogfood
# the policy engine on the codebase that ships it.
#
# Exits non-zero if any file produces a block (exit 2) or internal error.
# Warnings are surfaced but do not fail the build.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${REPO_ROOT}/.hector.yml"

if [[ ! -f "${CONFIG}" ]]; then
  echo "hector dogfood: no .hector.yml at repo root — skipping"
  exit 0
fi

# `hector check` fails closed on an unblessed config (the trust store). The
# repo's own committed config is trusted by definition here, so bless it once
# before dogfooding — without this every check returns exit 1 (not trusted).
hector trust --config "${CONFIG}"

blocked=0
internal=0
pass=0
files=0

while IFS= read -r file; do
  files=$((files + 1))
  EC=0
  hector check --file "${file}" --config "${CONFIG}" --format human || EC=$?
  case "${EC}" in
    0) pass=$((pass + 1)) ;;
    2)
      blocked=$((blocked + 1))
      echo "hector: BLOCK on ${file}" >&2
      ;;
    *)
      internal=$((internal + 1))
      echo "hector: internal error (${EC}) on ${file}" >&2
      ;;
  esac
done < <(find "${REPO_ROOT}/crates" -path '*/src/*.rs' -type f | sort)

echo
echo "hector dogfood summary: ${files} files, ${pass} pass, ${blocked} block, ${internal} internal-error"

if (( blocked > 0 || internal > 0 )); then
  exit 1
fi
