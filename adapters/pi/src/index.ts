// pi adapter for Hector. A pure translation layer between pi's extension
// lifecycle and the `hector` CLI — it contains no rule logic. See
// docs/superpowers/specs/2026-05-28-pi-adapter-design.md.

import { existsSync, readFileSync } from "node:fs"

/** The shape of the input payload pi passes for `write` / `edit` tool calls. */
export type PiToolInput = {
  path?: string
  // pi's renderer tolerates `file_path` as a `path` alias.
  file_path?: string
  // write tool: full post-write body.
  content?: string
  // edit tool: batch of replacements.
  edits?: Array<{ oldText?: string; newText?: string }>
  // edit tool: legacy single-edit form, normalized by pi into edits[].
  oldText?: string
  newText?: string
}

type Edit = { oldText: string; newText: string }

/**
 * Normalize an edit-tool input into a flat `{oldText,newText}[]`.
 *
 *   - `edits[]` (the canonical batch form) is validated member-by-member;
 *     any non-string `oldText`/`newText` poisons the whole batch -> null.
 *   - legacy top-level `{oldText,newText}` -> single-element array
 *     (missing `newText` defaults to "").
 *   - anything else (a write call, malformed input) -> null.
 *
 * Returns null when the input is not a recognizable edit (the caller then
 * skips the gate / falls back), never throws.
 */
export function normalizeEdits(input: PiToolInput): Edit[] | null {
  if (Array.isArray(input.edits)) {
    const out: Edit[] = []
    for (const e of input.edits) {
      if (typeof e?.oldText !== "string" || typeof e?.newText !== "string") {
        return null
      }
      out.push({ oldText: e.oldText, newText: e.newText })
    }
    return out.length > 0 ? out : null
  }
  if (typeof input.oldText === "string") {
    return [
      {
        oldText: input.oldText,
        newText: typeof input.newText === "string" ? input.newText : "",
      },
    ]
  }
  return null
}

// A line that looks like a real unified-diff header. Used to neutralize
// attacker-controlled old/new blocks (P1-9).
const DIFF_HEADER_RE = /^(---|\+\+\+|@@) /

/**
 * Prefix any line that mimics a diff header with a backslash so hector's
 * diff parser does not mistake user content for a real `--- a/...`,
 * `+++ b/...`, or `@@ ... @@` header (P1-9). We scrub the already-prefixed
 * block so a malicious old line `-- a/SECRET` (which becomes `--- a/SECRET`
 * after the `-` prefix) is also caught.
 */
function scrub(block: string): string {
  return block
    .split("\n")
    .map((l) => (DIFF_HEADER_RE.test(l) ? "\\" + l : l))
    .join("\n")
}

/**
 * Build a single `@@ ... @@` hunk from one (oldText, newText) pair.
 *
 * P1-8: a literal `@@ -1 +1 @@` is wrong the moment either side has more
 * than one line — hector's parser uses the header counts to number added
 * lines. Emit `1,N` form whenever a side has > 1 line, and omit a side's
 * block entirely when it is empty (a pure addition / pure deletion).
 */
function buildHunk(oldText: string, newText: string): string {
  const oldLines = oldText === "" ? 0 : oldText.split("\n").length
  const newLines = newText === "" ? 0 : newText.split("\n").length
  const hunkOld = oldLines <= 1 ? "1" : `1,${oldLines}`
  const hunkNew = newLines <= 1 ? "1" : `1,${newLines}`
  const oldBlock =
    oldText === "" ? "" : oldText.split("\n").map((l) => "-" + l).join("\n") + "\n"
  const newBlock =
    newText === "" ? "" : newText.split("\n").map((l) => "+" + l).join("\n") + "\n"
  return `@@ -${hunkOld} +${hunkNew} @@\n${scrub(oldBlock)}${scrub(newBlock)}`
}

/**
 * Build a synthetic unified diff for a write/edit tool call so
 * `hector session record` can ingest it. pi's tool events carry no real
 * diff. A `write` is the single-hunk `"" -> content` case; an `edit` is a
 * batch, so we emit one scrubbed hunk per `{oldText,newText}` under a single
 * file header.
 *
 * Exported for unit testing.
 */
export function synthesizeDiff(
  toolName: string,
  filePath: string,
  input: PiToolInput,
): string {
  const header = `--- a/${filePath}\n+++ b/${filePath}\n`
  if (toolName === "write") {
    const content = typeof input.content === "string" ? input.content : ""
    return header + buildHunk("", content)
  }
  const edits = normalizeEdits(input)
  if (edits === null) {
    // Unrecognizable edit — emit an empty single hunk so the call is still
    // a syntactically valid (no-op) diff rather than throwing.
    return header + buildHunk("", "")
  }
  return header + edits.map((e) => buildHunk(e.oldText, e.newText)).join("")
}

/**
 * Compute the file body pi is about to write, so the gate can pipe it to
 * `hector check --content -`. See spec §5.1.
 *
 *   - `write` -> `input.content` (the full body), even for a new file.
 *     Non-string content (malformed call) -> null; pi would reject it too.
 *   - `edit`  -> read the current file, apply each `{oldText,newText}` in
 *     order. Each `oldText` must occur EXACTLY ONCE in the working buffer
 *     (mirrors pi's contract); on any miss or non-unique match -> null.
 *     A non-existent file -> null.
 *
 * We deliberately do NOT reproduce pi's fuzzy-match fallback — diverging
 * there would feed hector content pi won't actually write, risking false
 * blocks. Returning null skips the gate (fail-open on simulate-failure).
 */
export function computeProposedContent(
  toolName: string,
  filePath: string,
  input: PiToolInput,
): string | null {
  if (toolName === "write") {
    return typeof input.content === "string" ? input.content : null
  }
  if (toolName === "edit") {
    const edits = normalizeEdits(input)
    if (edits === null) return null
    if (!existsSync(filePath)) return null
    let buf = readFileSync(filePath, "utf8")
    for (const { oldText, newText } of edits) {
      const first = buf.indexOf(oldText)
      if (first === -1) return null
      // Reject non-unique matches (and empty oldText, where first=0 and
      // last=buf.length) so we never guess which occurrence pi means.
      if (first !== buf.lastIndexOf(oldText)) return null
      buf = buf.slice(0, first) + newText + buf.slice(first + oldText.length)
    }
    return buf
  }
  return null
}
