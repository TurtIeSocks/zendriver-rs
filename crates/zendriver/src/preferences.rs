//! Chrome profile `Preferences` writer: merges suppression prefs (and user
//! overrides) into `<user_data_dir>/Default/Preferences` at launch. Default
//! suppression is written only for a port-owned (temp) profile; a
//! user-supplied profile is left untouched unless explicit prefs are given.

use serde_json::{Value, json};

/// The default popup-suppression preference set (password manager + autofill).
/// Written for port-owned profiles. Dotted keys expand to nested objects.
pub(crate) fn default_suppression() -> Vec<(String, Value)> {
    vec![
        ("credentials_enable_service".into(), json!(false)),
        ("profile.password_manager_enabled".into(), json!(false)),
        ("profile.password_manager_leak_detection".into(), json!(false)),
        ("autofill.profile_enabled".into(), json!(false)),
        ("autofill.credit_card_enabled".into(), json!(false)),
    ]
}

/// Merge `prefs` (dotted keys, last-wins) into `base` (a JSON object, or `{}`
/// if not an object). Returns the merged object. Existing unrelated keys are
/// preserved.
pub(crate) fn merge_preferences(mut base: Value, prefs: &[(String, Value)]) -> Value {
    if !base.is_object() {
        base = json!({});
    }
    for (key, val) in prefs {
        set_dotted(&mut base, key, val.clone());
    }
    base
}

fn set_dotted(root: &mut Value, dotted: &str, val: Value) {
    let parts: Vec<&str> = dotted.split('.').collect();
    let mut cur = root;
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !cur.is_object() {
            *cur = json!({});
        }
        let Some(obj) = cur.as_object_mut() else {
            return;
        };
        cur = obj.entry((*part).to_string()).or_insert_with(|| json!({}));
    }
    if !cur.is_object() {
        *cur = json!({});
    }
    if let (Some(obj), Some(last)) = (cur.as_object_mut(), parts.last()) {
        obj.insert((*last).to_string(), val);
    }
}

use std::path::Path;

/// Write the resolved prefs into `<user_data_dir>/Default/Preferences`,
/// merging with any existing file. `owned` = the port created this profile
/// (temp dir) → the default suppression set is included; a user-supplied
/// profile (`owned == false`) gets ONLY `user_prefs` (and nothing at all if
/// `user_prefs` is empty). Best-effort: any IO/parse failure is logged and
/// ignored (flags still suppress at the flag level).
pub(crate) fn write_preferences(
    user_data_dir: &Path,
    owned: bool,
    user_prefs: &[(String, Value)],
) {
    let mut prefs: Vec<(String, Value)> = Vec::new();
    if owned {
        prefs.extend(default_suppression());
    } else if user_prefs.is_empty() {
        return; // supplied profile + no explicit prefs → don't touch it
    }
    prefs.extend(user_prefs.iter().cloned());

    let default_dir = user_data_dir.join("Default");
    let path = default_dir.join("Preferences");
    let base = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}));
    let merged = merge_preferences(base, &prefs);
    if let Err(e) = std::fs::create_dir_all(&default_dir)
        .and_then(|()| std::fs::write(&path, merged.to_string()))
    {
        tracing::warn!(error = %e, path = %path.display(),
            "failed to write Chrome Preferences; relying on flag-level suppression");
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn dotted_key_expands_to_nested() {
        let out = merge_preferences(
            json!({}),
            &[("profile.password_manager_enabled".into(), json!(false))],
        );
        assert_eq!(out["profile"]["password_manager_enabled"], json!(false));
    }

    #[test]
    fn merge_preserves_existing_keys() {
        let base = json!({ "foo": 1, "profile": { "name": "bob" } });
        let out = merge_preferences(
            base,
            &[("profile.password_manager_enabled".into(), json!(false))],
        );
        assert_eq!(out["foo"], json!(1));
        assert_eq!(out["profile"]["name"], json!("bob")); // sibling preserved
        assert_eq!(out["profile"]["password_manager_enabled"], json!(false));
    }

    #[test]
    fn later_pref_overrides_earlier() {
        let out = merge_preferences(
            json!({}),
            &[
                ("credentials_enable_service".into(), json!(false)),
                ("credentials_enable_service".into(), json!(true)), // user override wins
            ],
        );
        assert_eq!(out["credentials_enable_service"], json!(true));
    }

    #[test]
    fn non_object_base_becomes_object() {
        let out = merge_preferences(json!("garbage"), &[("a".into(), json!(1))]);
        assert_eq!(out["a"], json!(1));
    }

    #[test]
    fn non_object_intermediate_is_overwritten() {
        let base = json!({ "profile": 5 }); // "profile" is not an object
        let out = merge_preferences(base, &[("profile.x".into(), json!(true))]);
        assert_eq!(out["profile"]["x"], json!(true));
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod io_tests {
    use super::*;

    #[test]
    fn owned_writes_default_suppression() {
        let dir = tempfile::tempdir().unwrap();
        write_preferences(dir.path(), true, &[]);
        let s =
            std::fs::read_to_string(dir.path().join("Default/Preferences")).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["credentials_enable_service"], json!(false));
        assert_eq!(v["profile"]["password_manager_enabled"], json!(false));
    }

    #[test]
    fn supplied_without_prefs_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_preferences(dir.path(), false, &[]);
        assert!(!dir.path().join("Default/Preferences").exists());
    }

    #[test]
    fn supplied_preserves_existing_and_adds_user_pref() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Default")).unwrap();
        std::fs::write(
            dir.path().join("Default/Preferences"),
            r#"{"foo":1}"#,
        )
        .unwrap();
        write_preferences(dir.path(), false, &[("x.y".into(), json!(true))]);
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("Default/Preferences")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["foo"], json!(1)); // preserved
        assert_eq!(v["x"]["y"], json!(true)); // user pref added
        assert!(v.get("credentials_enable_service").is_none()); // no defaults for supplied
    }
}
