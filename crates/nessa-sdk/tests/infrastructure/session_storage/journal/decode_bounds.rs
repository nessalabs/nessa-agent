//! Decode-pass read counts distinguish early bounds from post-allocation validation.
use super::{read, Loaded};
use crate::application::agent_execution::agents::{AgentError, ProviderDiagnostic};
use crate::application::agent_execution::sessions::{
    SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::domain::agent_execution::{
    prompts::{LinkedFile, UserMessage},
    sessions::SessionId,
    tools::ToolContent,
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    io::{self, Cursor, Read, Seek, SeekFrom},
    mem::size_of,
    rc::Rc,
};

struct CountDecode {
    bytes: Cursor<Vec<u8>>,
    decoded: Rc<Cell<usize>>,
    after_scan: bool,
}
impl Read for CountDecode {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = self.bytes.read(out)?;
        if self.after_scan {
            self.decoded.set(self.decoded.get() + count);
        }
        Ok(count)
    }
}
impl Seek for CountDecode {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if matches!(position, SeekFrom::Start(0)) && self.bytes.position() != 0 {
            self.after_scan = true;
        }
        self.bytes.seek(position)
    }
}
fn metadata() -> Value {
    json!({
        "submission":"Immediate", "execution_id":"active", "user_message":"input", "user_images":[], "user_files":[],
        "estimated_input_tokens":1, "reserved_output_tokens":1,
        "actor":{"principal_id":"user","surface_id":"test","request_id":"invoke"},
        "acknowledgement":"Pending",
        "provider_report":null,"local_outcome":null,"cancellation":null,"result":null
    })
}
fn record() -> Value {
    json!({
        "sequence":1,"id":"decode-bounds", "provider":{"name":"fixture","model_id":"model","context":"workspace"},
        "provider_context":"provider", "invocation_count":1, "queue_from":0, "queue_history":[],
        "invocations":[{"index":0,"metadata":metadata(),"events_from":0,"events":[],"scheduling_from":0,"scheduling":[]}]
    })
}
fn load(bytes: Vec<u8>) -> (Result<Loaded, StorageError>, usize) {
    let decoded = Rc::new(Cell::new(0));
    let result = read(
        CountDecode {
            bytes: Cursor::new(bytes),
            decoded: decoded.clone(),
            after_scan: false,
        },
        &SessionId::new("decode-bounds").unwrap(),
    );
    (result, decoded.get())
}
fn encoded(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn oversized_provider_field_is_rejected_before_reading_its_owned_payload() {
    // Put the attack field first and a valid, huge remainder after it. The initial
    // completeness scan is deliberately excluded from this allocation-bound check.
    for escaped in [false, true] {
        let unit = if escaped { "\\u0061" } else { "a" };
        let text = unit.repeat(2 * 1024 * 1024);
        let bytes = format!("{{\"provider\":{{\"name\":\"{text}\",\"model_id\":\"model\",\"context\":\"workspace\"}},\"sequence\":1,\"id\":\"decode-bounds\",\"provider_context\":\"provider\",\"invocation_count\":0,\"queue_from\":0,\"queue_history\":[],\"invocations\":[]}}\n").into_bytes();
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        assert!(
            decoded <= 64 * 1024,
            "oversized field was read before its bound: {decoded}/{length}, escaped={escaped}"
        );
    }
}

#[test]
fn provider_diagnostic_is_required_and_bounded_before_owned_decode() {
    let mut missing = record();
    missing["invocations"][0]["metadata"]["result"] = json!({"Err":{"Provider":{"code":-32000}}});
    assert!(matches!(
        load(encoded(&missing)).0,
        Err(StorageError::Corrupt(_))
    ));

    for escaped in [false, true] {
        let unit = if escaped { "\\u0070" } else { "p" };
        let diagnostic = unit.repeat(ProviderDiagnostic::MAX_BYTES + 1);
        let mut oversized = record();
        oversized["invocations"][0]["metadata"]["user_message"] = json!("x".repeat(1024 * 1024));

        // Construct the wire order explicitly: workspace feature unification can
        // make serde_json preserve insertion order instead of sorting map keys.
        let metadata = oversized["invocations"][0]["metadata"]
            .as_object_mut()
            .unwrap();
        metadata.remove("result").unwrap();
        let remaining = serde_json::to_string(metadata).unwrap();
        let result = format!(
            "{{\"Err\":{{\"Provider\":{{\"code\":-32000,\"diagnostic\":\"{diagnostic}\"}}}}}}"
        );
        let ordered = format!("{{\"result\":{result},{}", &remaining[1..]);
        oversized["invocations"][0]["metadata"] = json!("ordered-metadata");
        let bytes = String::from_utf8(encoded(&oversized))
            .unwrap()
            .replace("\"ordered-metadata\"", &ordered)
            .into_bytes();
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        assert!(
            decoded <= 64 * 1024,
            "oversized provider diagnostic was read before its bound: {decoded}/{length}, escaped={escaped}"
        );
    }
}

#[test]
fn valid_large_tool_string_remains_loadable() {
    let mut value = record();
    value["invocations"][0]["events"] = json!([{
        "execution_id":"active", "update":{"Tool":{
            "id":"tool","title":"t".repeat(5 * 1024 * 1024),"kind":null,"status":null,"locations":null,"content":null
        }}
    }]);
    assert!(
        load(encoded(&value)).0.is_ok(),
        "tool fields use their 32MiB aggregate budget, not the message chunk limit"
    );
}

#[test]
fn invocation_history_accepts_its_exact_bound_and_refuses_one_more() {
    assert_eq!(SessionSnapshot::MAX_INVOCATIONS, 1024);
    let mut value = record();
    let mut changes = Vec::new();
    for index in 0..=SessionSnapshot::MAX_INVOCATIONS {
        let mut change = record()["invocations"][0].clone();
        change["index"] = json!(index);
        change["metadata"]["execution_id"] = json!(format!("execution-{index}"));
        changes.push(change);
    }
    let overflow = changes.pop().unwrap();
    value["invocation_count"] = json!(changes.len());
    value["invocations"] = Value::Array(changes);
    let loaded = load(encoded(&value))
        .0
        .expect("the complete retained history bound must remain loadable");
    assert_eq!(
        loaded.snapshot.unwrap().invocations.len(),
        SessionSnapshot::MAX_INVOCATIONS
    );
    value["invocations"].as_array_mut().unwrap().push(overflow);
    value["invocation_count"] = json!(SessionSnapshot::MAX_INVOCATIONS + 1);
    assert!(matches!(
        load(encoded(&value)).0,
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn diagnostic_depth_and_hook_collection_are_bounded_before_later_fields_are_read() {
    for attack in ["depth", "hooks"] {
        let mut value = record();
        let error = if attack == "depth" {
            let mut error = json!("Closed");
            for _ in 0..32 {
                error = json!({"ExecutionObservation":{"error":error,"execution_result":null}});
            }
            error
        } else {
            json!({"AfterInvocationHooks":{
                "failures": (0..129).map(|index| json!({"index":index,"error":"Panicked"})).collect::<Vec<_>>(),
                "execution_result":{"Ok":"Completed"}
            }})
        };
        value["invocations"][0]["metadata"]["result"] = json!({"Err":error});
        value["invocations"][0]["metadata"]["user_message"] = json!("p".repeat(1024 * 1024));
        // Fix wire order explicitly. Value maps sort keys by default but preserve
        // insertion order when another workspace package enables preserve_order.
        let metadata = value["invocations"][0]["metadata"].as_object_mut().unwrap();
        let result = serde_json::to_string(&metadata.remove("result").unwrap()).unwrap();
        let remaining = serde_json::to_string(metadata).unwrap();
        let ordered = format!("{{\"result\":{result},{}", &remaining[1..]);
        value["invocations"][0]["metadata"] = json!("ordered-metadata");
        let bytes = String::from_utf8(encoded(&value))
            .unwrap()
            .replace("\"ordered-metadata\"", &ordered)
            .into_bytes();
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))), "{attack}");
        assert!(
            decoded <= 64 * 1024,
            "diagnostic {attack} bound ran after later allocation: {decoded}/{length}"
        );
    }
}

