// pi adapter for IronLint. A pure translation layer between pi's extension
// lifecycle and the `ironlint` CLI — it contains no rule logic. See
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
  // bash tool: the raw shell command the agent wants to run.
  command?: string
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
 * `ironlint check --content -`. See spec §5.1.
 *
 *   - `write` -> `input.content` (the full body), even for a new file.
 *     Non-string content (malformed call) -> null; pi would reject it too.
 *   - `edit`  -> read the current file, apply each `{oldText,newText}` in
 *     order. Each `oldText` must occur EXACTLY ONCE in the working buffer
 *     (mirrors pi's contract); on any miss or non-unique match -> null.
 *     A non-existent file -> null.
 *
 * We deliberately do NOT reproduce pi's fuzzy-match fallback — diverging
 * there would feed ironlint content pi won't actually write, risking false
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

// pi tools we gate. `bash` is gated by the bash-gate branch below, which
// shells out to `ironlint gate-bash` (the shared Rust matcher) — closing the
// "shell redirections are too brittle to parse" gap that previously kept bash
// ungated. See
// docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md.
const GATED_TOOLS = new Set(["write", "edit", "bash"])

// R3: filenames ironlint treats as policy files. Edits to these short-circuit
// the gate — checking a mid-edit policy file fails the trust gate (sha
// mismatch) and surfaces a confusing internal error.
const POLICY_FILES = new Set([".ironlint.yml", ".bully.yml"])

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
 * Invoke the `ironlint` binary (must be on PATH). Uses node:child_process
 * spawnSync for deterministic stdin (`input`) + exit code (`status`). `status`
 * is null only when the process was killed by a signal; map that to -1 so it
 * falls through to fail-open.
 */
export function runIronLint(args: string[], input = ""): ExecResult {
  const res = spawnSync("ironlint", args, { input, encoding: "utf8" })
  return {
    exitCode: typeof res.status === "number" ? res.status : -1,
    stdout: res.stdout ?? "",
    stderr: res.stderr ?? "",
  }
}

/**
 * Shared exit-3 (engine-internal-error) policy: fail-open (log + allow) by
 * default; fail-closed (return a block) under IRONLINT_FAIL_CLOSED_ON_INTERNAL=1.
 * A misconfigured ironlint must never brick the agent.
 */
function failOpenOrClosed(
  kind: string,
  stderr: string,
): { block: true; reason: string } | undefined {
  const suffix = stderr ? `: ${stderr}` : ""
  if (process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"] === "1") {
    console.error(
      `ironlint: internal error during ${kind} — failing closed (IRONLINT_FAIL_CLOSED_ON_INTERNAL=1)${suffix}`,
    )
    return { block: true, reason: `ironlint: internal error during ${kind} — failing closed` }
  }
  console.error(
    `ironlint: internal error during ${kind} — allowing; see .ironlint/log.jsonl${suffix}`,
  )
  return undefined
}

/**
 * Translate a `ironlint check --format json` verdict into the user-facing block
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
 * IronLint pi extension. Registers one lifecycle handler: the `tool_call`
 * pre-write gate that checks proposed `write` / `edit` content against the
 * project's `.ironlint.yml` policy before the tool executes.
 */
export default function ironlintExtension(pi: PiExtensionAPI): void {
  const projectRoot = resolveRoot(pi)
  const configPath = join(projectRoot, ".ironlint.yml")

  pi.on("tool_call", (event: ToolCallEvent) => {
    const toolName = event?.toolName
    if (!toolName || !GATED_TOOLS.has(toolName)) return
    const input = event?.input ?? {}

    // Bash branch: the bash-gate. Runs BEFORE the config-existence check —
    // the bash-gate must fire even with no .ironlint.yml, since that's exactly
    // when an agent is most motivated to run `ironlint trust`. Decides whether
    // the command would let the agent free itself (`ironlint trust`, or a Bash
    // write to `.ironlint.yml` / `.ironlint/gates/`). The deny logic lives in
    // `ironlint gate-bash` — the single source shared across every adapter.
    if (toolName === "bash") {
      const command = typeof input.command === "string" ? input.command : ""
      // Substring pre-filter: ordinary commands (ls, git, cargo) never mention
      // ironlint or .ironlint, so skip the spawn entirely — they pay nothing.
      if (!command.includes("ironlint") && !command.includes(".ironlint")) return
      const res = runIronLint(["gate-bash"], command)
      if (res.exitCode === 0) return // allow
      if (res.exitCode === 2) {
        return { block: true, reason: res.stdout || "ironlint blocked this bash command" }
      }
      // Spawn failure (exitCode -1, signal death) / unexpected exit → fail
      // CLOSED. The deny check is the thing being protected; a broken deny
      // check is never a silent allow. Do NOT reuse failOpenOrClosed — that's
      // the opposite posture (the file-gate fail-opens on internal errors).
      return {
        block: true,
        reason: `ironlint: bash-gate failed (exit ${res.exitCode}) — fail-closed`,
      }
    }

    // Late existence check: the extension may load before `ironlint init`.
    // Re-checking here means mid-session init starts gating with no restart.
    if (!existsSync(configPath)) return
    const filePath = getPath(input)
    if (!filePath) return
    if (isPolicyFile(filePath)) return // R3 self-edit short-circuit

    const proposed = computeProposedContent(toolName, filePath, input)
    if (proposed === null) return // can't faithfully simulate — skip the gate

    const res = runIronLint(
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
    if (res.exitCode === 4) {
      // Untrusted/tampered config (Task 3.2 / Finding C3): fail CLOSED,
      // unlike the generic exit-1 catch-all below. An untrusted config must
      // never be silently un-gated just because nobody re-ran `ironlint
      // trust` after pulling a changed `.ironlint.yml`.
      return {
        block: true,
        reason: "ironlint is configured here but not trusted — run 'ironlint trust' to enable checks",
      }
    }
    // exit 1 / other -> config error: log + allow.
    const suffix = res.stderr.trim() ? `: ${res.stderr.trim()}` : ""
    console.error(`ironlint: internal error checking ${filePath} (exit ${res.exitCode})${suffix}`)
    return
  })
}
