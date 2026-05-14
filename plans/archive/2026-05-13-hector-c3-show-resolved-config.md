# Hector C3 — `hector show-resolved-config`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec section:** [`specs/2026-05-12-bully-parity-closures.md` §C3](../specs/2026-05-12-bully-parity-closures.md)
**Severity:** 🟢 medium UX
**Sequencing:** Independent of A/B/D-tracks; lands in 0.2.2 alongside `coverage` / `debt` / `explain` / `guide`. No verdict-schema impact, no exit-code impact, no engine impact.

---

**Goal:** Ship a single read-only inspection subcommand `hector show-resolved-config` that prints the post-`extends:` merged rule set so authors can confirm what their actual config looks like after inheritance. Three formats: TSV (default, greppable), YAML (canonical merged form, sans `trust:`), JSON (deterministic, sorted-by-rule-id, machine-readable). Each rule is annotated with the path of the file it originated from — local `.hector.yml`, an `extends:`-referenced parent, or a deeper transitive ancestor — so operators can answer "where did this rule come from?" without `git grep`-ing the extends chain.

**Architecture:** A new core helper `extends::resolve_with_origin(path) -> Result<(Config, BTreeMap<String, PathBuf>)>` returns the merged config alongside a per-rule-id origin map. The map is built inside the existing DFS walker by recording each rule's source file at insertion time and refusing to overwrite (matching `merge_inherited`'s "local wins on collision" semantics). The CLI command lives in `crates/hector-cli/src/commands/show_resolved_config.rs` and dispatches into one of three private formatters: `format_tsv`, `format_yaml`, `format_json`. Each formatter produces a `String` so the dispatcher stays a one-liner and complexity per function stays well under the cap of 15. Sorting happens once in the dispatcher (the `BTreeMap` already iterates in sorted-by-key order, but we materialize a `Vec<(&str, &Rule, &Path)>` and re-sort defensively before formatting so the contract isn't tied to the upstream container choice).

**Origin-tracking decision (justified):** I picked **resolver-side tracking via a sibling map**, not an in-`Config` field. Rationale: putting `origin: PathBuf` on `Rule` would (a) bloat the hot-path `Config` shape that every engine touches; (b) leak a serializable field that would need careful `#[serde(skip)]` handling everywhere `Rule` is serialized (we round-trip rules via wiremock fixtures and snapshot tests); (c) force the trust fingerprint to either ignore or include origin, both of which are wrong (origin is a file-system fact, not a policy decision). A sibling `BTreeMap<String, PathBuf>` returned alongside the `Config` is allocated only when `show-resolved-config` asks for it, costs nothing on the check-rule hot path, and keeps `Rule` shape stable. The existing `extends::resolve` and `extends::resolve_trusted` entry points stay byte-for-byte identical. The new entry point `resolve_with_origin` is the only thing show-resolved-config calls; it does *not* verify trust because show-resolved-config is read-only and operators reach for it precisely when they're debugging a config that may not yet trust-verify.

**`trust:`-stripping mechanism:** The YAML formatter does **not** serialize the merged `Config` directly. Instead it builds a parallel struct `ResolvedView { schema_version, llm, skip, execution, rules }` (fields mirror `Config` minus `trust` and `extends`) and serializes that. `extends:` is also stripped because by definition the post-extends merge has already consumed it — leaving it in would be misleading. The JSON formatter uses the same `ResolvedView`. The TSV formatter walks the merged-rules vec directly and never touches `trust:` at all.

**Tech Stack:** clap (`ValueEnum` for `--format`); `serde_yaml` and `serde_json` (both already workspace deps); `assert_cmd` + `tempfile` for CLI integration tests; `insta` snapshots (already in workspace deps via `insta = { version = "1", features = ["yaml", "json", "redactions"] }`). The new core types are `serde::Serialize` only — they never round-trip back through deserialize.

---

## File Structure

**NEW:**
- `crates/hector-cli/src/commands/show_resolved_config.rs` — the subcommand implementation: `pub fn run(config: &Path, format: ShowFormat) -> Result<i32>`, three private formatters, one private `ResolvedView` shape, plus unit tests for each formatter.
- `crates/hector-cli/tests/cli_e2e_show_resolved_config.rs` — `assert_cmd` + `tempfile` integration tests for all three formats; `insta` snapshots for the canonical outputs; tests for the missing-config and invalid-format error paths; multi-level `extends:` test asserting inheritance + override + origin.
- `docs/show-resolved-config.md` — public documentation of the three output shapes (TSV column order, YAML structure, JSON schema). Locks the public contract so future format changes can be discussed against a written reference.

**MODIFIED:**
- `crates/hector-cli/src/cli.rs` — new `Command::ShowResolvedConfig { config, format }` variant plus a new `ShowFormat { Tsv, Yaml, Json }` `ValueEnum`.
- `crates/hector-cli/src/main.rs` — dispatch arm that calls `commands::show_resolved_config::run`.
- `crates/hector-cli/src/commands/mod.rs` — `pub mod show_resolved_config;`.
- `crates/hector-core/src/config/extends.rs` — add `pub fn resolve_with_origin(root: &Path) -> Result<(Config, BTreeMap<String, PathBuf>)>` and a private `resolve_inner_with_origin` walker that records every rule's source file. The existing `resolve` / `resolve_trusted` / `resolve_inner` are unchanged.
- `README.md` — one-line bullet under a new "Inspect" subsection that lists the command and the three formats.

**Out-of-scope to TOUCH:**
- `crates/hector-core/src/config/types.rs` — `Rule`, `Config`, `EngineKind`, `Severity` shapes are stable; this plan does **not** add fields to any of them.
- `crates/hector-core/src/runner.rs` — `HectorEngine::load` is the trust-gated path used by `check`. Show-resolved-config bypasses it deliberately (read-only, doesn't run rules).
- `crates/hector-core/src/verdict.rs` — verdict shape is locked; this command never emits a verdict.

---

## Risk / rollback

- **Verdict-schema impact:** none. This command never constructs a `Verdict`.
- **Exit-code-contract impact:** `0` on success, `1` on config-error. Never `2` (block-only). Cannot regress the locked exit-code contract because no rule evaluation happens.
- **Config-schema impact:** the TSV/YAML/JSON output shapes are a **new public contract**. The TSV column order, the YAML field set (sans `trust:`/`extends:`), and the JSON object shape are documented in `docs/show-resolved-config.md` so any future change has a written reference to discuss against.
- **Performance impact:** trivial. One YAML parse per file in the extends chain (already happens during normal load); one extra `BTreeMap<String, PathBuf>` allocation per call; no I/O on the hot path of `check`.
- **Trust impact:** `resolve_with_origin` does **not** verify trust (operators reach for `show-resolved-config` precisely when debugging an as-yet-unsigned config). Documented in the function rustdoc and in `docs/show-resolved-config.md`. The output is read-only, so this is safe — no `script:` rule executes from this command.
- **Backwards-compat impact:** none — pure new subcommand. Removing it later would be a CLI break.

---

## Phase 1 — Resolver origin tracking

### Task 1.1: Failing core test for `resolve_with_origin`

**Files:**
- Create: `crates/hector-core/tests/extends_origin.rs`

- [ ] **Step 1: Write the failing test**

```rust
//! C3 — origin tracking on the post-extends merge. The walker must
//! attribute every rule to the file it was defined in, with local
//! definitions winning on collision (matching `resolve`'s existing
//! merge semantics).

use hector_core::config::extends::resolve_with_origin;
use std::path::PathBuf;
use tempfile::tempdir;

fn write(p: &std::path::Path, body: &str) {
    std::fs::write(p, body).unwrap();
}

#[test]
fn origin_map_attributes_each_rule_to_its_defining_file() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "schema_version: 2\nrules:\n  inherited:\n    description: \"from parent\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"parent version\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  local:\n    description: \"only in child\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"child wins\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let (cfg, origins) = resolve_with_origin(&child).unwrap();

    assert_eq!(cfg.rules.len(), 3, "merged rule count");
    assert_eq!(
        cfg.rules.get("overridden").unwrap().description,
        "child wins",
        "local wins on collision"
    );

    let canon_child: PathBuf = child.canonicalize().unwrap();
    let canon_parent: PathBuf = parent.canonicalize().unwrap();
    assert_eq!(origins.get("local").unwrap(), &canon_child);
    assert_eq!(origins.get("overridden").unwrap(), &canon_child, "child wins → child is the origin");
    assert_eq!(origins.get("inherited").unwrap(), &canon_parent);
}

#[test]
fn origin_map_records_transitive_grandparent() {
    let dir = tempdir().unwrap();
    let grand = dir.path().join("grand.yml");
    write(
        &grand,
        "schema_version: 2\nrules:\n  from-grand:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let parent = dir.path().join("parent.yml");
    write(
        &parent,
        "schema_version: 2\nextends: [\"grand.yml\"]\nrules: {}\n",
    );
    let child = dir.path().join(".hector.yml");
    write(
        &child,
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules: {}\n",
    );

    let (cfg, origins) = resolve_with_origin(&child).unwrap();
    assert_eq!(cfg.rules.len(), 1);
    assert_eq!(
        origins.get("from-grand").unwrap(),
        &grand.canonicalize().unwrap()
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hector-core --test extends_origin`
Expected: FAIL — `unresolved import` / `cannot find function 'resolve_with_origin' in module 'extends'`.

### Task 1.2: Implement `resolve_with_origin`

**Files:**
- Modify: `crates/hector-core/src/config/extends.rs`

- [ ] **Step 3: Add the new entry point and the origin-aware walker**

Append below the existing `merge_inherited`:

```rust
use std::collections::BTreeMap;

/// C3: resolve extends and return a side-channel mapping of every
/// surviving rule id to the canonical path of the file it was defined
/// in. Local definitions win on collision — same semantics as
/// [`resolve`] — and the origin map reflects that (the local file's
/// path is recorded for any rule the local config defined directly).
///
/// This entry point does **not** verify trust. `show-resolved-config`
/// is a read-only inspection command and operators reach for it
/// precisely when debugging an as-yet-unsigned config; gating it on
/// trust would defeat the purpose. Callers that intend to *execute*
/// rules must continue to use [`resolve_trusted`].
pub fn resolve_with_origin(root: &Path) -> Result<(Config, BTreeMap<String, PathBuf>)> {
    let mut seen = HashSet::new();
    let mut origins: BTreeMap<String, PathBuf> = BTreeMap::new();
    let cfg = resolve_inner_with_origin(root, &mut seen, &mut origins)?;
    Ok((cfg, origins))
}

fn resolve_inner_with_origin(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    origins: &mut BTreeMap<String, PathBuf>,
) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(anyhow!("extends cycle detected at {}", canonical.display()));
    }
    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("reading {}", canonical.display()))?;
    if matches!(super::parser::peek_schema_version(&content), Some(1)) {
        return Err(anyhow!(
            "{} is schema_version 1 (legacy bully); run `hector migrate` to upgrade to schema_version 2",
            canonical.display()
        ));
    }
    let mut cfg = parse_str(&content)?;

    // Record every rule defined *directly* in this file. Inherited
    // rules will only land in the origin map if no closer ancestor (or
    // the local file itself) already claimed the id, mirroring
    // `merge_inherited`'s "local wins on collision".
    for id in cfg.rules.keys() {
        origins.insert(id.clone(), canonical.clone());
    }

    let parent_dir = canonical.parent().unwrap_or_else(|| Path::new("."));
    let extends = std::mem::take(&mut cfg.extends);
    for relative in &extends {
        let abs = parent_dir.join(relative);
        let inherited = resolve_inner_with_origin(&abs, seen, origins)?;
        merge_inherited_with_origin(&mut cfg, inherited, origins, &abs);
    }
    seen.remove(&canonical);
    Ok(cfg)
}

fn merge_inherited_with_origin(
    local: &mut Config,
    inherited: Config,
    origins: &mut BTreeMap<String, PathBuf>,
    inherited_from: &Path,
) {
    let inherited_canonical = inherited_from
        .canonicalize()
        .unwrap_or_else(|_| inherited_from.to_path_buf());
    for (id, rule) in inherited.rules {
        if !local.rules.contains_key(&id) {
            // Only record the inherited file as origin when the local
            // hasn't claimed the id. The recursive walker has already
            // recorded the *defining* file (closest ancestor); only
            // overwrite if no entry exists.
            origins
                .entry(id.clone())
                .or_insert_with(|| inherited_canonical.clone());
            local.rules.insert(id, rule);
        }
    }
    if local.llm.is_none() {
        local.llm = inherited.llm;
    }
    for g in inherited.skip {
        if !local.skip.contains(&g) {
            local.skip.push(g);
        }
    }
}
```

- [ ] **Step 4: Run the test, expect green**

Run: `cargo test -p hector-core --test extends_origin`
Expected: PASS for both tests.

- [ ] **Step 5: Confirm no existing tests regressed**

Run: `cargo test -p hector-core`
Expected: every existing test still passes (the new walker is additive — `resolve` and `resolve_trusted` are byte-for-byte unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/hector-core/src/config/extends.rs crates/hector-core/tests/extends_origin.rs
git commit -m "$(cat <<'EOF'
feat(core): add extends::resolve_with_origin for C3

`resolve_with_origin(path) -> (Config, BTreeMap<String, PathBuf>)`
returns the post-extends merged config plus a side-channel map
attributing each surviving rule id to the canonical file it was defined
in. Local-wins-on-collision is preserved by `BTreeMap::or_insert_with`
in the inherited-merge step.

The new entry point does not verify trust — show-resolved-config is a
read-only inspection command. The trust-gated path through `resolve_trusted`
is unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — TSV formatter (default)

### Task 2.1: Failing CLI integration test for TSV default

**Files:**
- Create: `crates/hector-cli/tests/cli_e2e_show_resolved_config.rs`

- [ ] **Step 7: Write the failing TSV integration test**

```rust
//! C3 — `hector show-resolved-config` end-to-end coverage.

use assert_cmd::Command;
use tempfile::tempdir;

fn write_trusted(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let cfg = dir.join(".hector.yml");
    let trusted = hector_core::trust::write_trust_block(body).unwrap();
    std::fs::write(&cfg, trusted).unwrap();
    cfg
}

fn write_plain(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn tsv_default_emits_id_engine_severity_scope_fix_hint_origin() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  no-todo:\n    description: \"reject TODO\"\n    engine: script\n    scope: [\"*.rs\", \"*.txt\"]\n    severity: warning\n    script: \"true\"\n    fix_hint: \"remove the TODO\"\n  no-unwrap:\n    description: \"avoid unwrap\"\n    engine: ast\n    scope: [\"*.rs\"]\n    severity: error\n    pattern: \"$X.unwrap()\"\n    language: \"rust\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", cfg.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "two rules → two lines, no header: {stdout}");

    // BTreeMap iteration is alphabetic by id; defensive sort lives in the
    // command. `no-todo` < `no-unwrap`.
    let cols0: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(cols0.len(), 6, "TSV row must have 6 columns: {:?}", cols0);
    assert_eq!(cols0[0], "no-todo");
    assert_eq!(cols0[1], "script");
    assert_eq!(cols0[2], "warning");
    assert_eq!(cols0[3], "*.rs,*.txt");
    assert_eq!(cols0[4], "remove the TODO");
    assert!(
        cols0[5].ends_with(".hector.yml"),
        "origin must point at the local config: {}",
        cols0[5]
    );

    let cols1: Vec<&str> = lines[1].split('\t').collect();
    assert_eq!(cols1[0], "no-unwrap");
    assert_eq!(cols1[1], "ast");
    assert_eq!(cols1[2], "error");
    assert_eq!(cols1[3], "*.rs");
    assert_eq!(cols1[4], "", "empty fix_hint must emit an empty cell, preserving column count");
    assert!(cols1[5].ends_with(".hector.yml"));
}

#[test]
fn tsv_extends_chain_inherits_and_overrides_with_origin() {
    let dir = tempdir().unwrap();
    let parent = write_plain(
        dir.path(),
        "parent.yml",
        "schema_version: 2\nrules:\n  inherited:\n    description: \"from parent\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"parent version\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: error\n    script: \"true\"\n",
    );
    let child = write_trusted(
        dir.path(),
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  local-only:\n    description: \"only in child\"\n    engine: script\n    scope: [\"*.md\"]\n    severity: warning\n    script: \"true\"\n  overridden:\n    description: \"child wins\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args(["show-resolved-config", "--config", child.to_str().unwrap()])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "merged rule count = 3");

    let parsed: std::collections::BTreeMap<&str, Vec<&str>> = lines
        .iter()
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            (cols[0], cols)
        })
        .collect();

    let inherited = parsed.get("inherited").unwrap();
    assert!(
        inherited[5].ends_with("parent.yml"),
        "inherited rule's origin is the parent file: {}",
        inherited[5]
    );

    let local = parsed.get("local-only").unwrap();
    assert!(local[5].ends_with(".hector.yml"));

    let overridden = parsed.get("overridden").unwrap();
    assert_eq!(overridden[2], "warning", "child wins on collision");
    assert!(
        overridden[5].ends_with(".hector.yml"),
        "child-defined rule's origin is the child file"
    );

    // Silence the unused-binding lint without weakening the test.
    let _ = parent;
}
```

- [ ] **Step 8: Run the test, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config tsv_default_emits_id_engine_severity_scope_fix_hint_origin`
Expected: FAIL — `error: unrecognized subcommand 'show-resolved-config'`.

### Task 2.2: Add the clap subcommand wiring

**Files:**
- Modify: `crates/hector-cli/src/cli.rs`
- Modify: `crates/hector-cli/src/main.rs`
- Modify: `crates/hector-cli/src/commands/mod.rs`

- [ ] **Step 9: Add `Command::ShowResolvedConfig` and `ShowFormat` to `cli.rs`**

In `crates/hector-cli/src/cli.rs`, append a new variant inside `Command` (between `Session` and the closing brace) **and** a new `ShowFormat` enum at the bottom:

```rust
    /// Print the post-extends merged rule set.
    ///
    /// Read-only. Does not run any rule. Default format is TSV with the
    /// columns: `id<TAB>engine<TAB>severity<TAB>scope<TAB>fix_hint<TAB>origin`.
    /// `--format yaml` prints the canonical merged config (sans `trust:`
    /// and `extends:`); `--format json` prints the same shape as JSON
    /// with each rule annotated by its origin.
    ShowResolvedConfig {
        #[arg(long, default_value = ".hector.yml")]
        config: PathBuf,
        #[arg(long, default_value = "tsv")]
        format: ShowFormat,
    },
```

Append the value-enum at the bottom of the file:

```rust
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ShowFormat {
    Tsv,
    Yaml,
    Json,
}
```

- [ ] **Step 10: Wire the dispatch arm in `main.rs`**

Add inside the existing `match cli.command { … }` (anywhere between the existing arms):

```rust
        Command::ShowResolvedConfig { config, format } => {
            commands::show_resolved_config::run(&config, format)?
        }
```

- [ ] **Step 11: Register the new module in `commands/mod.rs`**

Add the line:

```rust
pub mod show_resolved_config;
```

- [ ] **Step 12: Stub the command so the binary compiles**

Create `crates/hector-cli/src/commands/show_resolved_config.rs` with the bare-minimum scaffold (so the next task's implementation step is purely additive — no fighting the borrow checker on a half-typed module):

```rust
//! C3 — `hector show-resolved-config`. Print the post-extends merged
//! rule set in one of three formats. Read-only.

use crate::cli::ShowFormat;
use anyhow::Result;
use std::path::Path;

pub fn run(config: &Path, format: ShowFormat) -> Result<i32> {
    match hector_core::config::extends::resolve_with_origin(config) {
        Ok((cfg, origins)) => {
            let body = match format {
                ShowFormat::Tsv => format_tsv(&cfg, &origins),
                ShowFormat::Yaml => format_yaml(&cfg, &origins)?,
                ShowFormat::Json => format_json(&cfg, &origins)?,
            };
            print!("{body}");
            Ok(0)
        }
        Err(e) => {
            eprintln!("ERROR: {:#}", e);
            Ok(1)
        }
    }
}

fn format_tsv(
    _cfg: &hector_core::config::Config,
    _origins: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> String {
    String::new()
}

fn format_yaml(
    _cfg: &hector_core::config::Config,
    _origins: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> Result<String> {
    Ok(String::new())
}

fn format_json(
    _cfg: &hector_core::config::Config,
    _origins: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> Result<String> {
    Ok(String::new())
}
```

- [ ] **Step 13: Verify the binary compiles and the failing test now fails for the *right* reason**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config tsv_default_emits_id_engine_severity_scope_fix_hint_origin`
Expected: FAIL — assertion `lines.len() == 2` fails because `format_tsv` returns an empty string. (Subcommand is now wired; we're failing because the formatter is a stub.)

### Task 2.3: Implement `format_tsv`

**Files:**
- Modify: `crates/hector-cli/src/commands/show_resolved_config.rs`

- [ ] **Step 14: Replace the `format_tsv` stub with the real implementation**

Replace the `format_tsv` function body and add a small private helper for sorted iteration:

```rust
fn format_tsv(
    cfg: &hector_core::config::Config,
    origins: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> String {
    let mut out = String::new();
    for (id, rule) in sorted_rules(cfg) {
        let engine = engine_kind_str(rule.engine);
        let severity = severity_str(rule.severity);
        let scope = rule.scope.join(",");
        let fix_hint = rule.fix_hint.as_deref().unwrap_or("");
        let origin = origins
            .get(id)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        // Six columns; cell separator is a single tab; row terminator is
        // newline. Empty cells preserve column count so downstream
        // `cut -f6` still works on rows with no fix_hint.
        out.push_str(&format!(
            "{id}\t{engine}\t{severity}\t{scope}\t{fix_hint}\t{origin}\n"
        ));
    }
    out
}

/// Materialize the rule list once in deterministic id order. `BTreeMap`
/// already iterates in key order; we re-sort defensively so the output
/// contract isn't tied to the upstream container choice.
fn sorted_rules(
    cfg: &hector_core::config::Config,
) -> Vec<(&String, &hector_core::config::Rule)> {
    let mut v: Vec<(&String, &hector_core::config::Rule)> = cfg.rules.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

fn engine_kind_str(k: hector_core::config::EngineKind) -> &'static str {
    match k {
        hector_core::config::EngineKind::Script => "script",
        hector_core::config::EngineKind::Ast => "ast",
        hector_core::config::EngineKind::Semantic => "semantic",
        hector_core::config::EngineKind::Session => "session",
    }
}

fn severity_str(s: hector_core::config::Severity) -> &'static str {
    match s {
        hector_core::config::Severity::Error => "error",
        hector_core::config::Severity::Warning => "warning",
    }
}
```

- [ ] **Step 15: Run the TSV tests, expect green**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config tsv_default_emits_id_engine_severity_scope_fix_hint_origin`
Expected: PASS.

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config tsv_extends_chain_inherits_and_overrides_with_origin`
Expected: PASS.

- [ ] **Step 16: Commit**

```bash
git add crates/hector-cli/src/cli.rs crates/hector-cli/src/main.rs crates/hector-cli/src/commands/mod.rs crates/hector-cli/src/commands/show_resolved_config.rs crates/hector-cli/tests/cli_e2e_show_resolved_config.rs
git commit -m "$(cat <<'EOF'
feat(cli): add show-resolved-config TSV default (C3 phase 1)

`hector show-resolved-config` prints the post-extends merged rule set
as tab-separated `id<TAB>engine<TAB>severity<TAB>scope<TAB>fix_hint<TAB>origin`
rows, sorted by rule id. Multiple scope globs collapse into a single
comma-separated cell; missing fix_hint emits an empty cell so the
column count stays stable. Origin column points at the canonical path
of the file each rule was defined in (local file or extends-referenced
parent).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — YAML formatter (`--format yaml`)

### Task 3.1: Failing test for YAML output

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_show_resolved_config.rs`

- [ ] **Step 17: Append the failing YAML test**

```rust
#[test]
fn yaml_format_emits_canonical_merged_config_without_trust() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "yaml",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();

    // Trust block must be stripped from the rendered view; otherwise we
    // would imply the merged form has a fingerprint, which is meaningless.
    assert!(!stdout.contains("trust:"), "yaml must not emit trust block: {stdout}");
    assert!(!stdout.contains("fingerprint:"), "yaml must not emit fingerprint: {stdout}");
    // Extends already consumed by the merge; rendering it would mislead.
    assert!(!stdout.contains("extends:"), "yaml must not emit extends list: {stdout}");

    // The merged config round-trips through serde_yaml as a map; the
    // origin comments precede each rule.
    assert!(stdout.contains("alpha:"));
    assert!(stdout.contains("# origin:"));
    assert!(stdout.contains(".hector.yml"));
}

#[test]
fn yaml_format_origin_comment_precedes_each_rule() {
    let dir = tempdir().unwrap();
    let parent = write_plain(
        dir.path(),
        "parent.yml",
        "schema_version: 2\nrules:\n  beta:\n    description: \"b\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let child = write_trusted(
        dir.path(),
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            child.to_str().unwrap(),
            "--format",
            "yaml",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();

    // Every rule has a preceding `# origin: <path>` comment line.
    let alpha_origin = stdout.find("# origin: ").and_then(|i| {
        let after = &stdout[i..];
        after.lines().next()
    });
    assert!(alpha_origin.is_some(), "expected at least one origin comment: {stdout}");
    // Both rules surface in the body.
    assert!(stdout.contains("alpha:"));
    assert!(stdout.contains("beta:"));

    let _ = parent;
}
```

- [ ] **Step 18: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config yaml_format_emits_canonical_merged_config_without_trust`
Expected: FAIL — empty stdout (`format_yaml` is a stub).

### Task 3.2: Implement `format_yaml` with `ResolvedView` and origin comments

**Files:**
- Modify: `crates/hector-cli/src/commands/show_resolved_config.rs`

- [ ] **Step 19: Add the `ResolvedView` struct and replace the YAML stub**

Add the imports and the new struct near the top of `show_resolved_config.rs`:

```rust
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The shape that gets serialized by the YAML and JSON formatters.
///
/// Mirrors `Config` minus the two fields that are meaningless after the
/// extends merge:
/// - `trust:` — per-config-file fingerprint; the merged form has no
///   single source file to fingerprint.
/// - `extends:` — already consumed by the merge; leaving it in would
///   imply unresolved inheritance.
///
/// Constructed by [`build_view`] from a `Config` + the rule origin map.
#[derive(Debug, Serialize)]
struct ResolvedView<'a> {
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm: Option<&'a hector_core::config::LlmConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skip: &'a Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution: Option<&'a hector_core::config::ExecutionConfig>,
    /// Sorted-by-id rule list. Each entry carries an `origin` field
    /// alongside the rule body so the JSON shape can attribute every
    /// rule to its source file.
    rules: BTreeMap<&'a str, RuleView<'a>>,
}

#[derive(Debug, Serialize)]
struct RuleView<'a> {
    #[serde(flatten)]
    rule: &'a hector_core::config::Rule,
    origin: String,
}

fn build_view<'a>(
    cfg: &'a hector_core::config::Config,
    origins: &'a BTreeMap<String, PathBuf>,
) -> ResolvedView<'a> {
    let rules: BTreeMap<&'a str, RuleView<'a>> = cfg
        .rules
        .iter()
        .map(|(id, rule)| {
            let origin = origins
                .get(id)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            (id.as_str(), RuleView { rule, origin })
        })
        .collect();
    ResolvedView {
        schema_version: cfg.schema_version,
        llm: cfg.llm.as_ref(),
        skip: &cfg.skip,
        execution: cfg.execution.as_ref(),
        rules,
    }
}
```

Replace the YAML stub:

```rust
fn format_yaml(
    cfg: &hector_core::config::Config,
    origins: &BTreeMap<String, PathBuf>,
) -> Result<String> {
    let view = build_view(cfg, origins);
    let body = serde_yaml::to_string(&view)?;
    Ok(annotate_yaml_with_origins(&body, origins))
}

/// Walk the rendered YAML body and inject a `# origin: <path>` comment
/// line above each rule entry. Detects rule entries by matching lines
/// of the form `^  <id>:$` *inside* the `rules:` block — every rule key
/// in `ResolvedView` is two-space-indented.
fn annotate_yaml_with_origins(
    body: &str,
    origins: &BTreeMap<String, PathBuf>,
) -> String {
    let mut out = String::with_capacity(body.len() + 128);
    let mut in_rules_block = false;
    for line in body.lines() {
        if line.starts_with("rules:") {
            in_rules_block = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_rules_block {
            // A rule-key line is `  <id>:` with exactly two leading
            // spaces and a trailing colon. Anything more deeply
            // indented is a field of the rule body, not a new rule.
            if let Some(stripped) = line.strip_prefix("  ") {
                if !stripped.starts_with(' ')
                    && stripped.ends_with(':')
                    && stripped.len() > 1
                {
                    let id = &stripped[..stripped.len() - 1];
                    if let Some(origin) = origins.get(id) {
                        out.push_str(&format!("  # origin: {}\n", origin.display()));
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
```

- [ ] **Step 20: Run YAML tests**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config yaml_format_emits_canonical_merged_config_without_trust`
Expected: PASS.

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config yaml_format_origin_comment_precedes_each_rule`
Expected: PASS.

- [ ] **Step 21: Commit**

```bash
git add crates/hector-cli/src/commands/show_resolved_config.rs crates/hector-cli/tests/cli_e2e_show_resolved_config.rs
git commit -m "$(cat <<'EOF'
feat(cli): show-resolved-config --format yaml (C3 phase 2)

`hector show-resolved-config --format yaml` emits the canonical merged
config with `trust:` and `extends:` stripped (both meaningless on the
post-merge view) and a `# origin: <path>` comment above every rule
entry. Rendered through a `ResolvedView` struct so the live `Config`
shape never leaks the trust block.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — JSON formatter (`--format json`)

### Task 4.1: Failing test for JSON output

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_show_resolved_config.rs`

- [ ] **Step 22: Append the failing JSON test**

```rust
#[test]
fn json_format_emits_sorted_rules_with_origin_field() {
    let dir = tempdir().unwrap();
    let parent = write_plain(
        dir.path(),
        "parent.yml",
        "schema_version: 2\nrules:\n  zeta:\n    description: \"z\"\n    engine: script\n    scope: [\"*.txt\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let child = write_trusted(
        dir.path(),
        "schema_version: 2\nextends: [\"parent.yml\"]\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );

    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            child.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();

    assert!(v.get("trust").is_none(), "json view must not contain a trust block");
    assert!(v.get("extends").is_none(), "json view must not contain extends");
    assert_eq!(v["schema_version"], 2);

    let rules = v["rules"].as_object().unwrap();
    assert_eq!(rules.len(), 2);
    let keys: Vec<&str> = rules.keys().map(|k| k.as_str()).collect();
    // serde_json::Map preserves insertion order; we built the map from a
    // BTreeMap so insertion order *is* sorted-by-id order.
    assert_eq!(keys, vec!["alpha", "zeta"], "rules must be sorted by id");

    let alpha = &rules["alpha"];
    assert_eq!(alpha["engine"], "script");
    assert_eq!(alpha["severity"], "warning");
    assert!(alpha["origin"].as_str().unwrap().ends_with(".hector.yml"));

    let zeta = &rules["zeta"];
    assert!(zeta["origin"].as_str().unwrap().ends_with("parent.yml"));

    let _ = parent;
}
```

- [ ] **Step 23: Run, expect failure**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config json_format_emits_sorted_rules_with_origin_field`
Expected: FAIL — empty stdout (`format_json` is a stub).

### Task 4.2: Implement `format_json`

**Files:**
- Modify: `crates/hector-cli/src/commands/show_resolved_config.rs`

- [ ] **Step 24: Replace the JSON stub**

```rust
fn format_json(
    cfg: &hector_core::config::Config,
    origins: &BTreeMap<String, PathBuf>,
) -> Result<String> {
    let view = build_view(cfg, origins);
    // Pretty-printed for human inspection; tooling can re-serialize.
    let body = serde_json::to_string_pretty(&view)?;
    // Trailing newline so `... | wc -l` includes the last line.
    Ok(format!("{body}\n"))
}
```

- [ ] **Step 25: Run JSON test**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config json_format_emits_sorted_rules_with_origin_field`
Expected: PASS.

- [ ] **Step 26: Commit**

```bash
git add crates/hector-cli/src/commands/show_resolved_config.rs crates/hector-cli/tests/cli_e2e_show_resolved_config.rs
git commit -m "$(cat <<'EOF'
feat(cli): show-resolved-config --format json (C3 phase 3)

JSON output reuses the `ResolvedView` struct from the YAML path; rules
are sorted by id (insertion order from a BTreeMap), each rule object
carries an `origin: <path>` field, and `trust:` / `extends:` are
omitted. Pretty-printed via `serde_json::to_string_pretty` so the
output is human-readable as well as machine-parseable.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Error paths

### Task 5.1: Missing `.hector.yml` exits 1 with a hint on stderr

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_show_resolved_config.rs`

- [ ] **Step 27: Append the failing test**

```rust
#[test]
fn missing_config_exits_one_with_hint() {
    let dir = tempdir().unwrap();
    // No `.hector.yml` written; the path doesn't exist.
    let absent = dir.path().join(".hector.yml");
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            absent.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("ERROR: "), "stderr must lead with ERROR: prefix: {stderr}");
    assert!(
        stderr.contains(".hector.yml"),
        "stderr must name the absent file so the user can act on it: {stderr}"
    );
}
```

- [ ] **Step 28: Run; expect PASS already**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config missing_config_exits_one_with_hint`

The current `run` already converts the resolver `Err` into `ERROR: {:#}` on stderr and returns `Ok(1)`; the test should pass without further code changes. Confirm:
Expected: PASS. If it fails, the diagnostic from `canonicalize` may not include the file name on this OS — adjust `run` to wrap the error with `with_context(|| format!("loading {}", config.display()))` so the path is always part of the message.

### Task 5.2: `--format <invalid>` is rejected by clap

**Files:**
- Modify: `crates/hector-cli/tests/cli_e2e_show_resolved_config.rs`

- [ ] **Step 29: Append the failing test**

```rust
#[test]
fn invalid_format_value_is_rejected_by_clap() {
    let dir = tempdir().unwrap();
    let cfg = write_trusted(
        dir.path(),
        "schema_version: 2\nrules:\n  alpha:\n    description: \"a\"\n    engine: script\n    scope: [\"*.rs\"]\n    severity: warning\n    script: \"true\"\n",
    );
    let out = Command::cargo_bin("hector")
        .unwrap()
        .args([
            "show-resolved-config",
            "--config",
            cfg.to_str().unwrap(),
            "--format",
            "csv",
        ])
        .output()
        .unwrap();
    // clap exits with code 2 on argument-parse failure regardless of our
    // app-level contract, because the user never reached our `run`.
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("invalid")
            || stderr.to_lowercase().contains("possible values"),
        "clap should reject `csv` as an invalid format value: {stderr}"
    );
}
```

- [ ] **Step 30: Run, expect green**

Run: `cargo test -p hector-cli --test cli_e2e_show_resolved_config invalid_format_value_is_rejected_by_clap`
Expected: PASS — clap's `ValueEnum` handles this for free.

- [ ] **Step 31: Commit**

```bash
git add crates/hector-cli/tests/cli_e2e_show_resolved_config.rs
git commit -m "$(cat <<'EOF'
test(cli): cover show-resolved-config error paths (C3 phase 4)

Two new integration tests:
- missing `.hector.yml` exits 1 with the file path on stderr,
- `--format csv` (or any non-{tsv,yaml,json} value) is rejected by
  clap before our command ever runs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Unit tests on the formatters

### Task 6.1: Unit-test the formatter helpers in-place

**Files:**
- Modify: `crates/hector-cli/src/commands/show_resolved_config.rs`

- [ ] **Step 32: Append a `#[cfg(test)] mod tests` block to the command module**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn rule_for(scope: Vec<&str>, fix_hint: Option<&str>) -> hector_core::config::Rule {
        hector_core::config::Rule {
            description: "x".into(),
            engine: hector_core::config::EngineKind::Script,
            scope: scope.into_iter().map(|s| s.to_string()).collect(),
            severity: hector_core::config::Severity::Warning,
            script: Some("true".into()),
            pattern: None,
            language: None,
            context: None,
            capabilities: None,
            fix_hint: fix_hint.map(|s| s.to_string()),
        }
    }

    fn cfg_with(rules: Vec<(&str, hector_core::config::Rule)>) -> hector_core::config::Config {
        let mut map = std::collections::BTreeMap::new();
        for (id, r) in rules {
            map.insert(id.to_string(), r);
        }
        hector_core::config::Config {
            schema_version: 2,
            llm: None,
            extends: vec![],
            trust: None,
            skip: vec![],
            execution: None,
            rules: map,
        }
    }

    fn origins_for(pairs: Vec<(&str, &str)>) -> BTreeMap<String, PathBuf> {
        pairs
            .into_iter()
            .map(|(id, path)| (id.to_string(), PathBuf::from(path)))
            .collect()
    }

    #[test]
    fn tsv_emits_six_tab_separated_columns_per_row() {
        let cfg = cfg_with(vec![
            ("alpha", rule_for(vec!["*.rs", "*.txt"], Some("hint"))),
            ("zeta", rule_for(vec!["*.md"], None)),
        ]);
        let origins = origins_for(vec![
            ("alpha", "/path/to/.hector.yml"),
            ("zeta", "/path/to/parent.yml"),
        ]);
        let out = format_tsv(&cfg, &origins);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // Sorted by id: alpha first.
        assert_eq!(
            lines[0],
            "alpha\tscript\twarning\t*.rs,*.txt\thint\t/path/to/.hector.yml"
        );
        // Empty fix_hint becomes an empty cell, not a missing column.
        assert_eq!(
            lines[1],
            "zeta\tscript\twarning\t*.md\t\t/path/to/parent.yml"
        );
    }

    #[test]
    fn yaml_strips_trust_and_extends_and_inserts_origin_comments() {
        let mut cfg = cfg_with(vec![("alpha", rule_for(vec!["*.rs"], None))]);
        // Even if a Config carries trust/extends, the view must drop them.
        cfg.trust = Some(hector_core::config::TrustBlock {
            fingerprint: "deadbeef".into(),
        });
        cfg.extends = vec!["should-not-leak.yml".into()];
        let origins = origins_for(vec![("alpha", "/path/to/.hector.yml")]);
        let out = format_yaml(&cfg, &origins).unwrap();
        assert!(!out.contains("trust:"));
        assert!(!out.contains("fingerprint:"));
        assert!(!out.contains("extends:"));
        assert!(out.contains("# origin: /path/to/.hector.yml"));
        assert!(out.contains("alpha:"));
    }

    #[test]
    fn json_serializes_rules_sorted_by_id_with_origin() {
        let cfg = cfg_with(vec![
            ("zeta", rule_for(vec!["*.md"], None)),
            ("alpha", rule_for(vec!["*.rs"], None)),
        ]);
        let origins = origins_for(vec![
            ("alpha", "/a/.hector.yml"),
            ("zeta", "/a/parent.yml"),
        ]);
        let out = format_json(&cfg, &origins).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let keys: Vec<&str> = v["rules"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["alpha", "zeta"]);
        assert_eq!(v["rules"]["alpha"]["origin"], "/a/.hector.yml");
        assert_eq!(v["rules"]["zeta"]["origin"], "/a/parent.yml");
        assert!(v.get("trust").is_none());
        assert!(v.get("extends").is_none());
    }

    #[test]
    fn yaml_origin_comment_only_inside_rules_block() {
        // A rule body field happens to be named the same as a rule id —
        // the annotator must not treat it as a new rule.
        let mut cfg = cfg_with(vec![("alpha", rule_for(vec!["*.rs"], None))]);
        // Force a `language:` field in the rule body to ensure the
        // annotator doesn't misinterpret deeper-indented lines.
        if let Some(r) = cfg.rules.get_mut("alpha") {
            r.language = Some("rust".into());
        }
        let origins = origins_for(vec![("alpha", "/p/.hector.yml")]);
        let out = format_yaml(&cfg, &origins).unwrap();
        // Exactly one origin comment for one rule.
        assert_eq!(out.matches("# origin:").count(), 1);
    }
}
```

- [ ] **Step 33: Run the unit tests**

Run: `cargo test -p hector-cli --lib show_resolved_config`
Expected: All four unit tests PASS. (They live in the binary crate's lib-style `mod tests`; clap doesn't get involved.)

- [ ] **Step 34: Commit**

```bash
git add crates/hector-cli/src/commands/show_resolved_config.rs
git commit -m "$(cat <<'EOF'
test(cli): unit tests for show-resolved-config formatters (C3 phase 5)

Cover format_tsv (six-column shape, empty fix_hint preserves cells,
sorted-by-id), format_yaml (strips trust/extends, single origin
comment per rule), and format_json (sorted keys, origin field).
The yaml annotator's "rule body field that looks like a rule key"
edge case is exercised explicitly to lock the parser behavior.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — Documentation

### Task 7.1: Write `docs/show-resolved-config.md`

**Files:**
- Create: `docs/show-resolved-config.md`

- [ ] **Step 35: Author the public-contract doc**

```markdown
# `hector show-resolved-config`

Read-only inspection command. Prints the post-`extends:` merged rule
set so authors can confirm what their actual config looks like after
inheritance.

```bash
hector show-resolved-config [--config .hector.yml] [--format tsv|yaml|json]
```

Exit codes: `0` on success; `1` on config error (missing file, parse
failure, unsupported schema). Never `2` — this command does not run
rules.

This command does **not** verify the trust fingerprint. Operators
typically reach for it precisely when debugging an as-yet-unsigned
config, so trust enforcement would defeat the purpose. The command is
read-only and never executes a `script:` rule.

## Origin attribution

Every rule in the output is annotated with the canonical path of the
file it was *defined in* — your local `.hector.yml`, an
`extends:`-referenced parent, or a deeper transitive ancestor. When a
rule id collides between the local file and an inherited one, the
local definition wins (matching `extends::resolve` semantics) and the
origin reflects that.

## Output: TSV (default)

Columns, in order, separated by a single tab; one rule per line; rows
sorted by rule id; no header row.

| # | Column     | Notes |
|---|------------|-------|
| 1 | `id`       | Rule id from the merged config. |
| 2 | `engine`   | One of `script`, `ast`, `semantic`, `session`. |
| 3 | `severity` | One of `error`, `warning`. |
| 4 | `scope`    | Comma-separated list of glob patterns. No tabs inside the cell. |
| 5 | `fix_hint` | Empty cell when the rule has no fix_hint (column count is preserved). |
| 6 | `origin`   | Canonical filesystem path of the file that defined the rule. |

Greppable / cuttable:

```bash
hector show-resolved-config | cut -f1,2,6     # ids + engine + origin
hector show-resolved-config | grep semantic   # all semantic rules
```

## Output: YAML (`--format yaml`)

Canonical `serde_yaml` rendering of a view that *intentionally* omits
two fields from the live `Config` shape:

- `trust:` is per-config-file. The post-merge view has no single source
  file to fingerprint, so emitting one would mislead.
- `extends:` is already consumed by the merge.

Each rule entry is preceded by a `# origin: <path>` comment line so
the inheritance source is visible in the rendered YAML.

```yaml
schema_version: 2
rules:
  # origin: /work/repo/parent.yml
  inherited:
    description: "from parent"
    engine: script
    scope:
    - "*.txt"
    severity: warning
    script: "true"
  # origin: /work/repo/.hector.yml
  local-only:
    description: "only in child"
    engine: script
    scope:
    - "*.md"
    severity: warning
    script: "true"
```

## Output: JSON (`--format json`)

Pretty-printed `serde_json` rendering of the same view as YAML. Rules
are sorted by id (the `BTreeMap` keys ordering is preserved through
`serde_json::Map`'s insertion order). Each rule object carries an
`origin` field with the canonical defining-file path.

```json
{
  "schema_version": 2,
  "rules": {
    "inherited": {
      "description": "from parent",
      "engine": "script",
      "scope": ["*.txt"],
      "severity": "warning",
      "script": "true",
      "origin": "/work/repo/parent.yml"
    },
    "local-only": {
      "description": "only in child",
      "engine": "script",
      "scope": ["*.md"],
      "severity": "warning",
      "script": "true",
      "origin": "/work/repo/.hector.yml"
    }
  }
}
```

## Stability

These three output shapes are a public contract. TSV column order, the
YAML field set (sans `trust:` / `extends:`), and the JSON object
structure all freeze with this command. Breaking changes go through a
versioned `--format` value (e.g. `--format json-v2`).
```

- [ ] **Step 36: Commit the doc**

```bash
git add docs/show-resolved-config.md
git commit -m "$(cat <<'EOF'
docs: write show-resolved-config output contract (C3 phase 6)

Documents TSV column order, YAML structure (trust/extends stripped,
origin comments), and JSON shape (sorted keys, origin field) so the
three output shapes have a written reference. This locks the public
contract; future format breaks need an explicit versioned --format
value.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 7.2: Add a one-line listing to README.md

**Files:**
- Modify: `README.md`

- [ ] **Step 37: Add an "Inspect" subsection**

Apply this edit (insert after the `## Quick start` section and before `## Specs`):

```markdown
## Inspect

- `hector show-resolved-config [--format tsv|yaml|json]` — print the post-`extends:` merged rule set, with each rule annotated by the file that defined it. See [docs/show-resolved-config.md](docs/show-resolved-config.md).
```

- [ ] **Step 38: Commit the README update**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs(readme): list show-resolved-config under a new Inspect section

Single-bullet pointer to the command and the three output formats,
linking to docs/show-resolved-config.md for the full output contract.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Verification

### Task 8.1: Workspace fmt + clippy + tests

- [ ] **Step 39: Run the full local suite**

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

Expected: every gate passes. Likely friction:
- `clippy::cognitive_complexity` on `format_tsv` if the inline `format!` macro pushes the complexity counter; if it does, decompose into a per-row helper `fn tsv_row(id: &str, rule: &Rule, origin: Option<&Path>) -> String`.
- `clippy::missing_errors_doc` is allowed at workspace level.
- `format_yaml`'s `annotate_yaml_with_origins` is the highest-complexity helper. If clippy complains, split out the rule-key detection into a `fn is_rule_key_line(line: &str) -> Option<&str>` predicate that returns the id when the line is a rule key (and `None` otherwise).

### Task 8.2: Per-file region-coverage gate

- [ ] **Step 40: Run the coverage script**

```bash
bash scripts/ci-coverage.sh
```

Verify each modified or created file under `crates/*/src/` clears the ≥80% region threshold:
- `crates/hector-core/src/config/extends.rs` — the new walker has three new functions; the existing tests in `extends_origin.rs` cover both the single-file and the multi-level paths. If coverage on `merge_inherited_with_origin` drops below 80%, add an extends-chain test where the inherited config's `llm:` and `skip:` fields are populated and the local doesn't define them, which exercises both non-rules merge branches.
- `crates/hector-cli/src/commands/show_resolved_config.rs` — the `#[cfg(test)] mod tests` covers all three formatters and the YAML annotator's rule-key-vs-body discrimination. If coverage gaps remain on `run`'s `Err` arm, add a unit test that constructs a non-existent `Path` and asserts `run` returns `Ok(1)` with the formatter never called.
- `crates/hector-cli/src/cli.rs` — clap derive only; no new logic to cover.
- `crates/hector-cli/src/main.rs` — dispatch arm only; covered by every integration test that exercises the new subcommand.
- `crates/hector-cli/src/commands/mod.rs` — module-list line; trivial.

### Task 8.3: Plan archive

- [ ] **Step 41: When the PR merges**

Move this plan to `plans/archive/2026-05-13-hector-c3-show-resolved-config.md` and update `plans/README.md`'s index.

---

## Acceptance criteria checklist

Mapped to spec §C3 acceptance bullets and the implementation tasks that close them.

- [ ] **C3-(a) Rules inherited from `extends:` parents are marked with their origin.**
  Closed by:
  - Phase 1 — `resolve_with_origin` records every rule's defining file (Tasks 1.1 + 1.2).
  - Phase 2 — TSV's column 6 (Tasks 2.1, 2.3, integration test `tsv_extends_chain_inherits_and_overrides_with_origin`).
  - Phase 3 — YAML's `# origin: <path>` comment line per rule (Tasks 3.1, 3.2).
  - Phase 4 — JSON's `origin` field per rule (Tasks 4.1, 4.2).
  - Phase 6 unit tests assert origin presence in all three formats.

- [ ] **C3-(b) Output sorted by rule id for deterministic diffs.**
  Closed by:
  - `BTreeMap` iteration in `Config.rules` (sorted by key).
  - Defensive `sorted_rules` helper in `format_tsv` (Task 2.3) re-sorts before emitting rows.
  - `BTreeMap<&str, RuleView>` in `ResolvedView` (Task 3.2) preserves sort order through both `serde_yaml` and `serde_json::Map` (insertion-order-preserving).
  - Integration tests `tsv_default_…` and `json_format_emits_sorted_rules_with_origin_field` assert sorted output explicitly.

Beyond the spec's two bullets, this plan also delivers:

- [ ] **TSV column count is stable.** Empty `fix_hint` emits an empty cell. Asserted in both the integration test (`tsv_default_…`) and the unit test (`tsv_emits_six_tab_separated_columns_per_row`).
- [ ] **`trust:` is stripped from YAML and JSON.** Asserted by `yaml_format_emits_canonical_merged_config_without_trust`, `json_format_emits_sorted_rules_with_origin_field`, and the unit test `yaml_strips_trust_and_extends_and_inserts_origin_comments`.
- [ ] **Multiple scope globs do not break TSV column count.** Tested via the `["*.rs", "*.txt"]` rule in `tsv_default_…`; comma-joined into one cell.
- [ ] **Missing config exits 1 with the file path on stderr.** Test `missing_config_exits_one_with_hint`.
- [ ] **Invalid `--format` value rejected by clap before the command runs.** Test `invalid_format_value_is_rejected_by_clap`.
- [ ] **Public output contract documented.** `docs/show-resolved-config.md` (Phase 7) defines the TSV column order, the YAML field set, the JSON object shape, and the stability promise.
- [ ] **README discoverability.** New "Inspect" subsection (Phase 7).

---

## Self-review

- [x] Plan filename matches repo convention `plans/YYYY-MM-DD-hector-<id>-<slug>.md` (`plans/2026-05-13-hector-c3-show-resolved-config.md`), not the writing-plans skill default.
- [x] Header carries the REQUIRED SUB-SKILL note, Goal, Architecture, Tech Stack, and a back-link to `specs/2026-05-12-bully-parity-closures.md §C3`.
- [x] Risk / rollback section near the top covers verdict-schema (none), exit-code (0/1 only, never 2), config-schema (new public TSV/YAML/JSON contract documented in `docs/show-resolved-config.md`), performance (trivial), trust impact (deliberately read-only), backwards-compat (additive).
- [x] Origin-tracking decision is explicit: resolver-side `BTreeMap<String, PathBuf>` returned by a new `resolve_with_origin` entry point, justified against the in-`Config` alternative.
- [x] `trust:` stripping mechanism is explicit: a `ResolvedView` struct that mirrors `Config` minus `trust` and `extends`. Tested in unit and integration tests.
- [x] Every code-step shows the code in full; every test-step shows the test in full; every command-step shows the command + expected outcome.
- [x] No "TBD", "implement later", or "similar to above" placeholders.
- [x] Every spec-acceptance bullet (C3-a, C3-b) is mapped to a numbered task, in the acceptance-criteria checklist.
- [x] TSV column order locked: `id\tengine\tseverity\tscope\tfix_hint\torigin`. Six columns. Documented and asserted.
- [x] YAML output strips `trust:` and `extends:`; origin surfaces as a `# origin: <path>` comment line above each rule. Tested.
- [x] JSON output sorted by id (via `BTreeMap` insertion order); each rule object has an `origin` field; `trust:` / `extends:` absent. Tested.
- [x] Sorted-output requirement covered by both the upstream `BTreeMap` iteration and a defensive `sorted_rules` helper.
- [x] At least one unit test per formatter (`tsv_emits_six_tab_separated_columns_per_row`, `yaml_strips_trust_and_extends_and_inserts_origin_comments`, `json_serializes_rules_sorted_by_id_with_origin`, plus the YAML annotator edge-case test).
- [x] Documentation task adds `docs/show-resolved-config.md` and a README pointer.
- [x] Per-file ≥80% region-coverage gate addressed in Task 8.2 with concrete fallback tests for any file that comes in below threshold.
- [x] Cognitive-complexity cap of 15 addressed: dispatcher is one `match`, formatters are linear, the YAML annotator has a documented split-point if clippy flags it.
- [x] No source-file modifications outside the listed File Structure.
