use hector_core::verdict::{Verdict, SCHEMA_VERSION};

/// C6: additive fields (skip_serializing_if defaulted) must NOT bump
/// SCHEMA_VERSION. R6 added `deferred_rules` and (incorrectly) bumped
/// 2 → 3. Pin the corrected value here.
#[test]
fn schema_version_is_2_after_additive_r6_change() {
    assert_eq!(
        SCHEMA_VERSION, 2,
        "additive fields do not bump SCHEMA_VERSION"
    );
    let v = Verdict::pass();
    assert_eq!(v.schema_version, 2);
}
