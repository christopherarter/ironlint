use ironlint_core::arch::lowering::lower_architecture;
use ironlint_core::config::Config;

#[test]
fn lowers_architecture_to_synthetic_check() {
    let mut cfg: Config = serde_yaml::from_str(
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\n  rules:\n    - from: data\n      may_import: []\nchecks:\n  g:\n    files: \"*\"\n    run: \"true\"\n",
    )
    .unwrap();
    lower_architecture(&mut cfg).unwrap();
    let arch = cfg
        .checks
        .get("__arch__")
        .expect("synthetic __arch__ check inserted");
    assert!(arch.run.as_deref().unwrap().contains("ironlint arch check"));
    assert!(arch.files.iter().any(|f| f == "**/*"));
    assert!(cfg.arch_layers_yaml.is_some());
}

#[test]
fn rejects_user_owned_arch_check_id() {
    let mut cfg: Config = serde_yaml::from_str(
        "architecture:\n  layers:\n    - name: data\n      globs: [\"src/data/**\"]\nchecks:\n  __arch__:\n    files: \"*\"\n    run: \"false\"\n",
    )
    .unwrap();
    let err = lower_architecture(&mut cfg).unwrap_err().to_string();
    assert!(err.contains("reserved check id `__arch__`"), "{err}");
}

#[test]
fn no_architecture_block_is_noop() {
    let mut cfg: Config =
        serde_yaml::from_str("checks:\n  g:\n    files: \"*\"\n    run: \"true\"\n").unwrap();
    lower_architecture(&mut cfg).unwrap();
    assert!(!cfg.checks.contains_key("__arch__"));
    assert!(cfg.arch_layers_yaml.is_none());
}
