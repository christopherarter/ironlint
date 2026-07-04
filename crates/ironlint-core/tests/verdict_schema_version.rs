use ironlint_core::verdict::{Verdict, MIN_REQUIRED_SCHEMA_VERSION, SCHEMA_VERSION};

/// Shape-breaking changes bump SCHEMA_VERSION. v6 adds `GateError.detail`;
/// v5 renamed `gate`→`check` and added nullable `step`/`file` — pinned here so
/// the bump stays deliberate.
#[test]
fn schema_version_is_6() {
    assert_eq!(SCHEMA_VERSION, 6, "InternalError detail bumps schema to 6");
    let v = Verdict::pass();
    assert_eq!(v.schema_version, 6);
}

#[test]
fn min_required_schema_version_is_4() {
    assert_eq!(MIN_REQUIRED_SCHEMA_VERSION, 4);
}
