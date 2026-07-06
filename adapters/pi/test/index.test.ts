import { test, after } from "node:test"
import assert from "node:assert/strict"
import { normalizeEdits, blockReason } from "../src/index.ts"
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, readFileSync, chmodSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, delimiter } from "node:path"
import { execFileSync } from "node:child_process"
import { computeProposedContent } from "../src/index.ts"
import ironlintExtension from "../src/index.ts"

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

// --- blockReason ----------------------------------------------------------

test("blockReason: extracts the message from a json verdict", () => {
  const json = JSON.stringify({
    status: "block",
    blocks: [{ gate: "no-panic", file: "f.rs", message: "no panics in source" }],
  })
  assert.equal(blockReason(json), "no panics in source")
})

test("blockReason: joins multiple block messages with newlines", () => {
  const json = JSON.stringify({ blocks: [{ message: "first" }, { message: "second" }] })
  assert.equal(blockReason(json), "first\nsecond")
})

test("blockReason: non-json stdout falls back to a generic reason", () => {
  assert.equal(blockReason("panic! detected\n"), "policy violation")
})

test("blockReason: a json verdict with no block messages falls back", () => {
  assert.equal(blockReason(JSON.stringify({ blocks: [] })), "policy violation")
})

test("blockReason: a non-string message is filtered out, not coerced", () => {
  // Guards the `typeof m === "string"` predicate: a malformed verdict must not
  // surface `42` (or `"undefined"`) as the reason — it falls back instead.
  assert.equal(blockReason(JSON.stringify({ blocks: [{ message: 42 }] })), "policy violation")
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
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-cpc-"))
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
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-cpc-"))
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
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-cpc-"))
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
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-cpc-"))
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
// invoke them with synthetic pi-shaped events against the real `ironlint`
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
  ironlintExtension(pi as unknown as Parameters<typeof ironlintExtension>[0])
  return handlers
}

// Checks-format (0.4) policy: one check that greps the proposed content (piped
// on stdin per the ABI) for `panic!` and blocks (any nonzero exit) with a
// message on stdout.
const IRONLINT_YML = `checks:
  no-panic:
    files: "*.rs"
    run: 'if grep -q "panic!" ; then echo "no panics in source"; exit 1; fi'
`

// Plan 2 enforces trust at `check`: an unblessed config fails closed (exit 4
// as of Task 3.2; the adapter blocks on it, see below).
// Point XDG_CONFIG_HOME at one ephemeral store for the whole file so blessing
// (execFileSync) and the adapter's own `ironlint check` (spawnSync — both inherit
// process.env) share it, and the real ~/.config/ironlint/trust.json is untouched.
const TRUST_STORE = mkdtempSync(join(tmpdir(), "ironlint-pi-xdg-"))
const PRIOR_XDG = process.env["XDG_CONFIG_HOME"]
process.env["XDG_CONFIG_HOME"] = TRUST_STORE
after(() => {
  if (PRIOR_XDG === undefined) delete process.env["XDG_CONFIG_HOME"]
  else process.env["XDG_CONFIG_HOME"] = PRIOR_XDG
  rmSync(TRUST_STORE, { recursive: true, force: true })
})

/** Bless `config` in the ephemeral trust store so `check` runs to a verdict. */
function bless(config: string): void {
  execFileSync("ironlint", ["trust", "--config", config])
}

function makeProject(): string {
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-proj-"))
  mkdirSync(join(dir, "src"), { recursive: true })
  writeFileSync(join(dir, ".ironlint.yml"), IRONLINT_YML)
  bless(join(dir, ".ironlint.yml"))
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

test("tool_call: block reason is the gate message, not the raw JSON verdict", () => {
  const dir = makeProject()
  try {
    const file = join(dir, "src", "reason.rs")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn b() { panic!(); }\n" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    // pi surfaces `reason` verbatim to the user — it must be the gate's stdout
    // message, not the `--format json` Verdict blob the CLI prints.
    assert.equal(result?.reason, "no panics in source")
    assert.ok(!result?.reason?.includes("schema_version"))
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

test("tool_call: non-gated tools (read) are ignored", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    assert.equal(
      handlers.tool_call!({ toolName: "read", input: { path: "anything" } }, {}),
      undefined,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// --- tool_call: bash branch (bash-gate self-trust prevention) ---------------
// `bash` is now gated by the bash-gate branch, which shells out to
// `ironlint gate-bash` (the shared Rust matcher). It runs BEFORE the
// config-existence check — the bash-gate must fire even with no .ironlint.yml,
// since that's exactly when an agent is most motivated to run `ironlint trust`.
// Block contract: return { block: true, reason } (same as the write/edit path).

test("tool_call: bash 'ironlint trust' blocks", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "bash", input: { command: "ironlint trust" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    assert.match(result?.reason ?? "", /ironlint trust must be run by a human/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: bash redirect to .ironlint.yml blocks", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "bash", input: { command: "echo x > .ironlint.yml" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    assert.match(result?.reason ?? "", /policy files must be edited/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: bash 'ls' allows (pre-filter skip)", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    // 'ls' never mentions ironlint → pre-filter skips the spawn entirely.
    assert.equal(
      handlers.tool_call!({ toolName: "bash", input: { command: "ls" } }, {}),
      undefined,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: bash 'ironlint trust' blocks even with no .ironlint.yml", () => {
  // The bash-gate must fire regardless of config presence — a config-less
  // project is exactly when the agent is most motivated to self-trust.
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-noconfig-bash-"))
  try {
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "bash", input: { command: "ironlint trust" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    assert.match(result?.reason ?? "", /ironlint trust must be run by a human/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: bash fails closed when ironlint is missing", () => {
  // No ironlint on PATH → spawn fails. The bash-gate must fail CLOSED (block),
  // not allow — a broken deny check is never a silent allow.
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-nobin-"))
  const bin = mkdtempSync(join(tmpdir(), "ironlint-pi-emptybin-"))
  const origPath = process.env["PATH"] ?? ""
  try {
    // PATH with no ironlint binary anywhere.
    process.env["PATH"] = bin + delimiter + origPath.replace(/[^:]+ironlint[^:]*/g, "")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "bash", input: { command: "ironlint trust" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    assert.match(result?.reason ?? "", /fail-closed/)
  } finally {
    process.env["PATH"] = origPath
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
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

test("tool_call: no-op when .ironlint.yml is absent", () => {
  // Spec §11: a project without a config silently no-ops (safe global install).
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-noconfig-"))
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

test("tool_call: gate activates after .ironlint.yml is created mid-session", () => {
  // Regression: the existence check runs per-invocation, so a project that
  // becomes an ironlint project after the extension loads starts gating with
  // no restart.
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-late-"))
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
    // Create + bless the config, re-invoke the SAME handler closure.
    writeFileSync(join(dir, ".ironlint.yml"), IRONLINT_YML)
    bless(join(dir, ".ironlint.yml"))
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn b() { panic!(); }\n" } },
      {},
    ) as { block?: boolean } | undefined
    assert.equal(result?.block, true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tool_call: .ironlint.yml self-edit short-circuits (R3) — no ironlint invocation", () => {
  // Write a deliberately invalid config so ANY real ironlint invocation would
  // exit non-zero and the adapter would log an "internal error". A clean run
  // (no such log) proves the basename short-circuit fired before ironlint ran.
  const dir = mkdtempSync(join(tmpdir(), "ironlint-pi-policy-"))
  const errs: string[] = []
  const origErr = console.error
  console.error = (...args: unknown[]) => {
    errs.push(args.map(String).join(" "))
  }
  try {
    writeFileSync(join(dir, ".ironlint.yml"), "checks: [unterminated\n")
    const handlers = loadExtension(dir)
    const file = join(dir, ".ironlint.yml")
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

test("tool_call: bare relative .ironlint.yml self-edit short-circuits (R3)", () => {
  const dir = makeProject()
  try {
    const handlers = loadExtension(dir)
    assert.equal(
      handlers.tool_call!(
        { toolName: "write", input: { path: ".ironlint.yml", content: "anything\n" } },
        {},
      ),
      undefined,
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// --- tool_call: exit-3 fail-open / fail-closed ----------------------------
// A real exit 3 (engine-internal error) is forced via a fake `ironlint` on PATH
// that exits 3 — a semantic/session config no longer reaches the engine (it is
// rejected at load with exit 1), so we cannot provoke exit 3 through config.

test("tool_call: exit-3 fails open by default (allows the edit)", () => {
  const dir = makeProject()
  const bin = mkdtempSync(join(tmpdir(), "ironlint-pi-fakebin-"))
  const origPath = process.env["PATH"] ?? ""
  delete process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"]
  const origErr = console.error
  console.error = () => {}
  try {
    const fake = join(bin, "ironlint")
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

test("tool_call: exit-3 fails closed under IRONLINT_FAIL_CLOSED_ON_INTERNAL=1", () => {
  const dir = makeProject()
  const bin = mkdtempSync(join(tmpdir(), "ironlint-pi-fakebin-"))
  const origPath = process.env["PATH"] ?? ""
  const hadClosed = process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"]
  process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"] = "1"
  const origErr = console.error
  console.error = () => {}
  try {
    const fake = join(bin, "ironlint")
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
    if (hadClosed === undefined) delete process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"]
    else process.env["IRONLINT_FAIL_CLOSED_ON_INTERNAL"] = hadClosed
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
  }
})

// --- tool_call: exit-4 (untrusted config) always blocks --------------------
// A real exit 4 (Task 3.2 / Finding C3: an untrusted/tampered config) is
// forced the same way exit 3 is above — via a fake `ironlint` on PATH — since
// a real trust mismatch would require racing the shared XDG trust store this
// file's other tests depend on. Unlike exit 3, there is no fail-open branch:
// exit 4 must ALWAYS block, with the fixed trust message as the reason.

test("tool_call: exit-4 (untrusted config) blocks with the trust message", () => {
  const dir = makeProject()
  const bin = mkdtempSync(join(tmpdir(), "ironlint-pi-fakebin-"))
  const origPath = process.env["PATH"] ?? ""
  const origErr = console.error
  console.error = () => {}
  try {
    const fake = join(bin, "ironlint")
    writeFileSync(fake, "#!/bin/sh\necho 'not trusted' 1>&2\nexit 4\n")
    chmodSync(fake, 0o755)
    process.env["PATH"] = bin + delimiter + origPath

    const file = join(dir, "src", "x.rs")
    const handlers = loadExtension(dir)
    const result = handlers.tool_call!(
      { toolName: "write", input: { path: file, content: "fn a() {}\n" } },
      {},
    ) as { block?: boolean; reason?: string } | undefined
    assert.equal(result?.block, true)
    assert.match(result?.reason ?? "", /not trusted/)
    assert.match(result?.reason ?? "", /ironlint trust/)
  } finally {
    console.error = origErr
    process.env["PATH"] = origPath
    rmSync(dir, { recursive: true, force: true })
    rmSync(bin, { recursive: true, force: true })
  }
})
