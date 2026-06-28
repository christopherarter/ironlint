use hector_core::verdict::{Verdict, MIN_REQUIRED_SCHEMA_VERSION, SCHEMA_VERSION};

/// Shape-breaking changes bump SCHEMA_VERSION. v5 renames `gate`→`check`, adds
/// nullable `step` and `file` fields — pinned here so the bump stays deliberate.
#[test]
fn schema_version_is_5() {
    assert_eq!(
        SCHEMA_VERSION, 5,
        "checks pipeline redesign bumps schema to 5"
    );
    let v = Verdict::pass();
    assert_eq!(v.schema_version, 5);
}

#[test]
fn min_required_schema_version_is_4() {
    assert_eq!(MIN_REQUIRED_SCHEMA_VERSION, 4);
}
