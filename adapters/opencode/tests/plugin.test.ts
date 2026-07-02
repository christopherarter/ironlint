import { test, expect, beforeAll, afterAll } from "bun:test"
import { mkdtempSync, writeFileSync, readFileSync, rmSync, existsSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { $ } from "bun"
import IronLintPlugin from "../src/index.ts"

// End-to-end test of the OpenCode adapter plugin.
// Drives the plugin hooks directly with synthetic OpenCode-shaped input,
// against a real `.ironlint.yml` and the real `ironlint` binary on PATH.
//
// Requirements:
//   - `ironlint` binary on PATH (CI prepends target/release before running).

let project: string

const IRONLINT_YML = `checks:
  no-debug:
    files: ["*.txt"]
    run: "! grep -nE 'DEBUG'"
`

function fakeCtx(root: string) {
  // Cast through unknown — PluginInput has fields the plugin doesn't read
  // (`client`, `project`, `serverUrl`, `experimental_workspace`). The plugin
  // only touches `$`, `directory`, and `worktree`.
  return {
    $,
    directory: root,
    worktree: root,
  } as unknown as Parameters<typeof IronLintPlugin>[0]
}

beforeAll(async () => {
  project = mkdtempSync(join(tmpdir(), "ironlint-opencode-"))
  writeFileSync(join(project, ".ironlint.yml"), IRONLINT_YML)
  await $`ironlint trust --config ${join(project, ".ironlint.yml")}`.quiet()
})

afterAll(() => {
  rmSync(project, { recursive: true, force: true })
})

test("hooks no-op when .ironlint.yml is absent at load time", async () => {
  // Hooks are always registered so that a project that becomes an ironlint
  // project mid-session starts gating without an opencode restart. When
  // .ironlint.yml is missing, every invocation short-circuits silently.
  const empty = mkdtempSync(join(tmpdir(), "ironlint-opencode-empty-"))
  try {
    const hooks = await IronLintPlugin(fakeCtx(empty))
    expect(hooks["tool.execute.before"]).toBeDefined()
    const file = join(empty, "anything.txt")
    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "this has DEBUG\n" } },
      ),
    ).resolves.toBeUndefined()
    expect(existsSync(file)).toBe(false) // the adapter never writes the real file
  } finally {
    rmSync(empty, { recursive: true, force: true })
  }
})

test("gate activates when .ironlint.yml is created after plugin load", async () => {
  // Regression test for the silent-disable bug: opencode loads plugins once
  // at startup. If `.ironlint.yml` doesn't exist yet, the original plugin
  // returned `{}` and the gate was dead for the rest of the session. The
  // existsSync check now runs per-invocation, so late-init `ironlint init`
  // starts gating immediately.
  const root = mkdtempSync(join(tmpdir(), "ironlint-opencode-late-"))
  try {
    const hooks = await IronLintPlugin(fakeCtx(root))
    const file = join(root, "dirty.txt")

    // Sanity: no gating yet.
    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "this has DEBUG\n" } },
      ),
    ).resolves.toBeUndefined()

    // Now create + trust the config and re-invoke the SAME hook closure.
    writeFileSync(join(root, ".ironlint.yml"), IRONLINT_YML)
    await $`ironlint trust --config ${join(root, ".ironlint.yml")}`.quiet()

    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "this has DEBUG\n" } },
      ),
    ).rejects.toThrow(/ironlint blocked this edit/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("module exposes both default and named IronLintPlugin exports", async () => {
  // The opencode plugin docs consistently show named exports
  // (`export const MyPlugin = ...`). The published Claude Code adapter and
  // our own tests use the default import. Keep both alive so neither
  // loader pattern silently no-ops.
  const mod = await import("../src/index.ts")
  expect(typeof mod.default).toBe("function")
  expect(typeof mod.IronLintPlugin).toBe("function")
  expect(mod.default).toBe(mod.IronLintPlugin)
})

test("before-hook on clean Write content passes", async () => {
  const file = join(project, "clean-write.txt")
  rmSync(file, { force: true })
  const hooks = await IronLintPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: file, content: "ok\n" } },
    ),
  ).resolves.toBeUndefined()
  // The plugin never writes the real file — it only gates the proposed
  // content via stdin. A nonexistent target stays nonexistent; opencode
  // performs the real write after the before-hook returns.
  expect(existsSync(file)).toBe(false)
})

test("before-hook on Write with DEBUG blocks and leaves no file behind", async () => {
  const file = join(project, "dirty-write.txt")
  rmSync(file, { force: true })
  const hooks = await IronLintPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: file, content: "this has DEBUG\n" } },
    ),
  ).rejects.toThrow(/ironlint blocked this edit/)
  expect(existsSync(file)).toBe(false)
})

