//! Helpers for deliberately corrupting a complete JSONL checkpoint in storage tests.
use serde_json::{json, Value};

pub(super) fn snapshot_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    let mut snapshot = json!({"invocations": [], "queue_history": []});
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: Value = serde_json::from_slice(line)?;
        for key in ["id", "provider", "provider_session_id"] {
            if let Some(value) = record.get(key) {
                snapshot[key] = value.clone();
            }
        }
        let queue_history = snapshot["queue_history"].as_array_mut().unwrap();
        queue_history.truncate(record["queue_from"].as_u64().unwrap() as usize);
        queue_history.extend(record["queue_history"].as_array().unwrap().iter().cloned());
        let invocations = snapshot["invocations"].as_array_mut().unwrap();
        invocations.truncate(record["invocation_count"].as_u64().unwrap() as usize);
        for change in record["invocations"].as_array().unwrap() {
            let index = change["index"].as_u64().unwrap() as usize;
            if index == invocations.len() {
                invocations.push(json!({"events": [], "scheduling": []}));
            }
            let invocation = &mut invocations[index];
            if let Some(metadata) = change.get("metadata") {
                for (key, value) in metadata.as_object().unwrap() {
                    invocation[key] = value.clone();
                }
            }
            for (key, from) in [("events", "events_from"), ("scheduling", "scheduling_from")] {
                let tail = invocation[key].as_array_mut().unwrap();
                tail.truncate(change[from].as_u64().unwrap() as usize);
                tail.extend(change[key].as_array().unwrap().iter().cloned());
            }
        }
    }
    Ok(snapshot)
}

pub(super) fn journal_bytes(snapshot: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut record = snapshot.clone();
    record["sequence"] = 1.into();
    record["queue_from"] = 0.into();
    let invocations = record["invocations"].as_array_mut().unwrap();
    for (index, invocation) in invocations.iter_mut().enumerate() {
        let mut metadata = invocation.take();
        let events = metadata.as_object_mut().unwrap().remove("events").unwrap();
        let scheduling = metadata
            .as_object_mut()
            .unwrap()
            .remove("scheduling")
            .unwrap();
        *invocation = json!({"index": index, "metadata": metadata, "events_from": 0,
            "events": events, "scheduling_from": 0, "scheduling": scheduling});
    }
    record["invocation_count"] = invocations.len().into();
    let mut bytes = serde_json::to_vec(&record)?;
    bytes.push(b'\n');
    Ok(bytes)
}
