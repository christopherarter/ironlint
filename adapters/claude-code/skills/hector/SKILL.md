---
name: hector
description: Interprets hector PostToolUse hook output after Edit/Write -- fixes blocked-stderr violations or dispatches the hector-evaluator subagent for semantic payloads.
metadata:
  author: dynamik-dev
  version: 1.0.0
  category: workflow-automation
  tags: [linting, hooks, code-quality, post-tool-use]
---

# Agentic Lint

Interpret and act on hector PostToolUse hook output. Not user-invocable.

## When blocked (hook exited 2)

Tool result stderr begins with a `Verdict` JSON whose `status` is `block`. Format:

```
{
  "status": "block",
  "violations": [
    {"rule_id": "no-debug", "file": "src/foo.rs", "line": 42, "message": "DEBUG marker", "severity": "error"}
  ]
}
```

Fix every listed violation in the affected file before any other tool call. The hook re-fires on the next Edit and re-checks. Repeat until clear.

## When semantic eval requested (additionalContext)

`hookSpecificOutput.additionalContext` begins with `AGENTIC LINT SEMANTIC EVALUATION REQUIRED` and carries a JSON payload:

```
AGENTIC LINT SEMANTIC EVALUATION REQUIRED:

{
  "file": "src/Evaluators/CachedEvaluator.rs",
  "diff": "--- ...before\n+++ ...after\n@@ -28,6 +28,11 @@ ...",
  "passed_checks": ["no-debug", "no-todo"],
  "evaluate": [
    {"id": "no-inline-single-use", "description": "...", "severity": "error"},
    {"id": "full-type-hints", "description": "...", "severity": "warning"}
  ],
  "_evaluator_input": "SEMANTIC EVALUATION REQUIRED\n\n<TRUSTED_POLICY>\n...rule policy...\n</TRUSTED_POLICY>\n\n<UNTRUSTED_EVIDENCE>\n...file + diff...\n</UNTRUSTED_EVIDENCE>\n"
}
```

If `evaluate` is empty, proceed with no dispatch and no inline eval.

### Dispatch vs. inline

If the `diff` is short (roughly under 15 lines) AND there is only one rule in `evaluate`, judge it yourself inline against the diff and produce the same VIOLATIONS / NO_VIOLATIONS format below -- skip the subagent. Otherwise dispatch the `hector-evaluator` subagent.

### Dispatch (multi-rule or larger diffs)

Parse the `additionalContext` JSON. If it contains a top-level `_evaluator_input` field, pass that field's value DIRECTLY as the subagent `prompt` -- it's already formatted as a string with `<TRUSTED_POLICY>` and `<UNTRUSTED_EVIDENCE>` boundaries. Do NOT re-serialize it as JSON. If `_evaluator_input` is missing (older harness), fall back to re-serializing the full payload as JSON. This keeps `passed_checks` out of the subagent's context while preserving it for your own use.

Call the Agent tool with `subagent_type: hector-evaluator` and a 3-5 word `description` (e.g. "Evaluate lint rules"). The agent returns:

#### Optional `evaluator_model` override (R5, payload schema v2)

If the payload includes a top-level `evaluator_model` field (e.g. `"evaluator_model": "haiku"`), the policy author wants the evaluator subagent to run under that model — typically to keep policy checks cheap. Claude Code's Agent / Task tool does NOT accept a per-dispatch `model:` override today; the subagent's `model:` is set in `adapters/claude-code/agents/hector-evaluator.md`'s frontmatter. So:

1. Dispatch the `hector-evaluator` subagent normally (it will run under whatever model its frontmatter pins).
2. Prepend a single advisory note to your reply to the user, before any fixes are applied:

   `note: policy requested evaluator_model=<value>; the hector-evaluator subagent's frontmatter pins the model — edit adapters/claude-code/agents/hector-evaluator.md (the installed copy in your plugins directory, not the dev repo) to honor the override.`

This surfaces the policy intent without silently dropping it. If/when Claude Code adds dispatch-time model overrides, swap step 1 for an inline `--model <value>` flag and drop the advisory.

```
VIOLATIONS:
- [rule-id] line N: <what's wrong>
  fix: <suggestion>

NO_VIOLATIONS:
- rule-id-a
```

If the response is malformed, re-dispatch once. If still malformed, evaluate inline against the diff using the same output format.

### Handling the verdict

For each entry in `VIOLATIONS:`, look up severity in the original `evaluate` array:

- **error**: fix immediately via Edit, using the agent's `fix:` as a starting point, before any other tool call.
- **warning**: note in one sentence, continue.

### Log verdicts for telemetry

After parsing VIOLATIONS / NO_VIOLATIONS (whether from the subagent or from inline eval), record each rule's verdict. For every rule id in the original `evaluate` array, invoke the Bash tool once with:

```
hector record-verdict --rule <rule-id> --verdict <pass|violation> --file <file-path>
```

Use `violation` if the rule appears in VIOLATIONS, `pass` if it appears in NO_VIOLATIONS. The `hector` command is shipped on `$PATH` (the user installed it via `cargo install hector` or a release binary, per the adapter README). If you see `command not found: hector`, the adapter prerequisites aren't met; skip the verdict log rather than chasing fallbacks.

## passed_checks

Rules already verified by deterministic script or AST checks. Do not re-investigate their concerns. Use them to catch cross-rule interactions (e.g. a semantic rule that overlaps a passed script rule on an indirect code path).