#[test]
fn escaped_provider_identity_counts_decoded_utf8_bytes() {
    for (encoded_name, accepted) in [
        ("\\u0061".repeat(256), true),
        ("\\u0061".repeat(257), false),
        ("\\ud83d\\ude00".repeat(64), true),
        ("\\ud83d\\ude00".repeat(65), false),
    ] {
        let bytes = format!("{{\"sequence\":1,\"id\":\"decode-bounds\",\"provider\":{{\"name\":\"{encoded_name}\",\"model_id\":\"model\",\"context\":\"workspace\"}},\"provider_context\":\"provider\",\"invocation_count\":0,\"queue_from\":0,\"queue_history\":[],\"invocations\":[]}}\n").into_bytes();
        assert_eq!(load(bytes).0.is_ok(), accepted);
    }
}

#[test]
fn unknown_object_keys_are_bounded_before_serde_builds_the_key() {
    let key = "k".repeat(2 * 1024 * 1024);
    let bytes = format!("{{\"{key}\":null}}\n").into_bytes();
    let (result, decoded) = load(bytes);
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    assert!(
        decoded <= 64 * 1024,
        "unknown key allocation was not bounded: {decoded}"
    );
}

#[test]
fn invalid_change_indices_and_missing_metadata_stop_before_unbounded_history_building() {
    for attack in ["duplicate", "gap", "missing"] {
        let mut value = record();
        let mut later = value["invocations"][0].clone();
        later["metadata"]["user_message"] = json!("p".repeat(2 * 1024 * 1024));
        match attack {
            "duplicate" => value["invocations"].as_array_mut().unwrap().push(later),
            "gap" => {
                later["index"] = json!(1);
                value["invocations"] = json!([later]);
            }
            _ => {
                value["invocations"][0]["metadata"] = Value::Null;
                later["index"] = json!(1);
                value["invocations"].as_array_mut().unwrap().push(later);
                value["invocation_count"] = json!(2);
            }
        }
        let (result, decoded) = load(encoded(&value));
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        assert!(
            decoded <= 64 * 1024,
            "{attack} change list was materialized first: {decoded}"
        );
    }
}

