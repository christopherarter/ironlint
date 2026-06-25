// pi adapter for Hector. A pure translation layer between pi's extension
// lifecycle and the `hector` CLI — it contains no rule logic. See
// docs/superpowers/specs/2026-05-28-pi-adapter-design.md.

import { spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { basename, join } from "node:path"

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

// pi tools we gate. `bash` is intentionally not gated (shell redirections
// like `cat > foo` are too brittle to parse — universal adapter gap).
const GATED_TOOLS = new Set(["write", "edit"])

// R3: filenames hector treats as policy files. Edits to these short-circuit
// the gate — checking a mid-edit policy file fails the trust gate (sha
// mismatch) and surfaces a confusing internal error.
const POLICY_FILES = new Set([".hector.yml", ".bully.yml"])

/** R3: basename match covers both relative and absolute paths. */
export function isPolicyFile(filePath: string): boolean {
  return POLICY_FILES.has(basename(filePath))
}

/** pi uses `path`; `file_path` is tolerated as an alias. */
export function getPath(input: PiToolInput): string | undefined {
  return input.path ?? input.file_path
}

type ExecResult = { exitCode: number; stdout: string; stderr: string }

/**
 * Invoke the `hector` binary (must be on PATH). Uses node:child_process
 * spawnSync for deterministic stdin (`input`) + exit code (`status`). `status`
 * is null only when the process was killed by a signal; map that to -1 so it
 * falls through to fail-open.
 */
export function runHector(args: string[], input = ""): ExecResult {
  const res = spawnSync("hector", args, { input, encoding: "utf8" })
  return {
    exitCode: typeof res.status === "number" ? res.status : -1,
    stdout: res.stdout ?? "",
    stderr: res.stderr ?? "",
  }
}

/**
 * Shared exit-3 (engine-internal-error) policy: fail-open (log + allow) by
 * default; fail-closed (return a block) under HECTOR_FAIL_CLOSED_ON_INTERNAL=1.
 * A misconfigured hector must never brick the agent.
 */
function failOpenOrClosed(
  kind: string,
  stderr: string,
): { block: true; reason: string } | undefined {
  const suffix = stderr ? `: ${stderr}` : ""
  if (process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"] === "1") {
    console.error(
      `hector: internal error during ${kind} — failing closed (HECTOR_FAIL_CLOSED_ON_INTERNAL=1)${suffix}`,
    )
    return { block: true, reason: `hector: internal error during ${kind} — failing closed` }
  }
  console.error(
    `hector: internal error during ${kind} — allowing; see .hector/log.jsonl${suffix}`,
  )
  return undefined
}

/**
 * Translate a `hector check --format json` verdict into the user-facing block
 * reason pi surfaces. The CLI prints a Verdict JSON (schema_version 4) on
 * stdout; the human message lives in `blocks[].message` — surfacing raw stdout
 * would dump the whole JSON blob at the user. Falls back to a generic string
 * if stdout is not the expected JSON or carries no message.
 */
export function blockReason(stdout: string): string {
  try {
    const verdict = JSON.parse(stdout) as { blocks?: Array<{ message?: unknown }> }
    const messages = (verdict.blocks ?? [])
      .map((b) => b?.message)
      .filter((m): m is string => typeof m === "string" && m.length > 0)
    if (messages.length > 0) return messages.join("\n")
  } catch {
    // Not the expected JSON (e.g. a future format change) — fall through.
  }
  return "policy violation"
}

/** Minimal structural view of the pi extension API the adapter relies on. */
export interface PiExtensionAPI {
  on(event: string, handler: (event: never, ctx?: never) => unknown): void
  cwd?: string
  directory?: string
}

interface ToolCallEvent {
  toolName?: string
  toolCallId?: string
  input?: PiToolInput
}

/** Resolve the project root. process.cwd() is the terminal-agent fallback. */
function resolveRoot(pi: PiExtensionAPI): string {
  return pi.cwd ?? pi.directory ?? process.cwd()
}

/**
 * Hector pi extension. Registers one lifecycle handler: the `tool_call`
 * pre-write gate that checks proposed `write` / `edit` content against the
 * project's `.hector.yml` policy before the tool executes.
 */
export default function hectorExtension(pi: PiExtensionAPI): void {
  const projectRoot = resolveRoot(pi)
  const configPath = join(projectRoot, ".hector.yml")

  pi.on("tool_call", (event: ToolCallEvent) => {
    // Late existence check: the extension may load before `hector init`.
    // Re-checking here means mid-session init starts gating with no restart.
    if (!existsSync(configPath)) return
    const toolName = event?.toolName
    if (!toolName || !GATED_TOOLS.has(toolName)) return
    const input = event?.input ?? {}
    const filePath = getPath(input)
    if (!filePath) return
    if (isPolicyFile(filePath)) return // R3 self-edit short-circuit

    const proposed = computeProposedContent(toolName, filePath, input)
    if (proposed === null) return // can't faithfully simulate — skip the gate

    const res = runHector(
      ["check", "--file", filePath, "--content", "-", "--config", configPath, "--format", "json"],
      proposed,
    )
    if (res.exitCode === 0) return // pass/warn -> allow
    if (res.exitCode === 2) {
      return { block: true, reason: blockReason(res.stdout) }
    }
    if (res.exitCode === 3) {
      return failOpenOrClosed("check", res.stderr.trim())
    }
    // exit 1 / other -> config error: log + allow.
    const suffix = res.stderr.trim() ? `: ${res.stderr.trim()}` : ""
    console.error(`hector: internal error checking ${filePath} (exit ${res.exitCode})${suffix}`)
    return
  })
}
