#!/usr/bin/env bash
set -euo pipefail

# Enforce a minimum *region* coverage threshold on every Rust source file
# under `crates/*/src/`. Region coverage counts distinct decision points
# (branches, short-circuits, match arms) rather than executed lines —
# harder to satisfy with hollow tests that just walk straight-line code.
#
# Used by CI; runnable locally with the same dependencies.
#
# Requires: cargo, cargo-llvm-cov, jq, awk. POSIX-portable; runs on Linux
# (GitHub `ubuntu-latest`) and macOS (`macos-latest` and dev machines).
#
# Configuration:
#   COVERAGE_THRESHOLD — minimum region coverage percent per file (default: 80)
#
# Exit codes:
#   0 — every source file meets the threshold
#   1 — at least one file is below threshold, or a required tool is missing

# Lock numeric formatting so `awk` parses cargo-llvm-cov's JSON decimals
# consistently regardless of the developer's locale settings.
export LC_ALL=C

THRESHOLD="${COVERAGE_THRESHOLD:-80}"

for tool in cargo jq awk; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "ci-coverage: required tool not found: ${tool}" >&2
    exit 1
  fi
done

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "ci-coverage: cargo-llvm-cov is not installed. Install with:" >&2
  echo "  cargo install cargo-llvm-cov" >&2
  echo "and ensure the llvm-tools rustup component is present." >&2
  exit 1
fi

# Bare `mktemp` is the only invocation BSD (macOS) and GNU (Linux) treat
# identically — both return a unique, writable path under `$TMPDIR`/`/tmp`.
REPORT="$(mktemp)"
trap 'rm -f "${REPORT}"' EXIT

echo "ci-coverage: collecting coverage (threshold: ${THRESHOLD}% per file)…"
cargo llvm-cov --workspace --json --quiet --output-path "${REPORT}"

# Walk every file in the report, comparing line coverage against the threshold.
# `awk` does the float compare to avoid bash's integer-only `(( ))`.
# Paths are stripped down to a `crates/...` suffix by jq — anchoring on
# `/crates/` rather than the absolute repo root sidesteps macOS case-folding
# (where `pwd` may differ in case from cargo's canonical filename).
below=()
while IFS=$'\t' read -r filename pct; do
  [[ "${filename}" == *.rs ]] || continue
  if awk -v p="${pct}" -v t="${THRESHOLD}" 'BEGIN { exit !(p + 0 < t + 0) }'; then
    below+=("$(printf '  %s — %.2f%%' "${filename}" "${pct}")")
  fi
done < <(jq -r '.data[0].files[] | [(.filename | sub("^.*?/crates/"; "crates/")), .summary.regions.percent] | @tsv' "${REPORT}")

total_regions_pct=$(jq -r '.data[0].totals.regions.percent' "${REPORT}")
total_lines_pct=$(jq -r '.data[0].totals.lines.percent' "${REPORT}")
total_funcs_pct=$(jq -r '.data[0].totals.functions.percent' "${REPORT}")

echo
echo "ci-coverage: workspace totals — regions ${total_regions_pct}%, lines ${total_lines_pct}%, functions ${total_funcs_pct}%"

if (( ${#below[@]} > 0 )); then
  echo
  echo "ci-coverage: ${#below[@]} file(s) below ${THRESHOLD}% region coverage:" >&2
  printf '%s\n' "${below[@]}" >&2
  echo >&2
  echo "Raise coverage on the files above, or override the gate locally with" >&2
  echo "COVERAGE_THRESHOLD=N for an investigation run." >&2
  exit 1
fi

echo "ci-coverage: all files ≥ ${THRESHOLD}% region coverage."
