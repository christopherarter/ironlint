use hector_core::trust::{canonicalize_for_fingerprint, fingerprint, verify, write_trust_block};

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

/// Regression: `hector trust` must not destroy comments.
///
/// Round-tripping through `serde_yaml` would drop every comment and normalize
/// scalar style. Instead the writer does a string-level edit that locates the
/// existing `trust:` block (or appends one at EOF) and rewrites only that
/// block, leaving the rest of the file verbatim.
#[test]
fn comments_preserved_through_trust_write() {
    let original = "\
# top-level rationale: this codebase forbids prints in production
schema_version: 2
rules:
  no-print:
    # imports get audited by review, not by hector
    description: \"x\"
    engine: script
    scope: [\"*.py\"]
    severity: error
    script: \"true\" # idempotent placeholder
";
    let out = write_trust_block(original).expect("write_trust_block");
    assert!(
        out.contains("# top-level rationale: this codebase forbids prints in production"),
        "leading comment preserved; got:\n{out}"
    );
    assert!(
        out.contains("# imports get audited by review, not by hector"),
        "inline-rule comment preserved; got:\n{out}"
    );
    assert!(
        out.contains("# idempotent placeholder"),
        "trailing inline comment preserved; got:\n{out}"
    );
    assert!(out.contains("trust:"), "trust block written");
    assert!(out.contains("sha256:"), "fingerprint written");
    // Newly trusted file must still verify against itself.
    verify(&out).expect("trust round-trips");
}

/// Regression: rewriting an existing trust block in place must preserve
/// surrounding comments and structure.
#[test]
fn trust_rewrite_in_place_preserves_surrounding_lines() {
    let original = "\
# header comment
schema_version: 2
trust:
  fingerprint: \"sha256:stale\"
rules:
  r:
    description: \"x\"
    engine: script
    scope: [\"*\"]
    severity: error
    script: \"true\"
# trailing comment
";
    let out = write_trust_block(original).expect("write_trust_block");
    assert!(out.contains("# header comment"), "header preserved:\n{out}");
    assert!(
        out.contains("# trailing comment"),
        "trailing preserved:\n{out}"
    );
    assert!(
        !out.contains("sha256:stale"),
        "stale fingerprint replaced:\n{out}"
    );
    // Must verify against itself.
    verify(&out).expect("trust round-trips after rewrite");
}

/// Regression: when there is no existing `trust:` block, write one at EOF
/// without disturbing the body.
#[test]
fn trust_appended_when_block_absent_preserves_body() {
    let original = "\
# important
schema_version: 2
rules:
  r:
    description: \"x\"
    engine: script
    scope: [\"*\"]
    severity: error
    script: \"true\"
";
    let out = write_trust_block(original).expect("write_trust_block");
    assert!(out.starts_with("# important\n"), "body intact:\n{out}");
    assert!(out.contains("\ntrust:\n"), "trust appended:\n{out}");
    verify(&out).expect("trust round-trips");
}

/// Regression: no TOCTOU between trust verify and config parse.
///
/// Reading the file twice — once for `trust::verify`, once for `parse` —
/// opens a window for an attacker with write access to swap the file between
/// reads. The loader (`extends::resolve_trusted`) instead reads the file once
/// and passes the same in-memory buffer to both `trust::verify` and
/// `parse_str`.
///
/// End-to-end: after a successful trusted load, swap the file to a body that
/// mismatches its (preserved) trust fingerprint. A subsequent load must
/// reject — proving the loader re-reads fresh on every load *and* checks the
/// same bytes it parses.
#[test]
fn p2_3_load_rejects_when_body_diverges_from_trust_fingerprint() {
    use hector_core::runner::HectorEngine;
    use hector_core::trust::write_trust_block;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".hector.yml");

    // 1. Write a trusted config; load must succeed.
    let body_a = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let trusted_a = write_trust_block(body_a).unwrap();
    std::fs::write(&path, &trusted_a).unwrap();
    HectorEngine::load(&path).expect("trusted load succeeds");

    // 2. Extract the trust block from the trusted file and graft it onto a
    //    DIFFERENT body. The on-disk file now has a valid-looking trust block
    //    but a body that does not match — exactly the shape of a TOCTOU attack
    //    (swap content while keeping fingerprint headers).
    let trust_line = trusted_a
        .lines()
        .skip_while(|l| !l.starts_with("trust:"))
        .collect::<Vec<_>>()
        .join("\n");
    let body_b = "schema_version: 2\nrules:\n  evil:\n    description: \"diverged\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"touch /tmp/PWNED\"\n";
    let attacker_payload = format!("{body_b}{trust_line}\n");
    std::fs::write(&path, &attacker_payload).unwrap();

    // 3. Load must reject — proving the runner verifies the bytes it parses.
    let result = HectorEngine::load(&path);
    let err = match result {
        Ok(_) => panic!("loader must reject body/trust mismatch"),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        err.contains("trust") || err.contains("fingerprint"),
        "error must reference trust; got: {err}"
    );
}

/// Regression: appending new YAML structure after the `trust:` block must
/// trip the gate.
///
/// The fingerprint is computed by parsing the YAML and re-serializing a
/// canonical form before hashing, so any new mapping entry (a fresh top-level
/// field, or a new rule under `rules:`) becomes part of the canonical form and
/// changes the fingerprint. An attacker-controlled `engine: script` rule
/// appended after `trust:` MUST cause `verify` to fail.
#[test]
fn verify_rejects_new_rule_appended_after_trust_block() {
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = write_trust_block(body).expect("sign");
    verify(&trusted).expect("sanity: freshly-signed config verifies");

    // Attacker appends a brand-new top-level key after `trust:`.
    let attack_toplevel = format!("{trusted}attacker_field: \"pwned\"\n");
    assert!(
        verify(&attack_toplevel).is_err(),
        "appending a new top-level key after trust: must trip the gate"
    );

    // Attacker appends a new rule by re-opening `rules:` after the trust block.
    // YAML merges duplicate top-level keys differently across parsers; serde_yaml
    // accepts the duplicate and the second value wins. Either way, the parsed
    // map differs from the signed body, so the fingerprint must mismatch.
    let attack_rule = format!(
        "{trusted}rules:\n  evil:\n    description: \"pwned\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"touch /tmp/PWNED\"\n"
    );
    // serde_yaml rejects duplicate top-level keys outright in some versions;
    // either a parse error or a fingerprint mismatch is acceptable — both
    // prevent the malicious rule from running.
    let result = verify(&attack_rule);
    assert!(
        result.is_err(),
        "appending a duplicate `rules:` block with an attacker rule must not pass the trust gate"
    );
}

/// Pure YAML comments appended after `trust:` do NOT trip the gate, and that
/// is acceptable: comments are stripped at parse time by serde_yaml, carry no
/// executable payload, and cannot smuggle a `script:` rule past the
/// canonicalization step. Pinned so a future change to the canonicalization
/// path can't quietly alter the contract.
#[test]
fn verify_accepts_yaml_comment_appended_after_trust_block() {
    let body = "schema_version: 2\nrules:\n  r:\n    description: \"x\"\n    engine: script\n    scope: [\"*\"]\n    severity: error\n    script: \"true\"\n";
    let trusted = write_trust_block(body).expect("sign");
    verify(&trusted).expect("sanity: freshly-signed config verifies");

    let with_trailing_comment = format!("{trusted}# attacker-added comment, semantically inert\n");
    assert!(
        verify(&with_trailing_comment).is_ok(),
        "pure comments are stripped at parse time and do not affect the fingerprint; \
         they carry no script payload so this is acceptable"
    );
}
