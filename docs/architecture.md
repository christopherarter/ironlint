# Architecture diagram

Hector turns repo-local policy into an automatic gate for AI coding agents. The short version: adapters catch edits, the `hector` binary checks those edits against trusted rules, and the adapter turns the verdict back into "keep going" or "fix this first."

```mermaid
flowchart LR
    subgraph People["People and policy"]
        Team["Team intent<br/>security, style, tests, architecture"]
        Config[".hector.yml<br/>rules, scope, severity, LLM provider"]
        Trust["Trust fingerprint<br/>reviewed config before rules run"]
        Trusted["Trusted resolved config<br/>extends merged, fingerprint verified"]
        Baseline["Baseline and disables<br/>suppress known or approved findings"]
    end

    subgraph Agents["AI coding tools"]
        Claude["Claude Code"]
        OpenCode["OpenCode"]
        Reasonix["Reasonix"]
        Pi["pi"]
        Future["Future and custom adapters<br/>Aider, pre-commit, MCP"]
    end

    subgraph AdapterLayer["Adapter layer"]
        Hooks["Edit and session hooks<br/>capture proposed content, diff, or session state"]
        Contract["Stable command contract<br/>hector check --format json"]
    end

    subgraph Hector["Hector"]
        CLI["hector CLI<br/>arguments, I/O, exit codes"]
        Core["hector-core pipeline<br/>load config, verify trust, match scope"]

        subgraph Engines["Four rule engines"]
            Script["script<br/>run project checks and linters"]
            AST["ast<br/>match code structure"]
            Semantic["semantic<br/>LLM judges intent in a diff, file, or repo"]
            Session["session<br/>LLM reviews the whole agent turn"]
        end

        Filter["Noise control<br/>baseline and hector-disable filters"]
        Verdict["Verdict JSON<br/>pass, warn, block, or internal_error"]
        Telemetry["Telemetry<br/>append-only check log"]
    end

    subgraph Outcome["Outcome"]
        Allow["Allow edit<br/>agent continues"]
        Warn["Warn<br/>surface policy feedback"]
        Block["Block per-edit gates<br/>adapter rejects the edit so the agent retries"]
        Advisory["Surface session findings<br/>hosts that cannot rewind still show what to fix"]
        Audit["Operate and improve<br/>review noisy, dead, or valuable rules"]
    end

    Team --> Config
    Config --> Trust
    Trust --> Trusted
    Trusted --> Core
    Baseline --> Filter

    Claude --> Hooks
    OpenCode --> Hooks
    Reasonix --> Hooks
    Pi --> Hooks
    Future --> Hooks
    Hooks --> Contract
    Contract --> CLI
    CLI --> Core

    Core --> Script
    Core --> AST
    Core --> Semantic
    Core --> Session
    Script --> Filter
    AST --> Filter
    Semantic --> Filter
    Session --> Filter
    Filter --> Verdict
    Verdict --> Telemetry
    Verdict --> Allow
    Verdict --> Warn
    Verdict --> Block
    Verdict --> Advisory
    Telemetry --> Audit
    Audit --> Config
```

## What this shows

- **Policy lives with the code.** The `.hector.yml` travels with the repo, so every agent sees the same rules and severities.
- **Adapters are thin.** Claude Code, OpenCode, Reasonix, pi, and future adapters capture host events and consume Hector's verdict. Policy logic stays in `hector-core`.
- **Rules scale from cheap to smart.** Use shell checks and AST matching for deterministic policies, then semantic and session rules when the question needs judgment across a diff, file, repo, or full agent turn.
- **Trust comes before power.** Script rules can execute commands, so Hector verifies the signed config before any rule runs.
- **The verdict is machine-readable.** `pass`, `warn`, `block`, and `internal_error` map to stable exit codes that agents and CI can act on automatically. Per-edit gates can block immediately; post-turn session checks surface findings when a host cannot rewind a completed turn.
- **The system improves over time.** Baselines and disables keep adoption practical; telemetry shows which rules are noisy, valuable, or dead.

## Mental model

Hector is not another linter. It is the policy layer around AI-generated edits: local enough to understand a repository's rules, structured enough for deterministic gates, and flexible enough to ask an LLM about the policies that ordinary tools cannot express.
