// SPDX-License-Identifier: GPL-3.0-or-later

//! A JSON:API document (`data` and `included`) flattened into plain nested
//! objects: each resource's `id`, `type` and attributes, each relationship
//! replaced by the resource `included` holds for it (or its bare reference),
//! a cycle cut. Ported from ha-skolengo's `jsonapi_deserialize`
//! (infernalK, MIT; `custom_components/skolengo/api.py:147-193` at 20dd40f).
//! Bounded, whatever an answer chains: relationships are followed `DEPTH`
//! deep at most (a lesson needs three: the day, the lesson, its subject or
//! teachers), a reference past that depth left bare; and `MOST` resources
//! are resolved in all (one shared many times is resolved each time), the
//! references past that left out.

use serde_json::{Map, Value};
use std::collections::HashMap;

/// How deep relationships are followed, and how many resources are resolved in all.
const DEPTH: usize = 4;
const MOST: usize = 50_000;

/// The document's `data` flattened: an object, an array of objects, or null.
pub fn flatten(document: &Value) -> Value {
    let mut index: HashMap<(String, String), &Value> = HashMap::new();
    for resource in document["included"].as_array().into_iter().flatten() {
        index.insert(key(resource), resource);
    }
    let mut left = MOST;
    match &document["data"] {
        Value::Array(items) => Value::Array(items.iter().map(|item| resolve(item, &index, &[], &mut left)).collect()),
        Value::Object(_) => resolve(&document["data"], &index, &[], &mut left),
        _ => Value::Null,
    }
}

fn key(resource: &Value) -> (String, String) {
    (resource["type"].as_str().unwrap_or("").to_string(), resource["id"].as_str().unwrap_or("").to_string())
}

fn resolve(resource: &Value, index: &HashMap<(String, String), &Value>, seen: &[(String, String)], left: &mut usize) -> Value {
    *left = left.saturating_sub(1);
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
        // Too many resolved already: left out. Too deep: the reference as it is, its id and type alone.
        if *left == 0 {
            return None;
        }
        if seen.len() >= DEPTH {
            return Some(serde_json::json!({ "id": reference["id"], "type": reference["type"] }));
        }
        Some(resolve(index.get(&key).copied().unwrap_or(reference), index, &seen, left))
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

    /// E4: a chain of relationships a few hundred deep, or one resource shared
    /// at each step (2^n copies), stays bounded.
    #[test]
    fn a_deep_or_wide_chain_stays_bounded() {
        let link = |i: usize, n: usize| json!({ "id": format!("R{i}"), "type": "r", "relationships": { "next": { "data": (0..n).map(|_| json!({ "id": format!("R{}", i + 1), "type": "r" })).collect::<Vec<_>>() } } });
        let deep = json!({ "data": link(0, 1), "included": (1..500).map(|i| link(i, 1)).collect::<Vec<_>>() });
        let flat = flatten(&deep);
        assert_eq!(flat["next"][0]["next"][0]["next"][0]["next"][0]["id"], "R4");
        assert_eq!(flat["next"][0]["next"][0]["next"][0]["next"][0]["next"], Value::Null, "followed four deep at most");
        let wide = json!({ "data": link(0, 30), "included": (1..60).map(|i| link(i, 30)).collect::<Vec<_>>() });
        assert!(flatten(&wide).to_string().len() < 64 << 20, "30^4 resources at most, cut at 50,000");
    }
}
