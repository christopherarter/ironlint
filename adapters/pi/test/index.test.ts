import { test } from "node:test"
import assert from "node:assert/strict"
import { synthesizeDiff, normalizeEdits } from "../src/index.ts"
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

// --- synthesizeDiff (P1-8 hunk counts, P1-9 injection scrub) --------------

test("synthesizeDiff: write tool, single-line content", () => {
  const d = synthesizeDiff("write", "foo.ts", { content: "x" })
  assert.match(d, /^--- a\/foo\.ts\n\+\+\+ b\/foo\.ts\n/)
  assert.ok(d.includes("@@ -1 +1 @@"))
})

test("synthesizeDiff: write tool, multi-line content emits zero-count old side (P1-8)", () => {
  const d = synthesizeDiff("write", "foo.ts", { content: "x\ny" })
  assert.ok(d.includes("@@ -1 +1,2 @@"))
  // Empty old side: no `-<content>` deletion lines (only the `--- a/` header).
  assert.doesNotMatch(d, /^-[^-]/m)
})

test("synthesizeDiff: edit tool, multi-line new emits correct counts (P1-8)", () => {
  const d = synthesizeDiff("edit", "foo.ts", { oldText: "a\nb", newText: "x\ny\nz" })
  assert.ok(d.includes("@@ -1,2 +1,3 @@"))
})

test("synthesizeDiff: edit tool, multi-line old single-line new (P1-8)", () => {
  const d = synthesizeDiff("edit", "foo.ts", { oldText: "a\nb\nc", newText: "x" })
  assert.ok(d.includes("@@ -1,3 +1 @@"))
})

test("synthesizeDiff: batch edit emits one hunk per edit", () => {
  const d = synthesizeDiff("edit", "foo.ts", {
    edits: [
      { oldText: "a", newText: "x" },
      { oldText: "b", newText: "y" },
    ],
  })
  // Exactly one file header, two @@ hunks.
  assert.equal(d.match(/^--- a\/foo\.ts$/gm)?.length, 1)
  assert.equal(d.match(/^@@ /gm)?.length, 2)
  assert.ok(d.includes("-a\n+x"))
  assert.ok(d.includes("-b\n+y"))
})

test("synthesizeDiff: escapes embedded +++/---/@@ headers in newText (P1-9)", () => {
  const evil = "x\n--- a/SECRET\n+++ b/SECRET\n@@ -1 +1 @@\n+pwn"
  const d = synthesizeDiff("edit", "foo.ts", { oldText: "", newText: evil })
  assert.doesNotMatch(d, /^\+\+\+ b\/SECRET$/m)
  assert.doesNotMatch(d, /^--- a\/SECRET$/m)
  assert.doesNotMatch(d, /^@@ -1 \+1 @@$/m)
  // The real headers for the real file remain.
  assert.ok(d.includes("--- a/foo.ts"))
  assert.ok(d.includes("+++ b/foo.ts"))
})

