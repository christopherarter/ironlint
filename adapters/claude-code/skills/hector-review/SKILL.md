---
name: hector-review
description: Reviews hector rule health from the telemetry log. Use when the user says "review my hector rules", "check rule health", "which hector rules are noisy", "find dead hector rules", "hector review", or asks for an audit of .hector.yml.
metadata:
  author: dynamik-dev
  version: 0.1.0
  category: workflow-automation
  tags: [linting, telemetry, rule-pruning]
---

# Hector Review

Audit the rule set against telemetry. Surface candidates for removal, downgrade, or scope adjustment.

## Source of truth

`.hector/log.jsonl` — one record per `hector check` invocation. Schema:

```json
{"timestamp": "...", "kind": "check", "file": "src/foo.rs", "rule_id": null, "status": "pass"|"warn"|"block", "elapsed_ms": 42}
```

(Rule-level breakdowns will arrive in 0.2; at 0.1c the log is per-check.)

## Process

1. Read `.hector/log.jsonl`.
2. Aggregate by `status` over the last N entries (default last 1000 or all if fewer).
3. Surface concerning patterns:
   - **Block rate > 50%**: rules might be too strict or scope too broad.
   - **No blocks in N runs**: rule may be dead — never fires in practice.
   - **Slow rules** (high elapsed_ms): bump or split.

## Recommendations

For each concerning rule, propose ONE of:
- **Downgrade severity**: `error` → `warning`.
- **Tighten scope**: narrow the glob so it fires only where it should.
- **Remove**: dead rules add noise to the config.
- **Investigate**: high-block-rate rules may indicate a real codebase problem to fix in source, not in the rule.

Never apply recommendations silently. Present each one and ask the user.

## Output format

```
Reviewed N entries from .hector/log.jsonl (date range).

| Rule | Status counts | Recommendation |
|------|---------------|----------------|
| no-console-log | pass: 412, block: 0 | DEAD — consider removing |
| no-as-any | pass: 280, block: 89 | Active; high block rate; investigate |
| audit-tests | pass: 30, block: 1 | Healthy |
```

## Limitations at 0.1c

The telemetry log doesn't yet record per-rule pass/block (only per-check). Rule-level breakdowns will be richer in 0.2. For now, the review skill is a coarse-grained signal.
