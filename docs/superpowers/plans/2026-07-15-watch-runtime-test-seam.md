# Watch Runtime Test Seam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cover the standalone watch runtime at or above the repository's 80% region-coverage gate without changing its terminal behavior.

**Architecture:** Replace the event loop's hard-coded telemetry, clock, and Crossterm event calls with a private runtime-I/O boundary. Keep the real `Terminal` draw path, but let unit tests run it with `TestBackend` and deterministic scripted I/O. Extract loop state helpers only as needed to keep individual functions below the cognitive-complexity limit.

**Tech Stack:** Rust, `ratatui` 0.30 `Terminal<TestBackend>`, Crossterm events, `tempfile`, `cargo-llvm-cov`.

## Global Constraints

- Preserve `ironlint watch` rendering, key behavior, poll intervals, and current terminal-cleanup error semantics.
- Tests must use `TestBackend`; do not open a real TTY or add sleeps.
- Keep all Rust files under `crates/*/src/` at or above 80% region coverage.
- Keep functions at cognitive complexity 15 or below; decompose rather than suppress the lint.
- The runtime-I/O boundary is private to `commands::watch`; no public CLI or core API changes.
- Do not relax, exempt, or weaken `scripts/ci-coverage.sh`.

---

### Task 1: Specify deterministic loop behavior with failing tests

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch/tests/runtime.rs`

**Interfaces:**
- Consumes: `event_loop`, `StreamRow`, `ViewState`, `Loop`, `ACTIVE_POLL_MS`, `IDLE_POLL_MS`.
- Produces: failing tests for a `pub(super) RuntimeIo` boundary and `pub(super) event_loop(..., io: &mut impl RuntimeIo)`.

- [ ] **Step 1: Add a scripted runtime fixture and the first failing loop test**

```rust
struct ScriptedRuntimeIo {
    reads: VecDeque<std::io::Result<(Vec<LogEntry>, bool)>>,
    now_ms: VecDeque<u64>,
    polls: Vec<Duration>,
    events: VecDeque<Event>,
}

impl RuntimeIo for ScriptedRuntimeIo {
    fn read_since(&mut self, _: &Path, _: &mut u64) -> std::io::Result<(Vec<LogEntry>, bool)> {
        self.reads.pop_front().expect("scripted telemetry read")
    }
    fn now_ms(&mut self) -> u64 { self.now_ms.pop_front().expect("scripted clock") }
    fn poll(&mut self, wait: Duration) -> std::io::Result<bool> {
        self.polls.push(wait);
        Ok(!self.events.is_empty())
    }
    fn read(&mut self) -> std::io::Result<Event> { Ok(self.events.pop_front().expect("scripted event")) }
}

#[test]
fn event_loop_primes_backlog_draws_and_quits_on_pressed_q() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut io = scripted_io(vec![Ok((vec![entry()], false))], vec![0], vec![press('q')]);
    event_loop(&mut terminal, Path::new("/project"), &[], true, &mut io).unwrap();
    assert_eq!(io.polls, vec![Duration::from_millis(IDLE_POLL_MS)]);
}
```

- [ ] **Step 2: Run the focused test and verify it fails because the boundary is absent**

Run: `cargo test -p ironlint-cli commands::watch::tests::runtime::event_loop_primes_backlog_draws_and_quits_on_pressed_q`

Expected: FAIL with unresolved `RuntimeIo`/five-argument `event_loop`, before any implementation exists.

- [ ] **Step 3: Add two more failing tests for the high-region branches**

```rust
#[test]
fn event_loop_uses_active_then_idle_poll_for_a_live_entry() {
    // Script a successful empty first tick, a new entry, then times 0, 250,
    // and 250 + ENTER_MS. Feed Resize, a released key, then pressed q.
    // Assert polls are [IDLE_POLL_MS, ACTIVE_POLL_MS, IDLE_POLL_MS].
}

#[test]
fn event_loop_retries_after_read_error_and_reprimes_after_reset() {
    // Script Err(NotFound), success with one backlog row, then reset=true
    // with a replacement row; finish with pressed q. Assert all scripted
    // reads were consumed and the final frame contains only the replacement.
}
```

- [ ] **Step 4: Run the focused test module and verify all new tests fail for missing runtime-I/O support**

Run: `cargo test -p ironlint-cli commands::watch::tests::runtime`

Expected: FAIL only at the intended missing runtime-I/O API; the existing cascade and config tests remain green.

- [ ] **Step 5: Commit the red tests**

```bash
git add crates/ironlint-cli/src/commands/watch/tests/runtime.rs
git commit -m "test-watch-runtime-loop-seam"
```

### Task 2: Implement the private runtime-I/O seam and loop-state decomposition

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch/runtime.rs`
- Modify: `crates/ironlint-cli/src/commands/watch/tests/runtime.rs`

**Interfaces:**
- Consumes: the scripted `RuntimeIo` and `event_loop` tests from Task 1.
- Produces: `pub(super) trait RuntimeIo`, `CrosstermRuntimeIo`, loop-state helpers, and `pub(super) fn event_loop<B, I>(..., io: &mut I) -> Result<()>`.

- [ ] **Step 1: Add only the runtime-I/O interface required by the failing tests**

