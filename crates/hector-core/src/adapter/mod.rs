//! Harness onboarding: materialize Hector's hook into supported coding agents.
mod materialize;
pub use materialize::{
    atomic_write, backup_once, read_sidecar, sha256_hex, sidecar_path, write_sidecar,
    AdapterSidecar,
};
