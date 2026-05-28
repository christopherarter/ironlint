use hector_core::trust::{canonicalize_for_fingerprint, fingerprint};

/// Regression: the canonical output the fingerprint hashes must be JSON, not
/// YAML. `serde_yaml::to_string` is not normative — scalar style and indent
/// width drift across serde_yaml versions, so a cargo update could invalidate
/// every checked-in fingerprint with no actual config change. Canonicalizing
/// through `serde_json::Value` → `serde_json::to_string` is normative per
/// RFC 8259.
///
/// Pin: the canonical string parses as JSON and starts with `{` (a JSON
/// object literal).
#[test]
fn canonical_output_is_json_not_yaml() {
    let cfg = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let canonical = canonicalize_for_fingerprint(cfg).expect("canonicalize");
    // Canonical output must be RFC 8259 JSON and round-trip through serde_json.
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

/// The same semantic content in block-style and flow-style YAML must hash
/// identically — both parse to the same value, and the canonical form must
/// preserve that alpha-equivalence. Pinned so a future canonicalization
/// change can't regress it.
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

/// Unsupported YAML features (binary scalars, anchor references) must error
/// at fingerprint time with a clear message rather than produce a fragile hash.
#[test]
fn fingerprint_rejects_anchor_reference() {
    let with_anchor = "schema_version: 2\nrules:\n  base: &b\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n  alias: *b\n";
    let result = fingerprint(with_anchor);
    assert!(result.is_err(), "anchors must be rejected; got {result:?}");
}
