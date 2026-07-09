import type { Plugin } from "@opencode-ai/plugin"
import { existsSync, readFileSync } from "node:fs"
import { basename, isAbsolute, join, sep } from "node:path"

// OpenCode tools we gate. `apply_patch` is intentionally not gated at 0.1d
// (P2-14, deferred) — the opencode plugin SDK does not currently surface
// an `apply_patch` tool through `tool.execute.after`, and its multi-file
// patch format would need per-file extraction (split on `+++ b/<path>`
// boundaries, reissue `ironlint check --file` per file). See
// docs/adapters/opencode.md → "What it does NOT do" for the known-gap
// note. Tracked until the apply_patch tool is wired through the adapter.
// `bash` is gated by the bash-gate branch below, which shells out to
// `ironlint gate-bash` (the shared Rust matcher) — see
// docs/superpowers/specs/2026-07-06-bash-gate-self-trust-prevention-design.md.
const GATED_TOOLS = new Set(["edit", "write", "bash"])

// R3: filenames ironlint recognizes as policy files. Edits to these files
// must short-circuit both adapter hooks — running `ironlint check` against
// a mid-edit policy file fails the trust gate (sha mismatch) and
// surfaces a confusing "internal error" to the user.
const POLICY_FILES = new Set([".ironlint.yml", ".bully.yml"])

