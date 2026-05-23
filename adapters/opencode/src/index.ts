import type { Plugin } from "@opencode-ai/plugin"
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { basename, join } from "node:path"

// OpenCode tools we gate. `apply_patch` is intentionally not gated at 0.1d
// (P2-14, deferred) — the opencode plugin SDK does not currently surface
// an `apply_patch` tool through `tool.execute.after`, and its multi-file
// patch format would need per-file extraction (split on `+++ b/<path>`
// boundaries, reissue `hector check --file` per file). See
// docs/adapters/opencode.md → "What it does NOT do" for the known-gap
// note. Tracked until the apply_patch tool is wired through the adapter.
const GATED_TOOLS = new Set(["edit", "write"])

// R3: filenames hector recognizes as policy files. Edits to these files
// must short-circuit both adapter hooks — running `hector check` against
// a mid-edit policy file fails the trust gate (sha mismatch) and
// surfaces a confusing "internal error" to the user.
const POLICY_FILES = new Set([".hector.yml", ".bully.yml"])

function isPolicyFile(filePath: string): boolean {
  return POLICY_FILES.has(basename(filePath))
}

// Opencode's tool args use `find` / `replace` / `replaceAll` for the edit
// tool and `content` for the write tool (confirmed against the opencode
// 1.14.x binary). We keep the legacy `oldString` / `newString` names as
// fallbacks for older opencode versions.
type FileToolArgs = {
  filePath?: string
  find?: string
  replace?: string
  replaceAll?: boolean
  content?: string
  // Legacy fallbacks — older opencode shipped these names.
  oldString?: string
  newString?: string
}

function getOldString(args: FileToolArgs): string | undefined {
  return args.find ?? args.oldString
}

function getNewString(args: FileToolArgs): string | undefined {
  return args.replace ?? args.newString
}

/**
 * Hector OpenCode plugin.
 *
 *   - `tool.execute.before` on `edit`/`write` → shadow-write the proposed
 *      content to the target path, run `hector check --file <path>`, then
 *      always restore the pre-edit state. Throw on block so OpenCode never
 *      executes the tool (exit-code contract: 0 = pass/warn, 2 = block).
 *   - `tool.execute.after` on `edit`/`write` → `hector session record` for
 *      cross-edit (session-rule) tracking.
 *   - `event` filtering on `session.created` → clear stale `.hector/session.json`.
 *   - `event` filtering on `session.idle` → `hector check --session`.
 *
 * Hector itself is invoked as a child process via Bun's `$` API. The
 * plugin contains no rule logic — it's purely a translation layer between
 * OpenCode's lifecycle and the `hector` CLI.
 */
export const HectorPlugin: Plugin = async ({ $, directory, worktree }) => {
  const projectRoot = worktree || directory
  const configPath = join(projectRoot, ".hector.yml")
  const sessionStatePath = join(projectRoot, ".hector", "session.json")

  return {
    "tool.execute.before": async (input, output) => {
      // Late existence check: opencode may load this plugin once at startup,
      // before the project is initialized as a hector project. Re-check on
      // every invocation so that `hector init` mid-session starts gating.
      if (!existsSync(configPath)) return
      if (!GATED_TOOLS.has(input.tool)) return

      const args = (output.args ?? {}) as FileToolArgs
      const filePath = args.filePath
      if (!filePath) return
      // R3: skip self-checks of the policy file itself.
      if (isPolicyFile(filePath)) return

      const proposed = computeProposedContent(filePath, args)
      if (proposed === null) return // can't simulate — skip the gate

      const originalExists = existsSync(filePath)
      const original = originalExists ? readFileSync(filePath, "utf8") : null

      // Shadow-write the proposed content so rules that shell out and read
      // `{file}` from disk (grep, biome, depcruise) see what opencode is
      // about to write. Always restore in `finally`, whether the check
      // passes or blocks.
      let result: Awaited<ReturnType<typeof $>>
      try {
        writeFileSync(filePath, proposed)
        result =
          await $`hector check --file ${filePath} --config ${configPath} --format json`
            .quiet()
            .nothrow()
      } finally {
        restoreFile(filePath, original)
      }

      // Exit code contract (commands/check.rs):
      //   0 → pass or warn  (allow opencode to run the tool)
      //   2 → block         (throw — opencode cancels the tool call)
      //   1 → internal      (log to stderr, allow — agent shouldn't be
      //                      blocked by an unrelated hector failure)
      if (result.exitCode === 2) {
        const verdict = result.stdout.toString().trim() || "rule violation"
        throw new Error(`hector blocked this edit:\n${verdict}`)
      }
      if (result.exitCode !== 0) {
        const stderr = result.stderr.toString().trim()
        console.error(
          `hector: internal error checking ${filePath} (exit ${result.exitCode})${stderr ? `: ${stderr}` : ""}`,
        )
      }
    },

    "tool.execute.after": async (input) => {
      if (!existsSync(configPath)) return
      if (!GATED_TOOLS.has(input.tool)) return

      const args = input.args as FileToolArgs | undefined
      const filePath = args?.filePath
      if (!filePath) return
      // R3: don't record edits to the policy file in session state.
      if (isPolicyFile(filePath)) return

      // Record the edit into session state for cross-edit rules. Best-effort:
      // a flaky session record must never affect the agent.
      try {
        const diff = synthesizeDiff(filePath, args)
        await $`hector session record --dir ${projectRoot} --file ${filePath} --diff ${diff}`
          .quiet()
          .nothrow()
      } catch {
        // intentional: session recording is best-effort.
      }
    },

    event: async ({ event }) => {
      // Cross-version compatibility: the `event` object's discriminant is the
      // `type` field. We only react to two values; everything else is ignored.
      const type = (event as { type?: string }).type
      if (type === undefined) return

      if (type === "session.created") {
        if (existsSync(sessionStatePath)) {
          try {
            rmSync(sessionStatePath, { force: true })
          } catch {
            // intentional: stale-state cleanup is best-effort.
          }
        }
        return
      }

      if (type === "session.idle") {
        if (!existsSync(sessionStatePath)) return

        const result =
          await $`hector check --session --config ${configPath} --format json`
            .quiet()
            .nothrow()

        if (result.exitCode === 2) {
          const verdict = result.stdout.toString().trim() || "session rule violation"
          // session.idle fires after the agent's response — we can't
          // retroactively block the turn. Surface to stderr; OpenCode renders
          // this to the user so they see what to fix next iteration.
          console.error(`hector: session check blocked:\n${verdict}`)
          throw new Error(`hector session check blocked:\n${verdict}`)
        }
        if (result.exitCode !== 0) {
          const stderr = result.stderr.toString().trim()
          console.error(
            `hector: internal error during session check (exit ${result.exitCode})${stderr ? `: ${stderr}` : ""}`,
          )
        }
      }
    },
  }
}

