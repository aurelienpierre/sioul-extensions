// SPDX-License-Identifier: GPL-3.0-or-later

//! A JSON:API document (`data` and `included`) flattened into plain nested
//! objects: each resource's `id`, `type` and attributes, each relationship
//! replaced by the resource `included` holds for it (or its bare reference),
//! a cycle cut. Ported from ha-skolengo's `jsonapi_deserialize`
//! (infernalK, MIT; `custom_components/skolengo/api.py:147-193` at 20dd40f).

use serde_json::{Map, Value};
use std::collections::HashMap;

/// The document's `data` flattened: an object, an array of objects, or null.
pub fn flatten(document: &Value) -> Value {
    let mut index: HashMap<(String, String), &Value> = HashMap::new();
    for resource in document["included"].as_array().into_iter().flatten() {
        index.insert(key(resource), resource);
    }
    match &document["data"] {
        Value::Array(items) => Value::Array(items.iter().map(|item| resolve(item, &index, &[])).collect()),
        Value::Object(_) => resolve(&document["data"], &index, &[]),
        _ => Value::Null,
    }
}

fn key(resource: &Value) -> (String, String) {
    (resource["type"].as_str().unwrap_or("").to_string(), resource["id"].as_str().unwrap_or("").to_string())
}

fn resolve(resource: &Value, index: &HashMap<(String, String), &Value>, seen: &[(String, String)]) -> Value {
    let own = key(resource);
    let mut out = Map::new();
    out.insert("id".into(), resource["id"].clone());
    out.insert("type".into(), resource["type"].clone());
    if let Some(attributes) = resource["attributes"].as_object() {
        for (name, value) in attributes {
            out.insert(name.clone(), value.clone());
        }
    }
    let seen: Vec<(String, String)> = seen.iter().cloned().chain(std::iter::once(own)).collect();
    let mut follow = |reference: &Value| -> Option<Value> {
        let key = key(reference);
        if seen.contains(&key) {
            return None;
        }
        Some(resolve(index.get(&key).copied().unwrap_or(reference), index, &seen))
    };
    if let Some(relationships) = resource["relationships"].as_object() {
        for (name, body) in relationships {
            let value = match &body["data"] {
                Value::Array(items) => Value::Array(items.iter().filter_map(&mut follow).collect()),
                Value::Object(_) => follow(&body["data"]).unwrap_or(Value::Null),
                _ => Value::Null,
            };
            out.insert(name.clone(), value);
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn relationships_are_resolved_from_included() {
        let document = json!({
            "data": [{ "id": "D1", "type": "agenda", "attributes": { "date": "2026-11-19" },
                       "relationships": { "lessons": { "data": [{ "id": "L1", "type": "lesson" }] } } }],
            "included": [
                { "id": "L1", "type": "lesson", "attributes": { "canceled": true },
                  "relationships": { "subject": { "data": { "id": "S1", "type": "subject" } }, "back": { "data": { "id": "D1", "type": "agenda" } } } },
                { "id": "S1", "type": "subject", "attributes": { "label": "Mathématiques" } }
            ]
        });
        let flat = flatten(&document);
        assert_eq!(flat[0]["date"], "2026-11-19");
        assert_eq!(flat[0]["lessons"][0]["canceled"], true);
        assert_eq!(flat[0]["lessons"][0]["subject"]["label"], "Mathématiques");
        assert_eq!(flat[0]["lessons"][0]["back"], Value::Null, "a cycle is cut");
        assert_eq!(flatten(&json!({ "data": null })), Value::Null);
    }
}
