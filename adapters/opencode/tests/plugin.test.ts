import { test, expect, beforeAll, afterAll, beforeEach } from "bun:test"
import { mkdtempSync, writeFileSync, readFileSync, rmSync, existsSync, mkdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { $ } from "bun"
import HectorPlugin from "../src/index.ts"

// End-to-end test of the OpenCode adapter plugin.
// Drives the plugin hooks directly with synthetic OpenCode-shaped input,
// against a real `.hector.yml` and the real `hector` binary on PATH.
//
// Requirements:
//   - `hector` binary on PATH (CI prepends target/release before running).

let project: string

const HECTOR_YML = `schema_version: 2
rules:
  no-debug:
    description: "no DEBUG markers in source"
    engine: script
    scope: ["*.txt"]
    severity: error
    script: "grep -nE 'DEBUG' {file} && exit 1 || exit 0"
`

function fakeCtx(root: string) {
  // Cast through unknown — PluginInput has fields the plugin doesn't read
  // (`client`, `project`, `serverUrl`, `experimental_workspace`). The plugin
  // only touches `$`, `directory`, and `worktree`.
  return {
    $,
    directory: root,
    worktree: root,
  } as unknown as Parameters<typeof HectorPlugin>[0]
}

beforeAll(async () => {
  project = mkdtempSync(join(tmpdir(), "hector-opencode-"))
  writeFileSync(join(project, ".hector.yml"), HECTOR_YML)
  await $`hector trust --config ${join(project, ".hector.yml")}`.quiet()
})

afterAll(() => {
  rmSync(project, { recursive: true, force: true })
})

beforeEach(() => {
  // Reset session state between tests so the session-rule cases are isolated.
  rmSync(join(project, ".hector", "session.json"), { force: true })
})

test("hooks no-op when .hector.yml is absent at load time", async () => {
  // Hooks are always registered so that a project that becomes a hector
  // project mid-session starts gating without an opencode restart. When
  // .hector.yml is missing, every invocation short-circuits silently.
  const empty = mkdtempSync(join(tmpdir(), "hector-opencode-empty-"))
  try {
    const hooks = await HectorPlugin(fakeCtx(empty))
    expect(hooks["tool.execute.before"]).toBeDefined()
    const file = join(empty, "anything.txt")
    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "this has DEBUG\n" } },
      ),
    ).resolves.toBeUndefined()
    expect(existsSync(file)).toBe(false) // shadow-write never happened
  } finally {
    rmSync(empty, { recursive: true, force: true })
  }
})

test("gate activates when .hector.yml is created after plugin load", async () => {
  // Regression test for the silent-disable bug: opencode loads plugins once
  // at startup. If `.hector.yml` doesn't exist yet, the original plugin
  // returned `{}` and the gate was dead for the rest of the session. The
  // existsSync check now runs per-invocation, so late-init `hector init`
  // starts gating immediately.
  const root = mkdtempSync(join(tmpdir(), "hector-opencode-late-"))
  try {
    const hooks = await HectorPlugin(fakeCtx(root))
    const file = join(root, "dirty.txt")

    // Sanity: no gating yet.
    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "this has DEBUG\n" } },
      ),
    ).resolves.toBeUndefined()

    // Now create + trust the config and re-invoke the SAME hook closure.
    writeFileSync(join(root, ".hector.yml"), HECTOR_YML)
    await $`hector trust --config ${join(root, ".hector.yml")}`.quiet()

    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "this has DEBUG\n" } },
      ),
    ).rejects.toThrow(/hector blocked this edit/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("module exposes both default and named HectorPlugin exports", async () => {
  // The opencode plugin docs consistently show named exports
  // (`export const MyPlugin = ...`). The published Claude Code adapter and
  // our own tests use the default import. Keep both alive so neither
  // loader pattern silently no-ops.
  const mod = await import("../src/index.ts")
  expect(typeof mod.default).toBe("function")
  expect(typeof mod.HectorPlugin).toBe("function")
  expect(mod.default).toBe(mod.HectorPlugin)
})

test("before-hook on clean Write content passes", async () => {
  const file = join(project, "clean-write.txt")
  rmSync(file, { force: true })
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: file, content: "ok\n" } },
    ),
  ).resolves.toBeUndefined()
  // Shadow-write must be undone on the pass path so opencode's real
  // write is what lands on disk.
  expect(existsSync(file)).toBe(false)
})

test("before-hook on Write with DEBUG blocks and leaves no file behind", async () => {
  const file = join(project, "dirty-write.txt")
  rmSync(file, { force: true })
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: file, content: "this has DEBUG\n" } },
    ),
  ).rejects.toThrow(/hector blocked this edit/)
  expect(existsSync(file)).toBe(false)
})

test("before-hook on Edit that would introduce DEBUG blocks; file is unchanged", async () => {
  const file = join(project, "edit-introduce-debug.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await HectorPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, oldString: "world", newString: "DEBUG" } },
    ),
  ).rejects.toThrow(/hector blocked this edit/)

  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook handles opencode's native find/replace arg shape", async () => {
  // Regression: opencode's edit tool ships `find` / `replace` (and
  // `replaceAll`), not `oldString` / `newString`. The plugin was silently
  // falling into the Write branch with empty content and never seeing the
  // proposed content.
  const file = join(project, "edit-find-replace.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await HectorPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, find: "world", replace: "DEBUG" } },
    ),
  ).rejects.toThrow(/hector blocked this edit/)

  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook honours replaceAll", async () => {
  const file = join(project, "edit-replace-all.txt")
  writeFileSync(file, "clean clean clean\n")
  const hooks = await HectorPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, find: "clean", replace: "DEBUG", replaceAll: true } },
    ),
  ).rejects.toThrow(/hector blocked this edit/)

  expect(readFileSync(file, "utf8")).toBe("clean clean clean\n")
})

