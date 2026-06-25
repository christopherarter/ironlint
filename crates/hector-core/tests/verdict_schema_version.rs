use hector_core::verdict::{Verdict, MIN_REQUIRED_SCHEMA_VERSION, SCHEMA_VERSION};

/// Shape-breaking changes bump SCHEMA_VERSION. v4 removes Violation/Severity/Engine
/// and introduces blocks/errors — pinned here so the bump stays deliberate.
#[test]
fn schema_version_is_4() {
    assert_eq!(SCHEMA_VERSION, 4, "gates redesign bumps schema to 4");
    let v = Verdict::pass();
    assert_eq!(v.schema_version, 4);
}

#[test]
fn min_required_schema_version_is_4() {
    assert_eq!(MIN_REQUIRED_SCHEMA_VERSION, 4);
}
