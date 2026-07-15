use super::DoctorContext;
use ironlint_core::adapter::AdapterEnv;
use std::path::Path;

mod adapters;
mod config;
mod report;

pub(super) fn ctx_with(dir: &Path) -> DoctorContext {
    DoctorContext {
        dir: dir.to_path_buf(),
        config_path: dir.join(".ironlint.yml"),
    }
}

pub(super) fn adapter_env(tmp: &Path) -> AdapterEnv {
    AdapterEnv {
        home: tmp.to_path_buf(),
        config_home: tmp.join(".config"),
        project_root: tmp.join("proj"),
    }
}
