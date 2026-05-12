---
name: hector-review
description: Reviews hector rule health from the telemetry log. Use when the user says "review my hector rules", "check rule health", "which hector rules are noisy", "find dead hector rules", "hector review", or asks for an audit of .hector.yml.
metadata:
  author: dynamik-dev
  version: 0.1.1
  category: workflow-automation
  tags: [linting, telemetry, rule-pruning]
---

# Hector Review

Audit the rule set against telemetry. Surface candidates for removal, downgrade, or scope adjustment.

## Source of truth

`.hector/log.jsonl` — one record per check invocation. Schema:

```json
{"timestamp": "...", "kind": "check" | "check_session", "file": "src/foo.rs", "rule_id": null, "status": "pass"|"warn"|"block", "elapsed_ms": 42}
```

- `kind: "check"` rows come from `hector check --file <path>` (the per-edit gate). `file` is the path that was checked.
- `kind: "check_session"` rows come from `hector check --session` (the Stop hook). `file` is `""` because session checks evaluate the accumulated changeset, not a single file.

**Granularity at 0.1:** records are **per check invocation**, not per rule. `rule_id` is always `null`; when a `check` runs three rules against one file, the log has one row whose `status` is the most severe of the three. Rule-level breakdowns arrive in 0.2.

That means at 0.1 you can recommend on **file patterns and scopes**, not on individual rule IDs. To attribute a block to a specific rule, the user has to re-run `hector check --file <path>` and read the verdict JSON.

## Process

1. Read `.hector/log.jsonl`.
2. Aggregate over the last N entries (default last 1000 or all if fewer), grouping by `file` (and by `rule_id` once it is populated).
3. Surface concerning patterns at the file-or-scope level:
   - **High block rate for a file or directory** (>50%): either rules covering it are too strict, or that file genuinely needs fixing at the source.
   - **Scope with zero blocks across many runs**: at least one rule on that scope may be dead, or the scope is too broad and only ever matches green files.
   - **Slow checks** (high `elapsed_ms`): which rule fires there is unknown until 0.2 — recommend the user run `hector check --file <path>` to attribute.
4. Cross-reference findings against `.hector.yml` scopes to point at the candidate rule(s) by glob, since you can't point at them by `rule_id` yet.

## Recommendations

For each concerning file or scope, propose ONE of:
- **Investigate source**: high block rate may be a real codebase problem to fix in code, not in the rule.
- **Tighten scope**: narrow a glob so a noisy rule fires only where it should.
- **Downgrade severity**: `error` → `warning` for a rule whose scope keeps getting blocked.
- **Run a follow-up check**: when the offender's rule is ambiguous (multiple rules cover the file), suggest `hector check --file <path> --format json` to identify which rule blocked.

Never apply recommendations silently. Present each one and ask the user.

## Output format

```
Reviewed N entries from .hector/log.jsonl (date range A → B).

Per-file health:

| File or scope        | Status counts                | Note / recommendation                                   |
|----------------------|------------------------------|---------------------------------------------------------|
| src/api/handlers.rs  | pass: 12, warn: 0, block: 7  | High block rate — investigate source or tighten scope   |
| tests/**             | pass: 84, warn: 0, block: 0  | No blocks — confirm rules with scope `tests/**` still earn their keep |
| crates/codegen/*.rs  | pass: 6, warn: 3, block: 0   | Slow (avg elapsed_ms = 480) — re-run with `--file` to attribute      |

To attribute a block to a specific rule (0.1 limitation):
  hector check --file <path> --config .hector.yml --format json
```

Always include the disclaimer line that at 0.1 the log is per-check, not per-rule.

## Limitations at 0.1

- `rule_id` is `null` in every log entry; recommendations are per-file or per-scope, not per-rule.
- A single `check` row's `status` is the most severe of the rules that ran, so a `block` on a file does not say which rule blocked.
- Rule-level breakdowns are planned for 0.2 (`rule_id` will be populated and one row will be emitted per rule per check).