test("before-hook on Edit that would introduce DEBUG blocks; file is unchanged", async () => {
  const file = join(project, "edit-introduce-debug.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await IronLintPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, oldString: "world", newString: "DEBUG" } },
    ),
  ).rejects.toThrow(/ironlint blocked this edit/)

  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook handles opencode's native find/replace arg shape", async () => {
  // Regression: opencode's edit tool ships `find` / `replace` (and
  // `replaceAll`), not `oldString` / `newString`. The plugin was silently
  // falling into the Write branch with empty content and never seeing the
  // proposed content.
  const file = join(project, "edit-find-replace.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await IronLintPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, find: "world", replace: "DEBUG" } },
    ),
  ).rejects.toThrow(/ironlint blocked this edit/)

  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook honours replaceAll", async () => {
  const file = join(project, "edit-replace-all.txt")
  writeFileSync(file, "clean clean clean\n")
  const hooks = await IronLintPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, find: "clean", replace: "DEBUG", replaceAll: true } },
    ),
  ).rejects.toThrow(/ironlint blocked this edit/)

  expect(readFileSync(file, "utf8")).toBe("clean clean clean\n")
})

test("before-hook on clean Edit passes and leaves file unchanged (opencode writes next)", async () => {
  const file = join(project, "edit-clean.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await IronLintPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, oldString: "world", newString: "there" } },
    ),
  ).resolves.toBeUndefined()

  // The plugin never writes the real file — it only reads the current
  // content to simulate the edit for the gate. opencode performs the
  // actual write after the before-hook returns.
  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook skips gate when Edit's oldString is not in the file", async () => {
  // If we can't simulate the edit, we can't produce a faithful proposed
  // content — and opencode's Edit will fail anyway. Skip the gate rather
  // than write garbage.
  const file = join(project, "edit-no-match.txt")
  writeFileSync(file, "hello world\n")
  const hooks = await IronLintPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: { filePath: file, oldString: "nonexistent", newString: "DEBUG" } },
    ),
  ).resolves.toBeUndefined()
  expect(readFileSync(file, "utf8")).toBe("hello world\n")
})

test("before-hook ignores non-gated tools", async () => {
  const hooks = await IronLintPlugin(fakeCtx(project))
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
  const hooks = await IronLintPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "edit", sessionID: "s", callID: "c" },
      { args: {} },
    ),
  ).resolves.toBeUndefined()
})

test("before-hook skips self-check of .ironlint.yml (R3)", async () => {
  // R3: editing the policy file itself used to invoke ironlint check on
  // a mid-edit file whose on-disk sha no longer matched `trust:`, which
  // failed the trust gate (exit 1) and surfaced a confusing "internal
  // error" to the user. The plugin must short-circuit by basename
  // before any ironlint invocation runs.
  //
  // To prove no ironlint invocation ran, we deliberately break the trust
  // hash so any `ironlint check` would log an "internal error" line to
  // console.error. A clean run means the basename short-circuit fired.
  const root = mkdtempSync(join(tmpdir(), "ironlint-opencode-policy-"))
  const errs: string[] = []
  const origErr = console.error
  console.error = (msg: unknown) => {
    errs.push(String(msg))
  }
  try {
    writeFileSync(join(root, ".ironlint.yml"), IRONLINT_YML)
    await $`ironlint trust --config ${join(root, ".ironlint.yml")}`.quiet()
    const current = readFileSync(join(root, ".ironlint.yml"), "utf8")
    writeFileSync(
      join(root, ".ironlint.yml"),
      current.replace(/sha256:[0-9a-f]+/, "sha256:0".repeat(64)),
    )

    const hooks = await IronLintPlugin(fakeCtx(root))
    const file = join(root, ".ironlint.yml")
    const beforeBytes = readFileSync(file, "utf8")

    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "anything\n" } },
      ),
    ).resolves.toBeUndefined()

    // No ironlint invocation: no "internal error" log, no trust-verify log.
    expect(errs.join("\n")).not.toContain("internal error")
    expect(errs.join("\n")).not.toContain("trust verify")
    // File untouched — the plugin never writes the real file.
    expect(readFileSync(file, "utf8")).toBe(beforeBytes)
  } finally {
    console.error = origErr
    rmSync(root, { recursive: true, force: true })
  }
})

