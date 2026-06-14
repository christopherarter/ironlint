import { test } from "node:test"
import assert from "node:assert/strict"
import { normalizeEdits } from "../src/index.ts"
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, readFileSync, chmodSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, delimiter } from "node:path"
import { computeProposedContent } from "../src/index.ts"
import { execFileSync } from "node:child_process"
import hectorExtension from "../src/index.ts"

// --- normalizeEdits -------------------------------------------------------

test("normalizeEdits: batch edits[] array", () => {
  const edits = normalizeEdits({
    edits: [
      { oldText: "a", newText: "x" },
      { oldText: "b", newText: "y" },
    ],
  })
  assert.deepEqual(edits, [
    { oldText: "a", newText: "x" },
    { oldText: "b", newText: "y" },
  ])
})

test("normalizeEdits: legacy top-level oldText/newText", () => {
  const edits = normalizeEdits({ oldText: "a", newText: "x" })
  assert.deepEqual(edits, [{ oldText: "a", newText: "x" }])
})

test("normalizeEdits: legacy oldText with missing newText defaults to empty", () => {
  const edits = normalizeEdits({ oldText: "a" })
  assert.deepEqual(edits, [{ oldText: "a", newText: "" }])
})

test("normalizeEdits: malformed (no edits, no oldText) returns null", () => {
  assert.equal(normalizeEdits({ content: "whatever" }), null)
})

test("normalizeEdits: edits[] with a non-string member returns null", () => {
  assert.equal(normalizeEdits({ edits: [{ oldText: "a" }] as never }), null)
})

test("normalizeEdits: empty edits[] returns null", () => {
  assert.equal(normalizeEdits({ edits: [] }), null)
})

// --- computeProposedContent -----------------------------------------------

test("computeProposedContent: write returns the full body (new file ok)", () => {
  assert.equal(
    computeProposedContent("write", "/nonexistent/new.ts", { content: "hello\n" }),
    "hello\n",
  )
})

test("computeProposedContent: write with non-string content returns null", () => {
  assert.equal(
    computeProposedContent("write", "/nonexistent/new.ts", {} as never),
    null,
  )
})

