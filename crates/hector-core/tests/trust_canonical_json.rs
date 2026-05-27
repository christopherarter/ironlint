use hector_core::trust::{canonicalize_for_fingerprint, fingerprint};

/// C1 regression: the canonical output the fingerprint hashes must be
/// JSON, not YAML. The pre-fix algorithm routed through
/// `serde_yaml::to_string`, whose output is not normative (scalar style
/// and indent width drifted across serde_yaml 0.8/0.9/0.10 — a cargo
/// update could invalidate every checked-in fingerprint with no actual
/// config change). The fix routes through `serde_json::Value` →
/// `serde_json::to_string`, which is normative per RFC 8259.
///
/// Pin: the canonical string parses as JSON and starts with `{` (a JSON
/// object literal). The old YAML-emitter output (`schema_version: 2\n…`)
/// is not valid JSON.
#[test]
fn canonical_output_is_json_not_yaml() {
    let cfg = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let canonical = canonicalize_for_fingerprint(cfg).expect("canonicalize");
    // Pre-fix: canonical was YAML and would NOT round-trip through
    // serde_json. Post-fix: canonical is RFC 8259 JSON and does.
    let parsed: serde_json::Value =
        serde_json::from_str(&canonical).expect("canonical output must be valid JSON");
    assert!(
        canonical.starts_with('{'),
        "canonical output must be a JSON object literal; got: {canonical:?}"
    );
    // And the parsed shape is the config sans the trust block.
    assert!(parsed.get("schema_version").is_some());
    assert!(parsed.get("rules").is_some());
    assert!(
        parsed.get("trust").is_none(),
        "trust block must be stripped before canonicalization"
    );
}

/// C1: the same semantic content in block-style and flow-style YAML
/// must hash identically. (This was true under the old emitter too — both
/// styles parse to the same `serde_yaml::Value` — but it's the
/// "alpha-equivalence" property the canonical form should preserve and
/// is worth pinning so a future canonicalization change can't regress
/// it.)
#[test]
fn fingerprint_stable_across_yaml_styles() {
    let block = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let flow = "{schema_version: 2, rules: {r: {description: \"x\", engine: script, scope: [\"*\"], severity: error, script: \"true\"}}}";
    let fp_block = fingerprint(block).expect("block");
    let fp_flow = fingerprint(flow).expect("flow");
    assert_eq!(
        fp_block, fp_flow,
        "semantic equality must yield same fingerprint"
    );
}

/// C1: unsupported YAML features (binary scalars, anchor references)
/// must error at fingerprint time with a clear message instead of
/// silently producing a fragile hash.
#[test]
fn fingerprint_rejects_anchor_reference() {
    let with_anchor = "schema_version: 2\nrules:\n  base: &b\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n  alias: *b\n";
    let result = fingerprint(with_anchor);
    assert!(result.is_err(), "anchors must be rejected; got {result:?}");
}