/** R3: config by basename + `.ironlint/scripts/` path-anchored to `projectRoot`. */
export function isPolicyFile(filePath: string, projectRoot: string): boolean {
  if (POLICY_FILES.has(basename(filePath))) return true
  const abs = isAbsolute(filePath) ? filePath : join(projectRoot, filePath)
  const scriptsDir = join(projectRoot, ".ironlint", "scripts") + sep
  return abs === scriptsDir.slice(0, -1) || abs.startsWith(scriptsDir)
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
 * IronLint OpenCode plugin.
 *
 *   - `tool.execute.before` on `edit`/`write` → compute the proposed
 *      content and pipe it to `ironlint check --file <path> --content -`
 *      on stdin. The real file at `filePath` is never written or read
 *      back — the check only ever sees the proposed bytes. Throw on block
 *      so OpenCode never executes the tool (exit-code contract: 0 =
 *      pass/warn, 2 = block).
 *
 * IronLint itself is invoked as a child process via the async `Bun.spawn`
 * (awaited inside the async before-hook). The plugin contains no rule
 * logic — it's purely a translation layer between OpenCode's lifecycle
 * and the `ironlint` CLI. Spawn failures (missing binary) and signal death
 * are normalized into the internal-error tier (fail-open by default,
 * fail-closed under `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`) — never a block.
 */
export const IronLintPlugin: Plugin = async ({ directory, worktree }) => {
  const projectRoot = worktree || directory
  const configPath = join(projectRoot, ".ironlint.yml")

  return {
    "tool.execute.before": async (input, output) => {
      if (!GATED_TOOLS.has(input.tool)) return

      // Bash branch: the bash-gate. Runs BEFORE the config-existence check —
      // the bash-gate must fire even with no .ironlint.yml, since that's
      // exactly when an agent is most motivated to run `ironlint trust`.
      // Decides whether the command would let the agent free itself
      // (`ironlint trust`, or a Bash write to `.ironlint.yml` /
      // `.ironlint/scripts/`). The deny logic lives in `ironlint gate-bash` —
      // the single source shared across every adapter. Block contract =
      // throw (mirrors the existing exit-2 write/edit path). Spawn via
      // `Bun.spawn` (async, like the check path — a sync spawn blocks
      // opencode's event loop for the full duration).
      if (input.tool === "bash") {
        const args = (output.args ?? {}) as { command?: string }
        const command = typeof args.command === "string" ? args.command : ""
        // Substring pre-filter: ordinary commands (ls, git, cargo) never
        // mention ironlint or .ironlint, so skip the spawn entirely.
        if (!command.includes("ironlint") && !command.includes(".ironlint")) return
        let gateExit: number | null = null
        let gateStdout = ""
        try {
          const proc = Bun.spawn(["ironlint", "gate-bash"], {
            stdin: new TextEncoder().encode(command),
            stdout: "pipe",
            stderr: "pipe",
            env: process.env,
          })
          ;[gateStdout, , gateExit] = await Promise.all([
            new Response(proc.stdout).text(),
            new Response(proc.stderr).text(),
            proc.exited,
          ])
        } catch {
          // Spawn failure (missing binary → ENOENT): fail CLOSED.
          throw new Error("ironlint: bash-gate failed — fail-closed")
        }
        if (gateExit === null || gateExit >= 128) {
          // Signal death → fail closed.
          throw new Error("ironlint: bash-gate killed by signal — fail-closed")
        }
        if (gateExit === 0) return // allow
        if (gateExit === 2) {
          throw new Error(gateStdout || "ironlint blocked this bash command")
        }
        // Any other exit → fail closed.
        throw new Error(`ironlint: bash-gate unexpected exit ${gateExit} — fail-closed`)
      }

      // Late existence check: opencode may load this plugin once at startup,
      // before the project is initialized as an ironlint project. Re-check on
      // every invocation so that `ironlint init` mid-session starts gating.
      if (!existsSync(configPath)) return

      const args = (output.args ?? {}) as FileToolArgs
      const filePath = args.filePath
      if (!filePath) return
      // R3: skip self-checks of the policy file itself.
      if (isPolicyFile(filePath, projectRoot)) return

      const proposed = computeProposedContent(filePath, args)
      if (proposed === null) return // can't simulate — skip the gate

      // Pipe the proposed content to ironlint on stdin via `--content -`.
      // The real file at `filePath` is NEVER written or read back: no
      // shadow-write, no restore. That machinery used to (a) permanently
      // corrupt non-UTF8 files via a lossy readFileSync/writeFileSync
      // round-trip, even on a passing check; (b) leave blocked content on
      // disk if the process crashed mid-check; and (c) feed flashed
      // content to file watchers (HMR, tsc --watch). `--content -` is the
      // sanctioned ABI path for handing ironlint proposed content without
      // ever touching the real path. The content is sent as raw UTF-8
      // bytes (not spliced into a shell command string) so it travels
      // byte-for-byte.
      //
      // Spawn via the async `Bun.spawn` (not `spawnSync`): the before-hook
      // is already async, and a synchronous spawn blocks opencode's entire
      // event loop (every session, UI, timer, other hook) for the full
      // check duration — up to `timeout_secs × N checks`, or forever if
      // ironlint wedges. `env: process.env` is passed explicitly so the
      // child sees the live environment (Bun's spawn otherwise resolves
      // `ironlint` against a PATH snapshot taken at plugin load, ignoring
      // runtime PATH changes — which also made the missing-binary case
      // un-reproducible in tests).
      let exitCode: number | null = null
      let stdout = ""
      let stderr = ""
      try {
        const proc = Bun.spawn(
          [
            "ironlint",
            "check",
            "--file",
            filePath,
            "--content",
            "-",
            "--config",
            configPath,
            "--format",
            "json",
          ],
          {
            stdin: new TextEncoder().encode(proposed),
            stdout: "pipe",
            stderr: "pipe",
            env: process.env,
          },
        )
        ;[stdout, stderr, exitCode] = await Promise.all([
          new Response(proc.stdout).text(),
          new Response(proc.stderr).text(),
          proc.exited,
        ])
      } catch (err) {
        // Spawn failure (binary missing → ENOENT, EACCES, etc.): this is an
        // internal-error tier, NOT a block. A throw here used to escape the
        // async before-hook — which is exactly how this adapter signals
        // BLOCK — so a missing `ironlint` binary hard-blocked every edit.
        // `Bun.spawn` throws synchronously on ENOENT, so this try/catch is
        // load-bearing either way.
        exitCode = 3
        stderr = (err as Error).message
      }

      // Normalize signal death into the internal-error tier before the
      // branch chain. With the async API, a signal-killed CLI resolves
      // `proc.exited` to `128 + signum` (e.g. SIGKILL → 137); without this
      // normalization a signal death (OOM-kill, hook-timeout-kill) would
      // skip the `=== 3` arm and land in the generic `!== 0` log-and-allow
      // arm — silently defeating `IRONLINT_FAIL_CLOSED_ON_INTERNAL=1`
      // exactly when the engine is being killed. `null` is the defensive
      // form (the sync API reports signal death as `null`; kept for safety).
      if (exitCode === null || exitCode >= 128) {
        if (exitCode !== 3) stderr = stderr || `ironlint killed by signal (exit ${exitCode})`
        exitCode = 3
      }

      // Exit code contract (commands/check.rs):
      //   0 → pass or warn  (allow opencode to run the tool)
      //   2 → block         (throw — opencode cancels the tool call)
      //   3 → engine internal error (missing API key, spawn failure, etc.)
      //       fail-open by default; IRONLINT_FAIL_CLOSED_ON_INTERNAL=1 to block
      //   4 → untrusted config/gates (Task 3.2 / Finding C3): fail CLOSED
      //       (throw), unconditionally — unlike exit 3, there is no
      //       fail-open default here. An untrusted config must never be
      //       silently un-gated just because nobody re-ran `ironlint trust`
      //       after pulling a changed `.ironlint.yml`. Checked BEFORE the
      //       `!== 0` catch-all below so it can never fall into log-and-allow.
      //   1 → config/load error (log to stderr, allow)
      if (exitCode === 2) {
        const verdict = stdout.trim() || "rule violation"
        throw new Error(`ironlint blocked this edit:\n${verdict}`)
      }
      if (exitCode === 4) {
        throw new Error(
          "ironlint is configured here but not trusted — run 'ironlint trust' to enable checks",
        )
      }
      if (exitCode === 3) {
        // B7: engine runtime error — the gate is broken, not the policy.
        const trimmed = stderr.trim()
        if (process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"] === "1") {
          console.error(
            `ironlint: internal error — failing closed (IRONLINT_FAIL_CLOSED_ON_INTERNAL=1)${trimmed ? `: ${trimmed}` : ""}`,
          )
          throw new Error(`ironlint: internal error during check — failing closed`)
        }
        console.error(
          `ironlint: internal error checking ${filePath} — allowing edit; see .ironlint/log.jsonl${trimmed ? `: ${trimmed}` : ""}`,
        )
      } else if (exitCode !== 0) {
        const trimmed = stderr.trim()
        console.error(
          `ironlint: internal error checking ${filePath} (exit ${exitCode})${trimmed ? `: ${trimmed}` : ""}`,
        )
      }
    },
  }
}

/**
 * Compute the file content that opencode is about to write, so we can pipe
 * it to `ironlint check --content -` and gate it before opencode runs the
 * tool.
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

export default IronLintPlugin