#[test]
fn valid_error_depth_and_hook_node_boundaries_are_not_narrowed_by_preflight() {
    let mut depth = json!("Closed");
    for _ in 0..31 {
        depth = json!({"ExecutionObservation":{"error":depth,"execution_result":null}});
    }
    let hooks = json!({"AfterInvocationHooks":{
        "failures": (0..127).map(|index| json!({"index":index,"error":"Panicked"})).collect::<Vec<_>>(),
        "execution_result":{"Ok":"Completed"}
    }});
    for error in [depth, hooks] {
        let mut value = record();
        value["invocations"][0]["metadata"]["result"] = json!({"Err":error});
        assert!(load(encoded(&value)).0.is_ok());
    }
}

#[test]
fn tool_collection_slots_are_bounded_before_reading_the_excess_element() {
    let mut value = record();
    value["invocations"][0]["events"] = json!([{
        "execution_id":"active", "update":{"Tool":{
            "id":"tool","title":null,"kind":null,"status":null,"locations":null,"content":[]
        }}
    }]);
    let max_slots = 32 * 1024 * 1024 / size_of::<ToolContent>();
    let items = "{\"Text\":\"\"},".repeat(max_slots);
    let trailing_payload = "p".repeat(2 * 1024 * 1024);
    let text = serde_json::to_string(&value).unwrap().replace(
        "\"content\":[]",
        &format!("\"content\":[{items}{{\"Text\":\"{trailing_payload}\"}}]"),
    );
    let prefix = text.find(&trailing_payload).unwrap();
    let mut bytes = text.into_bytes();
    bytes.push(b'\n');
    let (result, decoded) = load(bytes);
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    assert!(
        decoded <= prefix + 8192,
        "excess collection element was decoded: {decoded}, prefix={prefix}"
    );
}

