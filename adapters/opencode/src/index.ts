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
 *
 * Hector itself is invoked as a child process via Bun's `$` API. The
 * plugin contains no rule logic — it's purely a translation layer between
 * OpenCode's lifecycle and the `hector` CLI.
 */
export const HectorPlugin: Plugin = async ({ $, directory, worktree }) => {
  const projectRoot = worktree || directory
  const configPath = join(projectRoot, ".hector.yml")

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
      //   3 → engine internal error (missing API key, spawn failure, etc.)
      //       fail-open by default; HECTOR_FAIL_CLOSED_ON_INTERNAL=1 to block
      //   1 → config/load error (log to stderr, allow)
      if (result.exitCode === 2) {
        const verdict = result.stdout.toString().trim() || "rule violation"
        throw new Error(`hector blocked this edit:\n${verdict}`)
      }
      if (result.exitCode === 3) {
        // B7: engine runtime error — the gate is broken, not the policy.
        const stderr = result.stderr.toString().trim()
        if (process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"] === "1") {
          console.error(
            `hector: internal error — failing closed (HECTOR_FAIL_CLOSED_ON_INTERNAL=1)${stderr ? `: ${stderr}` : ""}`,
          )
          throw new Error(`hector: internal error during check — failing closed`)
        }
        console.error(
          `hector: internal error checking ${filePath} — allowing edit; see .hector/log.jsonl${stderr ? `: ${stderr}` : ""}`,
        )
      } else if (result.exitCode !== 0) {
        const stderr = result.stderr.toString().trim()
        console.error(
          `hector: internal error checking ${filePath} (exit ${result.exitCode})${stderr ? `: ${stderr}` : ""}`,
        )
      }
    },
  }
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
