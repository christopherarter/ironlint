import { test } from "node:test"
import assert from "node:assert/strict"
import { synthesizeDiff, normalizeEdits } from "../src/index.ts"
import { mkdtempSync, writeFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { computeProposedContent } from "../src/index.ts"

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
