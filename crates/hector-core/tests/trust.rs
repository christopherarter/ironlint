use hector_core::trust::{canonicalize_for_fingerprint, fingerprint, verify};

const CFG_A: &str = "\
schema_version: 2
rules:
  r:
    description: \"x\"
    engine: script
    scope: [\"*\"]
    severity: error
    script: \"true\"
trust:
  fingerprint: \"sha256:placeholder\"
";

const CFG_A_REORDERED: &str = "\
trust:
  fingerprint: \"sha256:other\"
rules:
  r:
    severity: error
    scope: [\"*\"]
    script: \"true\"
    description: \"x\"
    engine: script
schema_version: 2
";

#[test]
fn fingerprint_ignores_key_order_and_trust_block() {
    let a = fingerprint(CFG_A).unwrap();
    let b = fingerprint(CFG_A_REORDERED).unwrap();
    assert_eq!(
        a, b,
        "canonicalization must ignore key order and trust block"
    );
    assert!(a.starts_with("sha256:"));
}

#[test]
fn fingerprint_detects_semantic_changes() {
    let modified = CFG_A.replace("engine: script", "engine: ast");
    let a = fingerprint(CFG_A).unwrap();
    let b = fingerprint(&modified).unwrap();
    assert_ne!(a, b);
}

#[test]
fn verify_accepts_matching_fingerprint() {
    // Compute fingerprint of a config body (without trust block), then embed it.
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let fp = fingerprint(body).unwrap();
    let cfg = format!("{body}trust:\n  fingerprint: \"{fp}\"\n");
    assert!(
        verify(&cfg).is_ok(),
        "self-consistent fingerprint should verify"
    );

    // Sanity: canonicalization function is callable.
    let _ = canonicalize_for_fingerprint(body).unwrap();
}

#[test]
fn verify_rejects_missing_trust_block() {
    let cfg = "schema_version: 2\nrules: {}\n";
    let result = verify(cfg);
    assert!(result.is_err());
}
