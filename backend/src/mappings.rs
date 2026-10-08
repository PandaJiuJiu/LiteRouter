//! Model mapping helpers: parse/encode the `targets` JSON column and the
//! legacy `target_model` fallback. Used by the relay and the config backup
//! system, not just the admin UI.

use serde::{Deserialize, Serialize};

/// One entry in the ordered `targets` array. The `#[serde(untagged)]` lets
/// both the new object form and the legacy plain-string form deserialize
/// from the same column.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TargetEntry {
    Obj {
        /// empty = any channel serving this model
        #[serde(default)]
        channel: String,
        /// upstream model id; "*" means every model on the pinned channel
        model: String,
    },
    Str(String),
}

impl TargetEntry {
    fn normalize(entry: TargetEntry, out: &mut Vec<(String, String)>) {
        let (channel, model) = match entry {
            TargetEntry::Obj { channel, model } => {
                (channel.trim().to_string(), model.trim().to_string())
            }
            TargetEntry::Str(s) => (String::new(), s.trim().to_string()),
        };
        // a wildcard needs a channel to expand against
        if model.is_empty() || (model == "*" && channel.is_empty()) {
            return;
        }
        if !out.iter().any(|(c, m)| c == &channel && m == &model) {
            out.push((channel, model));
        }
    }
}

/// Parse the JSON-array `targets` column into ordered (channel, model)
/// pairs; fall back to the legacy single `target_model` column when the row
/// predates the migration. An empty channel means "any channel".
pub fn parse_targets(targets: &str, fallback: &str) -> Vec<(String, String)> {
    let raw: serde_json::Value = match serde_json::from_str(targets) {
        Ok(v) => v,
        Err(_) => {
            if fallback.is_empty() {
                return Vec::new();
            } else {
                return vec![(String::new(), fallback.trim().to_string())];
            }
        }
    };
    let entries = deserialize_targets(raw);
    let mut out: Vec<(String, String)> = Vec::new();
    for e in entries {
        TargetEntry::normalize(e, &mut out);
    }
    if out.is_empty() && !fallback.trim().is_empty() {
        out.push((String::new(), fallback.trim().to_string()));
    }
    out
}

/// Serialize normalized targets back into the stored JSON form.
pub fn encode_targets(targets: &[(String, String)]) -> String {
    let entries: Vec<TargetEntry> = targets
        .iter()
        .map(|(channel, model)| TargetEntry::Obj {
            channel: channel.clone(),
            model: model.clone(),
        })
        .collect();
    serde_json::to_string(&entries).unwrap_or_default()
}

/// Like [`parse_targets`] but skips the legacy fallback, returning only
/// what the `targets` JSON column contains.
pub fn clean_targets(raw: &[TargetEntry]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in raw {
        TargetEntry::normalize(e.clone(), &mut out);
    }
    out
}

/// Deserialize a `targets` field that accepts both the legacy string form
/// (`["gpt-4o-mini"]`) and the new object form (`[{model:"gpt-4o-mini",channel:""}]`).
/// `#[serde(untagged)]` on an enum inside a Vec does not backtrack correctly in
/// serde_json, so we accept `Value` here and convert below.
pub fn deserialize_targets(raw: serde_json::Value) -> Vec<TargetEntry> {
    match raw {
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(TargetEntry::Str(s)),
                serde_json::Value::Object(mut obj) => {
                    let model = obj
                        .remove("model")
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default();
                    let channel = obj
                        .remove("channel")
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default();
                    if model.is_empty() {
                        None
                    } else {
                        Some(TargetEntry::Obj { channel, model })
                    }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_target_parses_as_str_variant() {
        let raw = serde_json::json!(["gpt-4o-mini"]);
        let entries = deserialize_targets(raw);
        let targets = clean_targets(&entries);
        assert_eq!(targets, vec![("".to_string(), "gpt-4o-mini".to_string())]);
    }

    #[test]
    fn object_target_parses_as_obj_variant() {
        let raw = serde_json::json!([{"channel": "volc", "model": "gpt-4o-mini"}]);
        let entries = deserialize_targets(raw);
        let targets = clean_targets(&entries);
        assert_eq!(
            targets,
            vec![("volc".to_string(), "gpt-4o-mini".to_string())]
        );
    }

    #[test]
    fn parse_targets_from_object_json() {
        // What relay_contract tests write directly to the DB
        let parsed = parse_targets(r#"[{"channel": "second", "model": "gpt-4o"}]"#, "");
        assert_eq!(parsed, vec![("second".to_string(), "gpt-4o".to_string())]);
    }

    #[test]
    fn parse_targets_from_string_json() {
        let parsed = parse_targets(r#"["gpt-4o-mini"]"#, "");
        assert_eq!(parsed, vec![("".to_string(), "gpt-4o-mini".to_string())]);
    }
}
