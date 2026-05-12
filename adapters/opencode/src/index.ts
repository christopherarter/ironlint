import type { Plugin } from "@opencode-ai/plugin"
import { existsSync, rmSync } from "node:fs"
import { join } from "node:path"

// OpenCode tools we gate. `apply_patch` is intentionally not gated at 0.1d —
// its multi-file patch format would need per-file extraction; see
// docs/adapters/opencode.md for the known-gap note.
const GATED_TOOLS = new Set(["edit", "write"])

type FileToolArgs = {
  filePath?: string
  oldString?: string
  newString?: string
  content?: string
}

/**
 * Hector OpenCode plugin.
 *
 * Mirrors the Claude Code adapter:
 *   - `tool.execute.after` on `edit`/`write` → `hector check --file <path>`,
 *      with the same exit-code contract (0 = pass/warn, 2 = block).
 *   - `event` filtering on `session.created` → clear stale `.hector/session.json`.
 *   - `event` filtering on `session.idle` → `hector check --session`.
 *
 * Hector itself is invoked as a child process via Bun's `$` API. The
 * plugin contains no rule logic — it's purely a translation layer between
 * OpenCode's lifecycle and the `hector` CLI.
 */
const HectorPlugin: Plugin = async ({ $, directory, worktree }) => {
  const projectRoot = worktree || directory
  const configPath = join(projectRoot, ".hector.yml")
  const sessionStatePath = join(projectRoot, ".hector", "session.json")

  // If the project isn't a hector project, register no hooks. Installing the
  // plugin in a non-hector project is a free, fast no-op.
  if (!existsSync(configPath)) {
    return {}
  }

  return {
    "tool.execute.after": async (input) => {
      if (!GATED_TOOLS.has(input.tool)) return

      const args = input.args as FileToolArgs | undefined
      const filePath = args?.filePath
      if (!filePath) return

      // 1. Record the edit into session state. Non-fatal: a flaky session
      //    record must never block the agent. We swallow all errors here.
      try {
        const diff = synthesizeDiff(filePath, args)
        await $`hector session record --dir ${projectRoot} --file ${filePath} --diff ${diff}`
          .quiet()
          .nothrow()
      } catch {
        // intentional: session recording is best-effort.
      }

      // 2. Gate the edit. Exit code contract (commands/check.rs):
      //      0 → pass or warn  (allow)
      //      2 → block         (reject — throw to surface to the agent)
      //      1 → internal      (log to stderr, allow — agent shouldn't be
      //                         blocked by an unrelated hector failure)
      const result =
        await $`hector check --file ${filePath} --config ${configPath} --format json`
          .quiet()
          .nothrow()

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

function synthesizeDiff(filePath: string, args: FileToolArgs): string {
  const old = args.oldString ?? ""
  const neu = args.newString ?? args.content ?? ""
  return `--- a/${filePath}\n+++ b/${filePath}\n@@ -1 +1 @@\n-${old}\n+${neu}`
}

export default HectorPlugin