```rust
pub(super) trait RuntimeIo {
    fn read_since(&mut self, log: &Path, offset: &mut u64) -> std::io::Result<(Vec<LogEntry>, bool)>;
    fn now_ms(&mut self) -> u64;
    fn poll(&mut self, wait: Duration) -> std::io::Result<bool>;
    fn read(&mut self) -> std::io::Result<Event>;
}

struct CrosstermRuntimeIo { loop_start: Instant }
```

The production implementation must delegate to the existing telemetry reader,
`millis(self.loop_start.elapsed())`, `event::poll`, and `event::read`.

- [ ] **Step 2: Keep loop mutation and row construction in focused helpers**

```rust
struct LoopState { state: ViewState, entries: Vec<LogEntry>, offset: u64, primed: bool, cascade: Cascade, revealed_ms: Vec<Option<u64>> }

impl LoopState {
    fn refresh<I: RuntimeIo>(&mut self, io: &mut I, log: &Path, now_ms: u64);
    fn rows(&self, now_ms: u64) -> (Vec<StreamRow<'_>>, bool);
}
```

`refresh` must preserve the existing successful-read, reset, initial-prime, and
failed-read behavior. `rows` must preserve each row's age and the animation
decision. Extract further helpers if clippy reports cognitive complexity above
15; do not add an allow attribute.

- [ ] **Step 3: Make the production path construct the real adapter**

```rust
let mut io = CrosstermRuntimeIo { loop_start: Instant::now() };
let result = event_loop(&mut terminal, dir, armed, config_loaded, &mut io);
```

Retain the existing order and `?` behavior of raw-mode setup, alternate-screen
entry, event loop, raw-mode cleanup, and alternate-screen exit.

- [ ] **Step 4: Run the focused runtime tests and verify they pass**

Run: `cargo test -p ironlint-cli commands::watch::tests::runtime`

Expected: PASS, including the three new scripted loop tests and existing
`load_armed`/cascade tests.

- [ ] **Step 5: Commit the green implementation**

```bash
git add crates/ironlint-cli/src/commands/watch/runtime.rs crates/ironlint-cli/src/commands/watch/tests/runtime.rs
git commit -m "refactor-watch-runtime-test-seam"
```

### Task 3: Verify coverage and add lifecycle seam only if evidence requires it

**Files:**
- Modify only if required: `crates/ironlint-cli/src/commands/watch/runtime.rs`
- Modify only if required: `crates/ironlint-cli/src/commands/watch/tests/runtime.rs`

**Interfaces:**
- Consumes: the Task 2 scripted runtime loop.
- Produces: verified >=80% region coverage for `runtime.rs`; no unnecessary terminal abstraction.

- [ ] **Step 1: Run the exact coverage gate**

Run: `bash scripts/ci-coverage.sh`

Expected: PASS with `crates/ironlint-cli/src/commands/watch/runtime.rs` at or
above 80% region coverage.

- [ ] **Step 2: If and only if runtime coverage remains below 80%, add red tests for lifecycle ordering**

```rust
#[test]
fn terminal_lifecycle_cleans_up_after_loop_error() {
    // A fake lifecycle records enter, loop, disable, and leave; assert cleanup
    // runs after a loop error and that current error precedence is preserved.
}
```

Run this test before implementation and verify it fails for the missing
lifecycle seam.

- [ ] **Step 3: Implement the smallest lifecycle seam and rerun its focused tests**

Keep production setup and cleanup ordering byte-for-byte equivalent in effect.
Test setup failure, loop failure, and cleanup failure paths; do not make an
existing partial-setup failure safer as part of this organizational change.

- [ ] **Step 4: Run final verification**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p ironlint-cli
bash scripts/ci-coverage.sh
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit any evidence-required lifecycle addition**

```bash
git add crates/ironlint-cli/src/commands/watch/runtime.rs crates/ironlint-cli/src/commands/watch/tests/runtime.rs
git commit -m "test-watch-runtime-lifecycle"
```

### Task 4: Review the coverage repair and finish the original module split

**Files:**
- Modify: `crates/ironlint-cli/src/commands/watch/mod.rs`
- Create: `crates/ironlint-cli/src/commands/watch/tests/mod.rs`
- Review: all files changed since merge base `bbb6ed7`.

**Interfaces:**
- Consumes: a coverage-compliant `runtime.rs`.
- Produces: watch tests live in focused test modules, temporary root test imports are removed, and the facade remains behavior-only.

- [ ] **Step 1: Complete the existing watch test relocation without changing assertions**

Retain `tests/{render,state,runtime}.rs` and their `tests/mod.rs` aggregator.
Remove temporary `#[cfg(test)]` imports from `watch/mod.rs`; each leaf test
module imports its direct dependencies.

- [ ] **Step 2: Run focused watch checks**

Run:

```bash
cargo test -p ironlint-cli commands::watch
cargo clippy -p ironlint-cli --all-targets -- -D warnings
```

Expected: both commands exit 0.

- [ ] **Step 3: Request a separate adversarial review**

Review the complete branch diff for behavior changes in terminal setup/cleanup,
poll timing, telemetry reset semantics, and test-only visibility. Every finding
must name a concrete runtime scenario; no style-only findings.

- [ ] **Step 4: Commit test relocation and verified fixes**

```bash
git add crates/ironlint-cli/src/commands/watch
git commit -m "test-watch-split-runtime-coverage"
```
