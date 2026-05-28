use hector_core::verdict::{Verdict, SCHEMA_VERSION};

/// Additive fields (skip_serializing_if defaulted) must NOT bump
/// SCHEMA_VERSION. Adding `deferred_rules` is additive, so the version stays
/// at 2 — pinned here.
#[test]
fn schema_version_is_2_after_additive_r6_change() {
    assert_eq!(
        SCHEMA_VERSION, 2,
        "additive fields do not bump SCHEMA_VERSION"
    );
    let v = Verdict::pass();
    assert_eq!(v.schema_version, 2);
}