fn saved_image(digest: &str, media_type: &str) -> Value {
    json!({"digest": digest, "media_type": media_type, "size": 1})
}
fn digest() -> String {
    format!("sha256:{}", "ab".repeat(32))
}
/// The record with `key` first in its metadata and two mebibytes of valid user
/// message after it, so that a bound applied only after the whole metadata was
/// decoded shows up in the decoded-byte count.
fn metadata_first(key: &str, first: &Value) -> Vec<u8> {
    let mut value = record();
    value["invocations"][0]["metadata"]["user_message"] = json!("p".repeat(2 * 1024 * 1024));
    let metadata = value["invocations"][0]["metadata"].as_object_mut().unwrap();
    assert!(
        metadata.remove(key).is_some(),
        "the fixture must carry {key}, which this moves to the front"
    );
    let remaining = serde_json::to_string(metadata).unwrap();
    let ordered = format!("{{\"{key}\":{first},{}", &remaining[1..]);
    value["invocations"][0]["metadata"] = json!("ordered-metadata");
    String::from_utf8(encoded(&value))
        .unwrap()
        .replace("\"ordered-metadata\"", &ordered)
        .into_bytes()
}
fn images_first(images: &Value) -> Vec<u8> {
    metadata_first("user_images", images)
}
fn files_first(files: &Value) -> Vec<u8> {
    metadata_first("user_files", files)
}

#[test]
fn a_saved_message_holds_at_most_the_live_number_of_images() {
    let one = saved_image(&digest(), "image/png");
    let most = Value::Array(vec![one.clone(); UserMessage::MAX_IMAGES]);
    let loaded = load(images_first(&most)).0.expect("the exact bound loads");
    assert_eq!(
        loaded.snapshot.unwrap().invocations[0]
            .request
            .user_message
            .images()
            .len(),
        UserMessage::MAX_IMAGES
    );

    // One more, and two hundred thousand more, stop at the eleventh element.
    for count in [UserMessage::MAX_IMAGES + 1, 200_000] {
        let bytes = images_first(&Value::Array(vec![one.clone(); count]));
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))), "{count}");
        assert!(
            decoded <= 64 * 1024,
            "{count} saved images were decoded before their bound: {decoded}/{length}"
        );
    }
}

#[test]
fn saved_image_fields_are_bounded_before_their_text_is_built() {
    let exact = json!([saved_image(&digest(), "image/jpeg")]);
    assert!(load(images_first(&exact)).0.is_ok());
    assert_eq!(digest().len(), 71);

    for (field, image) in [
        // One byte past the only digest form, and eight mebibytes past it.
        (
            "digest",
            saved_image(&format!("{}0", digest()), "image/png"),
        ),
        (
            "digest",
            saved_image(&"d".repeat(8 * 1024 * 1024), "image/png"),
        ),
        (
            "media_type",
            saved_image(&digest(), &"m".repeat(8 * 1024 * 1024)),
        ),
        ("media_type", saved_image(&digest(), "image/x-seventeen")),
        // Nothing but a digest, a media type, and a size belongs in an image.
        (
            "unknown",
            json!({"digest": digest(), "media_type": "image/png", "size": 1,
                "bytes": "b".repeat(8 * 1024 * 1024)}),
        ),
    ] {
        let bytes = images_first(&json!([image]));
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))), "{field}");
        assert!(
            decoded <= 64 * 1024,
            "saved image {field} was decoded before its bound: {decoded}/{length}"
        );
    }
}

