//! Drift guard: the shipped authoring skills must teach the 0.3 **gates**
//! model, never the retired pre-0.3 engine/severity/rules model.

use std::path::PathBuf;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> String {
    let path = repo_path(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Every authoring-related skill file shipped in the tree.
const SKILL_FILES: &[&str] = &[
    "adapters/shared/hector-config/SKILL.md",
    "adapters/claude-code/skills/hector/SKILL.md",
    "adapters/claude-code/skills/hector-init/SKILL.md",
    "adapters/claude-code/skills/hector-review/SKILL.md",
];

const RETIRED_TOKENS: &[&str] = &[
    "engine:",
    "severity",
    "rule_id",
    "passed_checks",
    "violations",
    "{file}",
    "capabilities:",
    "hector migrate",
];

#[test]
fn skills_contain_no_retired_engine_model_vocabulary() {
    for rel in SKILL_FILES {
        let body = read(rel);
        for token in RETIRED_TOKENS {
            assert!(
                !body.contains(token),
                "{rel} still teaches the retired model: contains `{token}`"
            );
        }
    }
}

#[test]
fn shared_guide_teaches_the_two_field_gate() {
    let body = read("adapters/shared/hector-config/SKILL.md");
    assert!(
        body.contains("name: hector-config"),
        "shared guide must carry Agent-Skills frontmatter `name: hector-config`"
    );
    for anchor in ["$HECTOR_FILE", "run:", "files:", "exit 2"] {
        assert!(
            body.contains(anchor),
            "shared guide must teach the gates model: missing `{anchor}`"
        );
    }
}

#[test]
fn hector_author_skill_is_retired() {
    // The hand-maintained authoring skill was consolidated into the shared
    // guide; its file must be gone so there is no second source to drift.
    assert!(
        !repo_path("adapters/claude-code/skills/hector-author/SKILL.md").exists(),
        "hector-author/SKILL.md must be removed (consolidated into adapters/shared/hector-config)"
    );
}

#[test]
fn runtime_skill_describes_the_gates_verdict_shape() {
    let body = read("adapters/claude-code/skills/hector/SKILL.md");
    assert!(
        body.contains("blocks"),
        "hector/SKILL.md must describe the `blocks` verdict array"
    );
    assert!(
        body.contains("\"gate\""),
        "hector/SKILL.md must key a block by `gate`"
    );
}