test("before-hook skips self-check of .bully.yml (R3)", async () => {
  // Same R3 short-circuit, applied to the migration-source filename.
  // The fixture project has no .bully.yml on disk; the plugin must
  // recognize the basename and exit before invoking ironlint at all.
  const file = join(project, ".bully.yml")
  rmSync(file, { force: true })
  const hooks = await IronLintPlugin(fakeCtx(project))

  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: file, content: "anything\n" } },
    ),
  ).resolves.toBeUndefined()

  // The plugin returned before ever touching the real file.
  expect(existsSync(file)).toBe(false)
})

test("real file is never written: a check inspecting $IRONLINT_FILE mid-check sees the pre-edit original", async () => {
  // This is the load-bearing regression test for the shadow-write bug
  // (E3): the old adapter wrote the PROPOSED content to the real file
  // path, ran `ironlint check --file <path>` (no `--content -`, so the
  // CLI read the file back off disk), and restored the original in a
  // `finally`. A check's `run` script that reads `$IRONLINT_FILE` (the
  // real path) *during* the check would therefore observe the flashed
  // proposed content — exactly what a file watcher (HMR, tsc --watch)
  // would see too.
  //
  // Fixture: a check whose `run` asserts $IRONLINT_FILE's on-disk content
  // still equals the pre-edit original. Under the old shadow-write code
  // this assertion fails (blocks) because the real file briefly held the
  // proposed content. Under the fixed stdin-piped code the real file is
  // never touched, so the assertion holds and the check passes.
  const root = mkdtempSync(join(tmpdir(), "ironlint-opencode-noshadow-"))
  try {
    const yml = [
      "checks:",
      "  no-shadow-write:",
      '    files: ["*.txt"]',
      "    run: 'test \"$(cat \"$IRONLINT_FILE\")\" = \"original content\"'",
      "",
    ].join("\n")
    writeFileSync(join(root, ".ironlint.yml"), yml)
    await $`ironlint trust --config ${join(root, ".ironlint.yml")}`.quiet()

    const file = join(root, "target.txt")
    writeFileSync(file, "original content")

    const hooks = await IronLintPlugin(fakeCtx(root))
    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "proposed content" } },
      ),
    ).resolves.toBeUndefined() // must PASS: the real file was never overwritten mid-check

    // The real file must be byte-identical to its pre-edit state — the
    // adapter must never have written to it, not even transiently.
    expect(readFileSync(file, "utf8")).toBe("original content")
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("a passing check leaves a non-UTF8 file byte-identical on disk", async () => {
  // Regression for the "permanent corruption even on PASS" half of E3:
  // the old adapter's shadow-write/restore read the real file back with
  // `readFileSync(file, "utf8")`. Invalid UTF-8 byte sequences decode
  // lossily into U+FFFD replacement characters, and the `finally` restore
  // re-encoded that lossy string over the original bytes — corrupting a
  // non-UTF8 file even when the check passed cleanly. The fixed adapter
  // never reads or writes the real file, so this can no longer happen.
  const root = mkdtempSync(join(tmpdir(), "ironlint-opencode-nonutf8-"))
  try {
    writeFileSync(join(root, ".ironlint.yml"), IRONLINT_YML)
    await $`ironlint trust --config ${join(root, ".ironlint.yml")}`.quiet()

    const file = join(root, "binary.txt")
    // Invalid UTF-8: a lone continuation byte (0xff) and an overlong/
    // malformed sequence — mangled by any readFileSync(file, "utf8").
    const originalBytes = Buffer.from([0x68, 0x69, 0xff, 0xfe, 0x00, 0x9c])
    writeFileSync(file, originalBytes)

    const hooks = await IronLintPlugin(fakeCtx(root))
    await expect(
      hooks["tool.execute.before"]!(
        { tool: "write", sessionID: "s", callID: "c" },
        { args: { filePath: file, content: "clean\n" } },
      ),
    ).resolves.toBeUndefined() // passes: no DEBUG in the proposed content

    // The plugin only gates; opencode itself performs the real write
    // after the hook returns. The real file must be untouched, byte for
    // byte — no lossy-decode-and-rewrite round-trip.
    expect(Buffer.compare(readFileSync(file), originalBytes)).toBe(0)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("before-hook skips self-check of bare relative .ironlint.yml (R3)", async () => {
  // Basename match must work even when filePath is a bare filename
  // (no directory component).
  const hooks = await IronLintPlugin(fakeCtx(project))
  await expect(
    hooks["tool.execute.before"]!(
      { tool: "write", sessionID: "s", callID: "c" },
      { args: { filePath: ".ironlint.yml", content: "anything\n" } },
    ),
  ).resolves.toBeUndefined()
})
