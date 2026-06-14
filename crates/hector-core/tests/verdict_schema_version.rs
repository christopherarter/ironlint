use hector_core::verdict::{Verdict, SCHEMA_VERSION};

/// Shape-breaking changes bump SCHEMA_VERSION. v3 removed the
/// `deferred_rules` field and the `semantic`/`session` `Engine` variants —
/// pinned here so the bump stays deliberate.
#[test]
fn schema_version_is_3() {
    assert_eq!(
        SCHEMA_VERSION, 3,
        "shape-breaking removals bump SCHEMA_VERSION"
    );
    let v = Verdict::pass();
    assert_eq!(v.schema_version, 3);
}
