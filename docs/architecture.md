# Architecture diagram

Hector turns repo-local policy into an automatic gate for AI coding agents. The short version: adapters catch edits, the `hector` binary runs the matching checks against each edit, and the adapter turns the verdict back into "keep going" or "fix this first."

```mermaid
flowchart LR
    subgraph People["People and policy"]
        Team["Team intent<br/>security, style, tests, architecture"]
        Config[".hector.yml<br/>checks: files + run"]
        Trust["Trust store<br/>~/.config/hector/trust.json"]
        Resolved["Resolved config<br/>extends merged, trust verified"]
    end

    subgraph Agents["AI coding tools"]
        Claude["Claude Code"]
        OpenCode["OpenCode"]
        Reasonix["Reasonix"]
        Pi["pi"]
        Future["Future and custom adapters<br/>Aider, pre-commit, MCP"]
    end

    subgraph AdapterLayer["Adapter layer"]
        Hooks["Edit hooks<br/>capture proposed content"]
        ABI["Stable ABI<br/>$HECTOR_FILE, $HECTOR_ROOT, $HECTOR_EVENT, $HECTOR_TMPFILE, stdin"]
    end

    subgraph Hector["Hector"]
        CLI["hector CLI<br/>arguments, I/O, exit codes"]
        Core["hector-core<br/>load, verify trust, match files"]
        Run["Run each matching check<br/>sh -c run, read exit code"]
        Verdict["Verdict<br/>pass, block, or internal_error"]
        Telemetry["Telemetry<br/>append-only check log"]
    end

    subgraph Outcome["Outcome"]
        Allow["Allow edit<br/>agent continues"]
        Block["Block edit<br/>adapter rejects it so the agent retries"]
        Audit["Operate and improve<br/>review noisy, dead, or valuable checks"]
    end

    Team --> Config
    Config --> Trust
    Trust --> Resolved
    Resolved --> Core

    Claude --> Hooks
    OpenCode --> Hooks
    Reasonix --> Hooks
    Pi --> Hooks
    Future --> Hooks
    Hooks --> ABI
    ABI --> CLI
    CLI --> Core

    Core --> Run
    Run --> Verdict
    Verdict --> Telemetry
    Verdict --> Allow
    Verdict --> Block
    Telemetry --> Audit
    Audit --> Config
```

## What this shows

- **Policy lives with the code.** The `.hector.yml` travels with the repo, so every agent runs the same checks.
- **Adapters are thin.** Claude Code, OpenCode, Reasonix, pi, and future adapters capture host edit events and consume Hector's verdict over a stable ABI. No policy logic lives in the adapter.
- **One execution model.** Hector matches the edited file to checks and runs each check's `run` command, reading only the exit code. There are no engines and no severities — a check blocks on any nonzero exit (1–125) and owns its own message.
- **Trust comes before power.** A check runs shell, so Hector refuses to run a config whose bytes — and its `.hector/gates/` scripts — aren't blessed in the out-of-repo trust store.
- **The verdict is machine-readable.** `pass`, `block`, and `internal_error` map to stable exit codes that agents and CI act on. A per-edit check blocks immediately, so the agent retries before the change lands.
- **The system improves over time.** Telemetry records what ran, what blocked, and how long it took, so you can see which checks are noisy, valuable, or dead.

## Mental model

Hector is not another linter. It is the portable substrate beneath your linters: it normalizes every harness's edit hook into one ABI, runs the same check commands everywhere, and turns their exit codes into a deterministic gate an agent must clear before its edit lands.