/// The same three bounds as images, for the paths a message points at: how many
/// there may be, how long each one may be, and what an entry may hold at all.
///
/// Each is applied while the collection is being read rather than after, which
/// is the whole point of stating them: a journal is a file on this disk that
/// something else may have written, and a decoder that allocates first and
/// checks afterwards has already done the expensive thing. `user_files` is put
/// first in the metadata so that a bound applied late would show as two
/// mebibytes of still-valid message read after it.
#[test]
fn a_saved_message_holds_at_most_the_live_number_of_paths_each_within_its_bound() {
    let one = json!({"path": "/Users/dev/notes.md"});
    let most = Value::Array(vec![one.clone(); UserMessage::MAX_FILES]);
    let loaded = load(files_first(&most)).0.expect("the exact bound loads");
    assert_eq!(
        loaded.snapshot.unwrap().invocations[0]
            .request
            .user_message
            .files()
            .len(),
        UserMessage::MAX_FILES
    );

    // One more, and two hundred thousand more, stop at the eleventh element.
    for count in [UserMessage::MAX_FILES + 1, 200_000] {
        let bytes = files_first(&Value::Array(vec![one.clone(); count]));
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))), "{count}");
        assert!(
            decoded <= 64 * 1024,
            "{count} saved paths were decoded before their bound: {decoded}/{length}"
        );
    }

    // A path longer than any the domain would have accepted, and long enough
    // that reading it before refusing it would be the expensive mistake.
    let long = format!("/{}", "p".repeat(2 * 1024 * 1024));
    let bytes = files_first(&json!([{ "path": long }]));
    let length = bytes.len();
    let (result, decoded) = load(bytes);
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    assert!(
        decoded <= 64 * 1024,
        "an oversized path was read before its bound: {decoded}/{length}"
    );
    // And the bound is the domain's own, not something smaller: the longest
    // path a message could have named loads back.
    let longest = format!("/{}", "p".repeat(LinkedFile::MAX_PATH_BYTES - 1));
    assert!(
        load(files_first(&json!([{ "path": longest }]))).0.is_ok(),
        "the longest path the domain accepts must load"
    );

    // A file entry is a path and nothing else. An unknown key is refused
    // before its value is read, so a mebibyte hung off one costs nothing.
    let bytes = files_first(&json!([{ "reach": "p".repeat(2 * 1024 * 1024), "path": "/a" }]));
    let length = bytes.len();
    let (result, decoded) = load(bytes);
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    assert!(
        decoded <= 64 * 1024,
        "an unknown file-entry key was read before it was refused: {decoded}/{length}"
    );
}

