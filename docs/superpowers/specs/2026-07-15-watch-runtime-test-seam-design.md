# Watch Runtime Test Seam Design

## Context

Splitting the watch command exposed the live runtime as its own Rust source
file. Its existing helper tests cover configuration loading and cascade timing,
but the real terminal loop remains at 27.36% region coverage (58/212), below
the repository's 80% per-file gate. Rendering and state are already covered.

## Decision

Keep the module split and make the runtime loop deterministic through a private
runtime-I/O boundary. Production I/O delegates to the current telemetry reader,
monotonic clock, and Crossterm event functions. Tests provide a scripted
implementation while continuing to draw through a real `ratatui::Terminal`
backed by `TestBackend`.

The event loop will be decomposed enough to keep cognitive complexity at the
project's limit. No command-line behavior, rendering contract, poll intervals,
or cleanup semantics change.

## Contract

The boundary supplies four operations:

1. read incremental telemetry, including reset markers;
2. report a deterministic elapsed time in milliseconds;
3. poll for an input event using the selected interval; and
4. read that event when polling reports availability.

The loop continues to own state mutation, summary construction, terminal draw,
poll selection, and the existing keyboard behavior. The production adapter
delegates directly to `telemetry::read_since`, `Instant`, `event::poll`, and
`event::read`.

## Tests

Scripted tests exercise actual loop iterations with `TestBackend` and a queued
quit event. They cover:

- first successful backlog read, initial draw, idle poll, and quit;
- live arrival animation followed by settlement, active and idle poll choices,
  non-key events, non-press keys, and quit; and
- telemetry read failure followed by success, plus a reset/re-prime cycle.

Those tests leave the concrete terminal adapter below 80% region coverage. The
approved follow-up is one macOS/Linux black-box integration test that runs the
compiled `ironlint watch` binary inside a pseudo-terminal. It must wait for a
stable visible UI string, send raw `q` (without a newline), and observe a clean
exit. `expectrl` is the primary test harness; `portable-pty` remains an
acceptable lower-level alternative if direct control of the PTY becomes
necessary.

The PTY test covers the real positive path: TTY detection, raw-mode setup,
alternate-screen entry, Crossterm event polling, and cleanup. It must use a
bounded expectation timeout, assert semantic screen text rather than ANSI byte
sequences or animation frames, and run only where the repository currently
supports it (macOS/Linux). Windows support is explicitly deferred.

## Non-goals

- No change to `ironlint watch` interaction, output, or error handling.
- No relaxation or exemption of the coverage gate.
