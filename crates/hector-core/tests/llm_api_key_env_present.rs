//! Narrow public probe so `hector doctor` can report API-key presence using
//! the same emptiness rule the runner uses internally.

use hector_core::llm::api_key_env_present;

#[test]
fn missing_env_var_is_absent() {
    // Pick a name vanishingly unlikely to collide with a real env var.
    let name = "HECTOR_DOCTOR_TEST_MISSING_VAR_THAT_DOES_NOT_EXIST";
    assert!(!api_key_env_present(name));
}

#[test]
fn empty_env_var_is_absent() {
    let name = "HECTOR_DOCTOR_TEST_EMPTY";
    std::env::set_var(name, "");
    assert!(!api_key_env_present(name));
    std::env::remove_var(name);
}

#[test]
fn nonempty_env_var_is_present() {
    let name = "HECTOR_DOCTOR_TEST_PRESENT";
    std::env::set_var(name, "x");
    assert!(api_key_env_present(name));
    std::env::remove_var(name);
}