test("computeProposedContent: edit applies a single replacement", () => {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-cpc-"))
  try {
    const file = join(dir, "a.txt")
    writeFileSync(file, "hello world\n")
    assert.equal(
      computeProposedContent("edit", file, { oldText: "world", newText: "DEBUG" }),
      "hello DEBUG\n",
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("computeProposedContent: edit applies a batch in order", () => {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-cpc-"))
  try {
    const file = join(dir, "a.txt")
    writeFileSync(file, "alpha beta\n")
    assert.equal(
      computeProposedContent("edit", file, {
        edits: [
          { oldText: "alpha", newText: "x" },
          { oldText: "beta", newText: "y" },
        ],
      }),
      "x y\n",
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("computeProposedContent: edit returns null when file does not exist", () => {
  assert.equal(
    computeProposedContent("edit", "/nonexistent/missing.txt", {
      oldText: "a",
      newText: "b",
    }),
    null,
  )
})

test("computeProposedContent: edit returns null when oldText is missing from file", () => {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-cpc-"))
  try {
    const file = join(dir, "a.txt")
    writeFileSync(file, "hello world\n")
    assert.equal(
      computeProposedContent("edit", file, { oldText: "nope", newText: "x" }),
      null,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("computeProposedContent: edit returns null when oldText is non-unique", () => {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-cpc-"))
  try {
    const file = join(dir, "a.txt")
    writeFileSync(file, "a a\n")
    assert.equal(
      computeProposedContent("edit", file, { oldText: "a", newText: "x" }),
      null,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("computeProposedContent: unknown tool returns null", () => {
  assert.equal(computeProposedContent("read", "/whatever", {}), null)
})

// Drive the exported factory with a fake `pi` that records handlers, then
// invoke them with synthetic pi-shaped events against the real `hector`
// binary (PATH includes target/release).

type Handler = (event: unknown, ctx?: unknown) => unknown
function loadExtension(root: string): Record<string, Handler> {
  const handlers: Record<string, Handler> = {}
  const pi = {
    on: (ev: string, h: Handler) => {
      handlers[ev] = h
    },
    cwd: root,
  }
  // Cast through unknown — the fake only implements the surface the factory uses.
  hectorExtension(pi as unknown as Parameters<typeof hectorExtension>[0])
  return handlers
}

const HECTOR_YML = `schema_version: 2
rules:
  no-panic:
    description: "no panics in source"
    engine: ast
    language: rust
    scope: ["src/**/*.rs"]
    severity: error
    pattern: "panic!($$$)"
`

function makeProject(): string {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-proj-"))
  mkdirSync(join(dir, "src"), { recursive: true })
  writeFileSync(join(dir, ".hector.yml"), HECTOR_YML)
  execFileSync("hector", ["trust", "--config", join(dir, ".hector.yml")])
  return dir
}

test("tool_call: clean write passes (returns nothing), file never written", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "clean.rs")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn a() {}\n" } },
      {},
    )
    assert.equal(result, undefined)
    // --content - never writes to disk.
    assert.equal(existsSync(file), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: write introducing panic blocks", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "dirty.rs")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn b() { panic!(); }\n" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    assert.ok(typeof result?.reason === "string" && result.reason.length > 0)
    assert.equal(existsSync(file), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: edit introducing panic blocks; on-disk file untouched", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "edit.rs")
    writeFileSync(file, "fn a() {}\n")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "edit", input: { path: file, oldText: "fn a() {}", newText: "fn a() { panic!(); }" } },
      {},
    ) as { block?: boolean } | undefined
    assert.equal(result?.block, true)
    // --content - means the gate never writes; pi's real edit was blocked.
    assert.equal(readFileSync(file, "utf8"), "fn a() {}\n")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: multi-edit batch is simulated and blocks", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "batch.rs")
    writeFileSync(file, "fn a() {}\nfn b() {}\n")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      {
        toolName: "edit",
        input: {
          path: file,
          edits: [
            { oldText: "fn a() {}", newText: "fn a() { let _ = 1; }" },
            { oldText: "fn b() {}", newText: "fn b() { panic!(); }" },
          ],
        },
      },
      {},
    ) as { block?: boolean } | undefined
    assert.equal(result?.block, true)
    assert.equal(readFileSync(file, "utf8"), "fn a() {}\nfn b() {}\n")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: legacy top-level oldText/newText edit blocks", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "legacy.rs")
    writeFileSync(file, "fn a() {}\n")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "edit", input: { path: file, oldText: "fn a() {}", newText: "fn a() { panic!(); }" } },
      {},
    ) as { block?: boolean } | undefined
    assert.equal(result?.block, true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: edit with unmatched oldText skips the gate (no false block)", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "nomatch.rs")
    writeFileSync(file, "fn a() {}\n")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "edit", input: { path: file, oldText: "does_not_exist", newText: "fn a() { panic!(); }" } },
      {},
    )
    assert.equal(result, undefined)
    assert.equal(readFileSync(file, "utf8"), "fn a() {}\n")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: non-gated tools (read, bash) are ignored", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    assert.equal(
      handlers.tool_call!({ toolName: "read", input: { path: "anything" } }, {}),
      undefined,
    )
    assert.equal(
      handlers.tool_call!({ toolName: "bash", input: {} }, {}),
      undefined,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: missing path is a no-op", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    assert.equal(handlers.tool_call!({ toolName: "edit", input: {} }, {}), undefined)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: no-op when .hector.yml is absent", () => {
  // Spec §11: a project without a config silently no-ops (safe global install).
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-noconfig-"))
  mkdirSync(join(dir, "src"), { recursive: true })
  try {
    const file = join(dir, "src", "dirty.rs")
    const handlers = loadExtension(dir)
    assert.equal(
      handlers.tool_call!(
        { toolName: "write", input: { path: file, content: "fn b() { panic!(); }\n" } },
        {},
      ),
      undefined,
    )
    assert.equal(existsSync(file), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: gate activates after .hector.yml is created mid-session", () => {
  // Regression: the existence check runs per-invocation, so a project that
  // becomes a hector project after the extension loads starts gating with
  // no restart.
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-late-"))
  mkdirSync(join(dir, "src"), { recursive: true })
  try {
    const file = join(dir, "src", "dirty.rs")
    const handlers = loadExtension(dir)
    // No config yet -> no gating.
    assert.equal(
      handlers.tool_call!(
        { toolName: "write", input: { path: file, content: "fn b() { panic!(); }\n" } },
        {},
      ),
      undefined,
    )
    // Create + trust the config, re-invoke the SAME handler closure.
    writeFileSync(join(dir, ".hector.yml"), HECTOR_YML)
    execFileSync("hector", ["trust", "--config", join(dir, ".hector.yml")])
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn b() { panic!(); }\n" } },
      {},
    ) as { block?: boolean } | undefined
    assert.equal(result?.block, true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: .hector.yml self-edit short-circuits (R3) — no hector invocation", () => {
  // Break the trust hash so ANY hector check would log an internal error.
  // A clean run proves the basename short-circuit fired before any check.
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-policy-"))
  const errs: string[] = []
  const origErr = console.error
  console.error = (...args: unknown[]) => {
    errs.push(args.map(String).join(" "))
  }
  try {
    writeFileSync(join(dir, ".hector.yml"), HECTOR_YML)
    execFileSync("hector", ["trust", "--config", join(dir, ".hector.yml")])
    const current = readFileSync(join(dir, ".hector.yml"), "utf8")
    writeFileSync(
      join(dir, ".hector.yml"),
      current.replace(/sha256:[0-9a-f]+/, "sha256:" + "0".repeat(64)),
    )
    const handlers = loadExtension(dir)
    const file = join(dir, ".hector.yml")
    assert.equal(
      handlers.tool_call!({ toolName: "write", input: { path: file, content: "anything\n" } }, {}),
      undefined,
    )
    assert.ok(!errs.join("\n").includes("internal error"))
  } finally {
    console.error = origErr
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: .bully.yml self-edit short-circuits (R3)", () => {
  const dir = makeProject()
  try {
    const file = join(dir, ".bully.yml")
    const handlers = loadExtension(dir)
    assert.equal(
      handlers.tool_call!({ toolName: "write", input: { path: file, content: "anything\n" } }, {}),
      undefined,
    )
    assert.equal(existsSync(file), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: bare relative .hector.yml self-edit short-circuits (R3)", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    assert.equal(
      handlers.tool_call!(
        { toolName: "write", input: { path: ".hector.yml", content: "anything\n" } },
        {},
      ),
      undefined,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// --- tool_call: exit-3 fail-open / fail-closed ----------------------------
// A real exit 3 (engine-internal error) is forced via a fake `hector` on PATH
// that exits 3 — a semantic/session config no longer reaches the engine (it is
// rejected at load with exit 1), so we cannot provoke exit 3 through config.

test("tool_call: exit-3 fails open by default (allows the edit)", () => {
  const dir = makeProject()
  const bin = mkdtempSync(join(tmpdir(), "hector-pi-fakebin-"))
  const origPath = process.env["PATH"] ?? ""
  delete process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"]
  const origErr = console.error
  console.error = () => {}
  try {
    const fake = join(bin, "hector")
    writeFileSync(fake, "#!/bin/sh\necho 'engine error' 1>&2\nexit 3\n")
    chmodSync(fake, 0o755)
    process.env["PATH"] = bin + delimiter + origPath

    const file = join(dir, "src", "x.rs")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn a() {}\n" } },
      {},
    )
    assert.equal(result, undefined) // fail-open
  } finally {
    console.error = origErr
    process.env["PATH"] = origPath
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
  }
})

test("tool_call: exit-3 fails closed under HECTOR_FAIL_CLOSED_ON_INTERNAL=1", () => {
  const dir = makeProject()
  const bin = mkdtempSync(join(tmpdir(), "hector-pi-fakebin-"))
  const origPath = process.env["PATH"] ?? ""
  const hadClosed = process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"]
  process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"] = "1"
  const origErr = console.error
  console.error = () => {}
  try {
    const fake = join(bin, "hector")
    writeFileSync(fake, "#!/bin/sh\necho 'engine error' 1>&2\nexit 3\n")
    chmodSync(fake, 0o755)
    process.env["PATH"] = bin + delimiter + origPath

    const file = join(dir, "src", "x.rs")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn a() {}\n" } },
      {},
    ) as { block?: boolean } | undefined
    assert.equal(result?.block, true) // fail-closed
  } finally {
    console.error = origErr
    process.env["PATH"] = origPath
    if (hadClosed === undefined) delete process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"]
    else process.env["HECTOR_FAIL_CLOSED_ON_INTERNAL"] = hadClosed
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
  }
})
