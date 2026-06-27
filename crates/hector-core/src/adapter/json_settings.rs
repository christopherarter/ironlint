use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchResult {
    Added,
    AlreadyPresent,
}

/// True if any string anywhere in `v` contains `marker`.
fn contains_marker(v: &Value, marker: &str) -> bool {
    match v {
        Value::String(s) => s.contains(marker),
        Value::Array(a) => a.iter().any(|e| contains_marker(e, marker)),
        Value::Object(o) => o.values().any(|e| contains_marker(e, marker)),
        _ => false,
    }
}

/// Mutable reference to `settings["hooks"][key]` as an array, creating the
/// `hooks` object and the array if missing.
fn hook_array<'a>(settings: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !settings.is_object() {
        *settings = Value::Object(serde_json::Map::new());
    }
    let obj = settings.as_object_mut().expect("just ensured object");
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(serde_json::Map::new());
    }
    let arr = hooks
        .as_object_mut()
        .expect("just ensured object")
        .entry(key)
        .or_insert_with(|| Value::Array(Vec::new()));
    if !arr.is_array() {
        *arr = Value::Array(Vec::new());
    }
    arr.as_array_mut().expect("just ensured array")
}

/// Insert `desired` into `settings.hooks[key]`, replacing any stale hector entry.
///
/// Identified by `marker`. Idempotent: if the only hector entry already equals
/// `desired`, returns `AlreadyPresent`.
pub fn sync_hook_array(
    settings: &mut Value,
    key: &str,
    desired: Value,
    marker: &str,
) -> PatchResult {
    let arr = hook_array(settings, key);
    let hector: Vec<&Value> = arr.iter().filter(|e| contains_marker(e, marker)).collect();
    if hector.len() == 1 && hector[0] == &desired {
        return PatchResult::AlreadyPresent;
    }
    arr.retain(|e| !contains_marker(e, marker));
    arr.push(desired);
    PatchResult::Added
}

/// Remove every hector-owned entry from `settings.hooks[key]`. Returns whether
/// anything was removed.
pub fn remove_from_hook_array(settings: &mut Value, key: &str, marker: &str) -> bool {
    let arr = hook_array(settings, key);
    let before = arr.len();
    arr.retain(|e| !contains_marker(e, marker));
    arr.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claude_entry(cmd: &str) -> serde_json::Value {
        json!({"matcher": "Edit|Write",
               "hooks": [{"type": "command", "command": cmd}]})
    }

    #[test]
    fn sync_inserts_into_empty_settings() {
        let mut s = json!({});
        let cmd = "\"/h/adapters/claude-code/hook.sh\" post-tool-use";
        let r = sync_hook_array(
            &mut s,
            "PostToolUse",
            claude_entry(cmd),
            "/h/adapters/claude-code/",
        );
        assert!(matches!(r, PatchResult::Added));
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn sync_is_idempotent_for_identical_entry() {
        let cmd = "\"/h/adapters/reasonix/hook.sh\" pre-tool-use";
        let entry = json!({"command": cmd, "match": "^(write_file|edit_file|multi_edit)$",
                           "description": "hector", "timeout": 30000});
        let mut s = json!({});
        sync_hook_array(&mut s, "PreToolUse", entry.clone(), "/h/adapters/reasonix/");
        let r = sync_hook_array(&mut s, "PreToolUse", entry, "/h/adapters/reasonix/");
        assert!(matches!(r, PatchResult::AlreadyPresent));
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn sync_strips_stale_hector_entry_and_keeps_foreign() {
        let mut s = json!({"hooks": {"PreToolUse": [
            {"command": "\"/h/adapters/reasonix/hook.sh\" pre-tool-use", "match": "old"},
            {"command": "other-tool guard", "match": "x"}
        ]}});
        let new_cmd = "\"/h/adapters/reasonix/hook.sh\" pre-tool-use";
        let entry = json!({"command": new_cmd, "match": "^(write_file|edit_file|multi_edit)$"});
        let r = sync_hook_array(&mut s, "PreToolUse", entry, "/h/adapters/reasonix/");
        assert!(matches!(r, PatchResult::Added));
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2); // foreign kept, single hector entry refreshed
        assert!(arr.iter().any(|e| e["command"] == "other-tool guard"));
        assert!(arr
            .iter()
            .any(|e| e["match"] == "^(write_file|edit_file|multi_edit)$"));
    }

    #[test]
    fn remove_drops_only_hector_entries() {
        let mut s = json!({"hooks": {"PostToolUse": [
            claude_entry("\"/h/adapters/claude-code/hook.sh\" post-tool-use"),
            {"matcher": "Edit", "hooks": [{"type": "command", "command": "keep me"}]}
        ]}});
        let removed = remove_from_hook_array(&mut s, "PostToolUse", "/h/adapters/claude-code/");
        assert!(removed);
        let arr = s["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "keep me");
    }

    #[test]
    fn remove_returns_false_when_absent() {
        let mut s = json!({"hooks": {"PostToolUse": []}});
        assert!(!remove_from_hook_array(
            &mut s,
            "PostToolUse",
            "/h/adapters/claude-code/"
        ));
    }
}