test("before-hook on clean Edit passes and leaves file unchanged (opencode writes next)", async () => {
  const file = join(project, "edit-clean.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await HectorPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, oldString: "world", newString: "there" } },
    ),
  ).resolves.toBeUndefined()

  // The shadow content must be restored so opencode's own write is the
  // canonical one. We only assert pre-state here; opencode does the real
  // write after the before-hook returns.
  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook skips gate when Edit's oldString is not in the file", async () => {
  // If we can't simulate the edit, we can't produce a faithful proposed
  // content — and opencode's Edit will fail anyway. Skip the gate rather
  // than write garbage.
  const file = join(project, "edit-no-match.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, oldString: "nonexistent", newString: "DEBUG" } },
    ),
  ).resolves.toBeUndefined()
  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook ignores non-gated tools", async () => {
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "read", sessionID: "s", callID: "c" },
      { args: { filePath: "anything" } },
    ),
  ).resolves.toBeUndefined()
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "bash", sessionID: "s", callID: "c" },
      { args: { command: "ls" } },
    ),
  ).resolves.toBeUndefined()
})

test("before-hook no-ops when filePath is missing", async () => {
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: {} },
    ),
  ).resolves.toBeUndefined()
})

test("event session.created clears stale session.json", async () => {
  mkdirSync(join(project, ".hector"), { recursive: true })
  writeFileSync(
    join(project, ".hector", "session.json"),
    JSON.stringify({ session_id: "stale", started_at: "t", edits: [] }),
  )
  expect(existsSync(join(project, ".hector", "session.json"))).toBe(true)

  const hooks = await HectorPlugin(fakeCtx(project))
  await hooks.event!({ event: { type: "session.created" } as never })

  expect(existsSync(join(project, ".hector", "session.json"))).toBe(false)
})

test("event session.idle no-ops without session.json", async () => {
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks.event!({ event: { type: "session.idle" } as never }),
  ).resolves.toBeUndefined()
})

test("event ignores unrelated event types", async () => {
  const hooks = await HectorPlugin(fakeCtx(project))
  // No throw; the handler just returns.
  await expect(
    hooks.event!({ event: { type: "message.updated" } as never }),
  ).resolves.toBeUndefined()
  await expect(
    hooks.event!({ event: { type: "permission.asked" } as never }),
  ).resolves.toBeUndefined()
})

test("before-hook skips self-check of .hector.yml (R3)", async () => {
  // R3: editing the policy file itself used to invoke hector check on
  // a mid-edit file whose on-disk sha no longer matched `trust:`, which
  // failed the trust gate (exit 1) and surfaced a confusing "internal
  // error" to the user. The plugin must short-circuit by basename
  // before any hector invocation runs.
  //
  // To prove no hector invocation ran, we deliberately break the trust
  // hash so any `hector check` would log an "internal error" line to
  // console.error. A clean run means the basename short-circuit fired.
  const root = mkdtempSync(join(tmpdir(), "hector-opencode-policy-"))
  const errs: string[] = []
  const origErr = console.error
  console.error = (msg: unknown) => {
    errs.push(String(msg))
  }
  try {
    writeFileSync(join(root, ".hector.yml"), HECTOR_YML)
    await $`hector trust --config ${join(root, ".hector.yml")}`.quiet()
    const current = readFileSync(join(root, ".hector.yml"), "utf8")
    writeFileSync(
      join(root, ".hector.yml"),
      current.replace(/sha256:[0-9a-f]+/, "sha256:0".repeat(64)),
    )

    const hooks = await HectorPlugin(fakeCtx(root))
    const file = join(root, ".hector.yml")
    const beforeBytes = readFileSync(file, "utf8")

    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "anything\n" } },
      ),
    ).resolves.toBeUndefined()

    // No hector invocation: no "internal error" log, no trust-verify log.
    expect(errs.join("\n")).not.toContain("internal error")
    expect(errs.join("\n")).not.toContain("trust verify")
    // File untouched (shadow-write never happened).
    expect(readFileSync(file, "utf8")).toBe(beforeBytes)
  } finally {
    console.error = origErr
    rmSync(root, { recursive: true, force: true })
  }
})

test("before-hook skips self-check of .bully.yml (R3)", async () => {
  // Same R3 short-circuit, applied to the migration-source filename.
  // The fixture project has no .bully.yml on disk; the plugin must
  // recognize the basename and exit before attempting any shadow-write.
  const file = join(project, ".bully.yml")
  rmSync(file, { force: true })
  const hooks = await HectorPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: file, content: "anything\n" } },
    ),
  ).resolves.toBeUndefined()

  // The plugin returned before shadow-writing the proposed content.
  expect(existsSync(file)).toBe(false)
})

test("before-hook skips self-check of bare relative .hector.yml (R3)", async () => {
  // Basename match must work even when filePath is a bare filename
  // (no directory component).
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: ".hector.yml", content: "anything\n" } },
    ),
  ).resolves.toBeUndefined()
})

test("tool.execute.after records edit to session.json", async () => {
  const file = join(project, "tracked.txt")
  writeFileSync(file, "ok\n")
  const hooks = await HectorPlugin(fakeCtx(project))
  await hooks["tool.execute.after"]!(
    {
      tool: "write",
      sessionID: "s",
      callID: "c",
      args: { filePath: file, content: "ok\n" },
    },
    { title: "", output: "", metadata: {} },
  )

  const stateFile = join(project, ".hector", "session.json")
  expect(existsSync(stateFile)).toBe(true)
  const body = await Bun.file(stateFile).text()
  expect(body).toContain("tracked.txt")
})