test("synthesizeDiff: escapes embedded headers in oldText (P1-9)", () => {
  // "-- a/SECRET" prefixed with "-" would become "--- a/SECRET" without scrubbing.
  const d = synthesizeDiff("edit", "foo.ts", { oldText: "-- a/SECRET", newText: "x" })
  assert.doesNotMatch(d, /^--- a\/SECRET$/m)
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

test("tool_result: records a write to session.json", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "tracked.txt")
    writeFileSync(file, "ok\n")
    const handlers = loadExtension(dir)
    handlers.tool_result!(
      { toolName: "write", input: { path: file, content: "ok\n" }, isError: false },
      {},
    )
    const stateFile = join(dir, ".hector", "session.json")
    assert.equal(existsSync(stateFile), true)
    assert.ok(readFileSync(stateFile, "utf8").includes("tracked.txt"))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_result: isError result records nothing", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "failed.txt")
    const handlers = loadExtension(dir)
    handlers.tool_result!(
      { toolName: "write", input: { path: file, content: "x\n" }, isError: true },
      {},
    )
    assert.equal(existsSync(join(dir, ".hector", "session.json")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_result: non-gated tool records nothing", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    handlers.tool_result!({ toolName: "read", input: { path: "x" } }, {})
    assert.equal(existsSync(join(dir, ".hector", "session.json")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_result: policy-file edit records nothing (R3)", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    handlers.tool_result!(
      { toolName: "write", input: { path: join(dir, ".hector.yml"), content: "x\n" } },
      {},
    )
    assert.equal(existsSync(join(dir, ".hector", "session.json")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("session_start: clears stale session.json", () => {
  const dir = makeProject()
  try {
    mkdirSync(join(dir, ".hector"), { recursive: true })
    writeFileSync(
      join(dir, ".hector", "session.json"),
      JSON.stringify({ session_id: "stale", started_at: "t", edits: [] }),
    )
    const handlers = loadExtension(dir)
    handlers.session_start!({}, {})
    assert.equal(existsSync(join(dir, ".hector", "session.json")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("session_start: no-op when no session.json exists", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    handlers.session_start!({}, {}) // must not throw
    assert.equal(existsSync(join(dir, ".hector", "session.json")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("agent_end: no-op without session.json", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    const notified: string[] = []
    const result = handlers.agent_end!({}, { ui: { notify: (m: string) => notified.push(m) } })
    assert.equal(result, undefined)
    assert.equal(notified.length, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("agent_end: advisory surface on a session block (never blocks the turn)", () => {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-agentend-"))
  const bin = mkdtempSync(join(tmpdir(), "hector-pi-fakebin-"))
  const origPath = process.env["PATH"] ?? ""
  const origErr = console.error
  const errs: string[] = []
  try {
    mkdirSync(join(dir, ".hector"), { recursive: true })
    writeFileSync(
      join(dir, ".hector", "session.json"),
      JSON.stringify({ session_id: "s", started_at: "t", edits: [] }),
    )
    // Fake `hector` that always exits 2 with a JSON verdict on stdout.
    const fake = join(bin, "hector")
    writeFileSync(
      fake,
      "#!/bin/sh\necho '{\"status\":\"block\",\"violations\":[{\"rule_id\":\"audit-tests\"}]}'\nexit 2\n",
    )
    chmodSync(fake, 0o755)
    process.env["PATH"] = bin + delimiter + origPath
    console.error = (...args: unknown[]) => {
      errs.push(args.map(String).join(" "))
    }

    const handlers = loadExtension(dir)
    const notified: string[] = []
    const result = handlers.agent_end!({}, { ui: { notify: (m: string) => notified.push(m) } })

    // Advisory: agent_end never returns a block, even on a session violation.
    assert.equal(result, undefined)
    assert.ok(errs.join("\n").includes("session check blocked"))
    assert.equal(notified.length, 1)
    assert.ok(notified[0]!.includes("session check blocked"))
  } finally {
    console.error = origErr
    process.env["PATH"] = origPath
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
  }
})

test("agent_end: non-blocking on session check internal error", () => {
  const dir = mkdtempSync(join(tmpdir(), "hector-pi-agentend2-"))
  const bin = mkdtempSync(join(tmpdir(), "hector-pi-fakebin2-"))
  const origPath = process.env["PATH"] ?? ""
  const origErr = console.error
  const errs: string[] = []
  try {
    mkdirSync(join(dir, ".hector"), { recursive: true })
    writeFileSync(
      join(dir, ".hector", "session.json"),
      JSON.stringify({ session_id: "s", started_at: "t", edits: [] }),
    )
    const fake = join(bin, "hector")
    writeFileSync(fake, "#!/bin/sh\necho 'boom' 1>&2\nexit 1\n")
    chmodSync(fake, 0o755)
    process.env["PATH"] = bin + delimiter + origPath
    console.error = (...args: unknown[]) => {
      errs.push(args.map(String).join(" "))
    }

    const handlers = loadExtension(dir)
    const notified: string[] = []
    const result = handlers.agent_end!({}, { ui: { notify: (m: string) => notified.push(m) } })

    assert.equal(result, undefined) // never blocks
    assert.equal(notified.length, 0) // no advisory notify on non-2 exit
    assert.ok(errs.join("\n").includes("internal error during session check"))
  } finally {
    console.error = origErr
    process.env["PATH"] = origPath
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
  }
})
