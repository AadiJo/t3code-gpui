//! The opt-in id table of thread snapshots, history pages and thread stream items
//! (`orchestrationV2IdTable.ts`). A client that sends `acceptCompactIds` may receive these
//! responses with a top-level `ids` array; on each record (a top-level array element, a top-level
//! object, or a record's `item`), values under `id`, `*Id` and `*Ids` are then indexes into it.
//! Decoding through this module restores the real ids, so decoded values never carry indexes.

use crate::{OrchestrationV2ThreadBoundedSnapshot, OrchestrationV2ThreadHistoryPage, OrchestrationV2ThreadStreamItem};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Where a response keeps the records whose ids are interned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// The response object itself holds the records.
    TopLevel,
    /// The object under this key holds the records.
    Key(&'static str),
    /// This value has no records (other stream item kinds).
    None,
}

/// A response type that may arrive with an id table.
pub trait RecordIdTable: DeserializeOwned {
    /// Where the records of `value` live, mirroring the `container` callback in the TypeScript.
    fn container(value: &Map<String, Value>) -> Container;
}

impl RecordIdTable for OrchestrationV2ThreadBoundedSnapshot {
    fn container(_: &Map<String, Value>) -> Container {
        Container::Key("projection")
    }
}

impl RecordIdTable for OrchestrationV2ThreadHistoryPage {
    fn container(_: &Map<String, Value>) -> Container {
        Container::TopLevel
    }
}

impl RecordIdTable for OrchestrationV2ThreadStreamItem {
    fn container(value: &Map<String, Value>) -> Container {
        if value.get("kind").and_then(Value::as_str) == Some("snapshot") {
            Container::Key("projection")
        } else {
            Container::None
        }
    }
}

/// Decodes a response that may carry an id table. Without one this is a plain `from_str`.
pub fn from_str<T: RecordIdTable>(text: &str) -> Result<T, serde_json::Error> {
    if !has_top_level_ids(text)? {
        return serde_json::from_str(text);
    }
    from_value(serde_json::from_str(text)?)
}

/// Decodes an already parsed response (for example a stream chunk element) that may carry an
/// id table.
pub fn from_value<T: RecordIdTable>(mut value: Value) -> Result<T, serde_json::Error> {
    if let Value::Object(map) = &mut value
        && map.contains_key("ids")
    {
        expand_record_ids(map, T::container);
    }
    // Union decoders re-read raw JSON text, so decode from text rather than from the tree.
    serde_json::from_str(&serde_json::to_string(&value)?)
}

/// Removes `ids` from `raw` and puts the real ids back into the records `container` names.
pub fn expand_record_ids(raw: &mut Map<String, Value>, container: impl FnOnce(&Map<String, Value>) -> Container) {
    let Some(Value::Array(ids)) = raw.remove("ids") else { return };
    let target = match container(raw) {
        Container::TopLevel => Some(raw),
        Container::Key(key) => match raw.get_mut(key) {
            Some(Value::Object(map)) => Some(map),
            _ => None,
        },
        Container::None => None,
    };
    let Some(target) = target else { return };
    for value in target.values_mut() {
        match value {
            Value::Array(records) => {
                for record in records {
                    if let Value::Object(record) = record {
                        expand_record(record, &ids);
                    }
                }
            }
            Value::Object(record) => expand_record(record, &ids),
            _ => {}
        }
    }
}

fn is_id_key(key: &str) -> bool {
    key == "id" || key.ends_with("Id")
}

fn is_id_list_key(key: &str) -> bool {
    key.ends_with("Ids")
}

/// The id at `index`; an index past the table becomes null, which then fails to decode as an id.
fn lookup(ids: &[Value], index: &serde_json::Number) -> Value {
    index.as_u64().and_then(|i| ids.get(i as usize)).cloned().unwrap_or(Value::Null)
}

fn expand_record(record: &mut Map<String, Value>, ids: &[Value]) {
    for (key, value) in record.iter_mut() {
        match value {
            Value::Number(index) if is_id_key(key) => *value = lookup(ids, index),
            Value::Array(entries) if is_id_list_key(key) => {
                for entry in entries {
                    if let Value::Number(index) = entry {
                        *entry = lookup(ids, index);
                    }
                }
            }
            Value::Object(item) if key == "item" => expand_record(item, ids),
            _ => {}
        }
    }
}

/// Whether the top-level object of `text` has an `ids` key, without building a tree.
fn has_top_level_ids(text: &str) -> Result<bool, serde_json::Error> {
    use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
            let mut found = false;
            while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                found |= key == "ids";
                map.next_value::<IgnoredAny>()?;
            }
            Ok(found)
        }
    }
    if !text.trim_start().starts_with('{') {
        return Ok(false);
    }
    serde_json::Deserializer::from_str(text).deserialize_map(V)
}