/**
 * Build a synthetic unified diff for an Edit/Write tool invocation.
 *
 * The OpenCode tool events don't include a real diff; we fake one from
 * the (oldString, newString) pair so `hector session record` can ingest
 * it. Two correctness concerns:
 *
 * 1. **Hunk-header counts (P1-8).** A literal `@@ -1 +1 @@` is wrong as
 *    soon as either side has more than one line — hector's diff parser
 *    uses the header's `new_start` to number added lines, so wrong
 *    counts produce wrong line numbers in downstream violations. Count
 *    lines on each side and emit `1,N` form whenever N > 1.
 *
 * 2. **Injection scrub (P1-9).** `oldString`/`newString` are arbitrary
 *    user content. Without escaping, a `newString` containing
 *    `\n+++ b/SECRET\n` becomes a real `+++ b/SECRET` header in the
 *    synthesized diff, fooling hector's parser into thinking the edit
 *    targets a different file. We prefix any line in the user-provided
 *    blocks that *looks* like a diff header with a backslash, which the
 *    parser does not recognize.
 *
 * Exported for unit testing — see `tests/synthesize_diff.test.ts`.
 */
export function synthesizeDiff(filePath: string, args: FileToolArgs): string {
  const old = getOldString(args) ?? ""
  const neu = getNewString(args) ?? args.content ?? ""

  // Neutralize attacker-controlled lines that mimic diff headers. We act
  // on the prefixed block (after `-`/`+` is applied) so a malicious OLD
  // string like `-- a/SECRET` (which would become `--- a/SECRET` after
  // the `-` prefix) is also caught.
  const scrub = (s: string) =>
    s
      .split("\n")
      .map((l) => (/^(---|\+\+\+|@@) /.test(l) ? "\\" + l : l))
      .join("\n")

  const oldLines = old === "" ? 0 : old.split("\n").length
  const newLines = neu === "" ? 0 : neu.split("\n").length
  // Diff parsers expect `0,0` (no lines on this side) or `<start>,<count>`
  // when count != 1, or bare `<start>` when count == 1.
  const hunkOld = oldLines <= 1 ? "1" : `1,${oldLines}`
  const hunkNew = newLines <= 1 ? "1" : `1,${newLines}`
  const oldBlock =
    old === "" ? "" : old.split("\n").map((l) => "-" + l).join("\n") + "\n"
  const newBlock =
    neu === "" ? "" : neu.split("\n").map((l) => "+" + l).join("\n") + "\n"

  return `--- a/${filePath}\n+++ b/${filePath}\n@@ -${hunkOld} +${hunkNew} @@\n${scrub(oldBlock)}${scrub(newBlock)}`
}

/**
 * Compute the file content that opencode is about to write, so we can
 * shadow-write it and run hector against it before opencode runs the tool.
 *
 * - `write` tool → `content` (or `newString`) is the full file body.
 * - `edit` tool → replace the first occurrence of `oldString` with
 *   `newString` in the current file content. (Opencode's Edit fails if
 *   `oldString` is not unique; we mirror "first occurrence" semantics here.)
 *
 * Returns `null` when we cannot reasonably simulate the edit — e.g. an
 * Edit whose `oldString` doesn't appear in the file. In that case the
 * tool will fail anyway; we just skip the gate rather than write garbage
 * to disk.
 */
function computeProposedContent(filePath: string, args: FileToolArgs): string | null {
  const old = getOldString(args)
  const neu = getNewString(args) ?? args.content ?? ""

  // Write tool: `content` (or `replace` with empty `find`) is the whole
  // file. Either the file is new, or it's a full overwrite.
  if (old === undefined || old === "") {
    return neu
  }

  // Edit tool: must read current content and splice in the replacement.
  if (!existsSync(filePath)) return null
  const current = readFileSync(filePath, "utf8")
  if (args.replaceAll) {
    if (!current.includes(old)) return null
    return current.split(old).join(neu)
  }
  const idx = current.indexOf(old)
  if (idx === -1) return null
  return current.slice(0, idx) + neu + current.slice(idx + old.length)
}

/**
 * Restore a file to its pre-check state. `original === null` means the
 * file did not exist before, so delete it. Failures are swallowed —
 * the check has already happened and the agent will get whatever
 * verdict it produced; a failed restore is reported via stderr.
 */
function restoreFile(filePath: string, original: string | null): void {
  try {
    if (original === null) {
      rmSync(filePath, { force: true })
    } else {
      writeFileSync(filePath, original)
    }
  } catch (err) {
    console.error(
      `hector: failed to restore ${filePath} after check: ${(err as Error).message}`,
    )
  }
}

export default HectorPlugin
