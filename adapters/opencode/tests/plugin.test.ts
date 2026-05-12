import { test, expect, beforeAll, afterAll, beforeEach } from "bun:test"
import { mkdtempSync, writeFileSync, rmSync, existsSync, mkdirSync } from "node:fs"
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

test("registers no hooks when .hector.yml is absent", async () => {
  const empty = mkdtempSync(join(tmpdir(), "hector-opencode-empty-"))
  try {
    const hooks = await HectorPlugin(fakeCtx(empty))
    expect(Object.keys(hooks)).toHaveLength(0)
  } finally {
    rmSync(empty, { recursive: true, force: true })
  }
})

test("tool.execute.after on clean file passes", async () => {
  const file = join(project, "clean.txt")
  writeFileSync(file, "ok\n")
  const hooks = await HectorPlugin(fakeCtx(project))
  const after = hooks["tool.execute.after"]
  expect(after).toBeDefined()
  await expect(
    after!(
      { tool: "edit", sessionID: "s", callID: "c", args: { filePath: file } },
      { title: "", output: "", metadata: {} },
    ),
  ).resolves.toBeUndefined()
})

test("tool.execute.after on dirty file blocks (throws)", async () => {
  const file = join(project, "dirty.txt")
  writeFileSync(file, "this has DEBUG in it\n")
  const hooks = await HectorPlugin(fakeCtx(project))
  const after = hooks["tool.execute.after"]
  await expect(
    after!(
      { tool: "edit", sessionID: "s", callID: "c", args: { filePath: file } },
      { title: "", output: "", metadata: {} },
    ),
  ).rejects.toThrow(/hector blocked this edit/)
})

test("tool.execute.after ignores non-gated tools", async () => {
  // Even with DEBUG in the file, a `read` or `bash` tool call should pass
  // through — we only gate edit/write.
  const file = join(project, "dirty-but-ignored.txt")
  writeFileSync(file, "this has DEBUG too\n")
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.after"]!(
      { tool: "read", sessionID: "s", callID: "c", args: { filePath: file } },
      { title: "", output: "", metadata: {} },
    ),
  ).resolves.toBeUndefined()
  await expect(
    hooks["tool.execute.after"]!(
      { tool: "bash", sessionID: "s", callID: "c", args: { command: "ls" } },
      { title: "", output: "", metadata: {} },
    ),
  ).resolves.toBeUndefined()
})

test("tool.execute.after no-ops when filePath is missing", async () => {
  const hooks = await HectorPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.after"]!(
      { tool: "edit", sessionID: "s", callID: "c", args: {} },
      { title: "", output: "", metadata: {} },
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
