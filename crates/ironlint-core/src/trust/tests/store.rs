use super::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[test]
fn store_path_joins_under_config_home() {
    let p = store_path_in(Path::new("/home/u/.config"));
    assert_eq!(p, Path::new("/home/u/.config/ironlint/trust.json"));
}

#[test]
fn read_missing_store_is_empty_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = read_store(&dir.path().join("trust.json")).unwrap();
    assert!(store.entries.is_empty());
}

#[test]
fn write_then_read_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/trust.json"); // parent must be created
    let mut store = TrustStore {
        version: TRUST_STORE_VERSION,
        ..Default::default()
    };
    store.entries.insert(
        "/abs/.ironlint.yml".to_string(),
        TrustEntry {
            hash: "sha256:abc".into(),
            blessed_at: "2026-06-24T00:00:00Z".into(),
        },
    );
    write_store(&path, &store).unwrap();
    let back = read_store(&path).unwrap();
    assert_eq!(back.entries["/abs/.ironlint.yml"].hash, "sha256:abc");
    assert_eq!(
        back.entries["/abs/.ironlint.yml"].blessed_at,
        "2026-06-24T00:00:00Z"
    );
    assert_eq!(back.version, TRUST_STORE_VERSION);
}

#[test]
fn xdg_config_home_overrides_home() {
    // config_home() prefers XDG_CONFIG_HOME. Test the pure resolver with an
    // explicit value rather than mutating process env.
    assert_eq!(
        config_home_from(Some("/x".into()), Some("/h".into())),
        Some(PathBuf::from("/x"))
    );
    assert_eq!(
        config_home_from(None, Some("/h".into())),
        Some(PathBuf::from("/h/.config"))
    );
    // An empty XDG_CONFIG_HOME is treated as unset and falls through to HOME.
    assert_eq!(
        config_home_from(Some(String::new()), Some("/h".into())),
        Some(PathBuf::from("/h/.config"))
    );
    assert_eq!(config_home_from(None, None), None);
}

#[test]
fn read_store_surfaces_non_notfound_errors() {
    // A path that exists but is a directory makes read_to_string fail with a
    // kind other than NotFound — that must propagate as Err, not be swallowed
    // into an empty store.
    let dir = tempfile::tempdir().unwrap();
    assert!(read_store(dir.path()).is_err());
}

#[test]
fn unique_tmp_path_differs_across_calls() {
    let base = Path::new("/x/trust.json");
    let a = unique_tmp_path(base);
    let b = unique_tmp_path(base);
    assert_ne!(a, b, "temp names must be unique per write");
}

#[test]
fn classify_store_read_permission_denied_propagates_err() {
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("trust.json");
    let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    assert!(classify_store_read(&store_path, Err(err)).is_err());
}

#[test]
fn classify_store_read_not_found_is_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("trust.json");
    let err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let store = classify_store_read(&store_path, Err(err)).unwrap();
    assert!(store.entries.is_empty());
}

#[test]
fn classify_store_read_invalid_json_is_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("trust.json");
    let store = classify_store_read(&store_path, Ok("{ not json".to_string())).unwrap();
    assert!(store.entries.is_empty());
}

// --- store v2 (Task 3) -----------------------------------------------
#[test]
fn version_one_store_deserializes_with_empty_worktree_entries() {
    let v1 =
        r#"{"version":1,"entries":{"\/x\/.ironlint.yml":{"hash":"sha256:ab","blessed_at":"t"}}}"#;
    let store: TrustStore = serde_json::from_str(v1).unwrap();
    assert_eq!(store.version, 1);
    assert_eq!(store.entries.len(), 1);
    assert!(
        store.worktree_entries.is_empty(),
        "v1 store gets empty worktree_entries"
    );
}

#[test]
fn worktree_entries_round_trip() {
    let mut store = TrustStore::default();
    store.worktree_entries.insert("/common/.git".to_string(), {
        let mut inner = BTreeMap::new();
        inner.insert(
            ".ironlint.yml".to_string(),
            TrustEntry {
                hash: "sha256:cd".to_string(),
                blessed_at: "t".to_string(),
            },
        );
        inner
    });
    let json = serde_json::to_string(&store).unwrap();
    let back: TrustStore = serde_json::from_str(&json).unwrap();
    assert_eq!(back.worktree_entries, store.worktree_entries);
}

#[test]
fn trust_store_version_is_two() {
    assert_eq!(TRUST_STORE_VERSION, 2);
}
