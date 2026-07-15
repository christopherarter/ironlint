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

If those tests leave `runtime.rs` below 80%, a second, narrowly scoped terminal
lifecycle seam will test setup and cleanup error ordering without altering its
current semantics. It is not introduced unless coverage evidence requires it.

## Non-goals

- No pseudo-terminal integration test or real terminal in unit tests.
- No change to `ironlint watch` interaction, output, or error handling.
- No relaxation or exemption of the coverage gate.