/// A journal written before a message could point at files has no `user_files`
/// key, and it loads: the message named none, which is what empty says.
///
/// That is the true historical meaning rather than a compatibility fiction —
/// no message could name a file — so this is one contract reading its own
/// earlier records, not a second reader kept alive beside a first. Six
/// `Option` fields in the same struct already take that reading of their own
/// absence.
///
/// The line a default must not cross is inventing a value a record could have
/// meant something else by, and the test below walks right up to it: a `path`
/// has no default, so a file entry without one is corrupt and stays corrupt.
#[test]
fn a_journal_written_before_files_reads_as_a_message_that_named_none() {
    let mut value = record();
    let metadata = value["invocations"][0]["metadata"].as_object_mut().unwrap();
    assert!(
        metadata.remove("user_files").is_some(),
        "the fixture must carry the field this test removes"
    );
    let loaded = load(encoded(&value)).0.expect("an older journal loads");
    let restored = loaded.snapshot.expect("the snapshot is there");
    let message = &restored.invocations[0].request.user_message;
    assert!(message.files().is_empty());
    // And nothing else about the record moved.
    assert_eq!(message.text_str(), "input");
    assert!(message.images().is_empty());

    // The default stops at the field it is on. A file entry is still a path
    // and nothing else, so one without a path names nothing and is refused
    // rather than quietly becoming an empty string.
    let mut missing = record();
    missing["invocations"][0]["metadata"]["user_files"] = json!([{}]);
    assert!(matches!(
        load(encoded(&missing)).0,
        Err(StorageError::Corrupt(_))
    ));
    let mut nulled = record();
    nulled["invocations"][0]["metadata"]["user_files"] = json!([{ "path": null }]);
    assert!(matches!(
        load(encoded(&nulled)).0,
        Err(StorageError::Corrupt(_))
    ));
    // As does a record whose own required fields are gone: defaulting one
    // field is not defaulting the struct.
    let mut gutted = record();
    let metadata = gutted["invocations"][0]["metadata"]
        .as_object_mut()
        .unwrap();
    metadata.remove("user_images");
    assert!(matches!(
        load(encoded(&gutted)).0,
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn acknowledgement_audit_uses_direct_optional_error_shape_and_preflight_bounds() {
    let mut valid = record();
    valid["invocations"][0]["metadata"]["acknowledgement"] = json!({
        "Failed": { "audit": "AuditFailure", "storage": null }
    });
    let restored = load(encoded(&valid)).0.unwrap().snapshot.unwrap();
    assert!(matches!(
        restored.invocations[0].acknowledgement,
        SubmissionAcknowledgement::Failed {
            audit: Some(AgentError::AuditFailure),
            storage: None
        }
    ));

    for (field, error) in [
        ("audit", json!({ "Protocol": "x".repeat(1024 * 1024 + 1) })),
        ("storage", json!({ "Io": "x".repeat(4097) })),
    ] {
        let mut oversized = record();
        let mut acknowledgement = json!({
            "Failed": { "audit": null, "storage": null }
        });
        acknowledgement["Failed"][field] = error;
        oversized["invocations"][0]["metadata"]["acknowledgement"] = acknowledgement;
        let metadata = oversized["invocations"][0]["metadata"]
            .as_object_mut()
            .unwrap();
        let acknowledgement = metadata.remove("acknowledgement").unwrap();
        metadata.remove("user_message").unwrap();
        let remaining = serde_json::to_string(metadata).unwrap();
        let ordered = format!(
            "{{\"acknowledgement\":{},\"user_message\":\"{}\",{}",
            serde_json::to_string(&acknowledgement).unwrap(),
            "x".repeat(1024 * 1024),
            &remaining[1..]
        );
        oversized["invocations"][0]["metadata"] = json!("ordered-metadata");
        let bytes = String::from_utf8(encoded(&oversized))
            .unwrap()
            .replace("\"ordered-metadata\"", &ordered)
            .into_bytes();
        let length = bytes.len();
        let (result, decoded) = load(bytes);
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        assert!(
            decoded <= 64 * 1024,
            "oversized acknowledgement {field} was owned before rejection: {decoded}/{length}"
        );
    }

    let mut exact = record();
    exact["invocations"][0]["metadata"]["acknowledgement"] = json!({
        "Failed": { "audit": null, "storage": { "Io": "x".repeat(4096) } }
    });
    assert!(load(encoded(&exact)).0.is_ok());

    let mut nested = json!("AuditFailure");
    for _ in 0..33 {
        nested = json!({
            "MultipleOperationFailures": {
                "first_error": nested,
                "subsequent_error": "AuditFailure"
            }
        });
    }
    let mut deep = record();
    deep["invocations"][0]["metadata"]["acknowledgement"] = json!({
        "Failed": { "audit": nested, "storage": null }
    });
    let metadata = deep["invocations"][0]["metadata"].as_object_mut().unwrap();
    let acknowledgement = metadata.remove("acknowledgement").unwrap();
    metadata.remove("user_message").unwrap();
    let remaining = serde_json::to_string(metadata).unwrap();
    let ordered = format!(
        "{{\"acknowledgement\":{},\"user_message\":\"{}\",{}",
        serde_json::to_string(&acknowledgement).unwrap(),
        "x".repeat(1024 * 1024),
        &remaining[1..]
    );
    deep["invocations"][0]["metadata"] = json!("ordered-metadata");
    let bytes = String::from_utf8(encoded(&deep))
        .unwrap()
        .replace("\"ordered-metadata\"", &ordered)
        .into_bytes();
    let length = bytes.len();
    let (result, decoded) = load(bytes);
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    assert!(
        decoded <= 64 * 1024,
        "deep acknowledgement error was owned before rejection: {decoded}/{length}"
    );
}
