//! The composed `fetch` over a real protected connection to a scripted
//! native peer and real files: the one JSON line, the typed exit, the
//! recheck only after an authority refusal, and no output unless verified.
use super::{fetch, kept};
use crate::read_only_sync::application::held_file::{FetchFiles, Pause, BUSY_RETRY_DELAYS_MS};
use crate::read_only_sync::domain::HeldFile;
use crate::read_only_sync::entrypoint::CommandError;
use crate::read_only_sync::infrastructure::gateway::tests::session::{
    connect, peer, request, send, PeerSocket,
};
use crate::read_only_sync::infrastructure::gateway::GatewayConnection;
use crate::read_only_sync::infrastructure::held_file::DiskFiles;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::product_method;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, ResponseFrame};
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::time::Duration;

struct NoPause;
impl Pause for NoPause {
    fn pause(&self, _: Duration, _: &dyn Fn() -> bool) {}
}

fn bytes() -> Vec<u8> {
    (0..40_000).map(|index| (index % 249) as u8).collect()
}
fn held(bytes: &[u8]) -> HeldFile {
    HeldFile::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        Sha256Digest::from_bytes(Sha256::digest(bytes).into()),
        MediaType::parse("application/octet-stream").unwrap(),
        bytes.len() as u64,
    )
    .unwrap()
}
fn refuse(socket: &mut PeerSocket, code: &str) {
    let read = request(socket);
    assert_eq!(read.method, product_method::ARTIFACT_READ);
    send(
        socket,
        OutgoingMessage::Response(ResponseFrame::failure(&read.id, code, "refused")),
    );
}
fn serve(socket: &mut PeerSocket, bytes: &[u8]) {
    let read = request(socket);
    let offset = read.params["offset"].as_u64().unwrap() as usize;
    let end = (offset + read.params["maxBytes"].as_u64().unwrap() as usize).min(bytes.len());
    let data = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes[offset..end],
    );
    send(
        socket,
        OutgoingMessage::Response(
            ResponseFrame::success(&read.id, &json!({"offset": offset, "data": data})).unwrap(),
        ),
    );
}
/// The one line `output` holds, parsed.
fn line(output: &[u8]) -> Value {
    assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(output.last(), Some(&b'\n'));
    serde_json::from_slice(output).unwrap()
}
/// Run the composed fetch against `script`, with a recheck that counts how
/// often it was asked.
fn run(
    out: &Path,
    file: &HeldFile,
    script: impl FnOnce(&mut PeerSocket) + Send + 'static,
) -> (Result<(), CommandError>, Value, usize) {
    let (endpoint, peer) = peer(script);
    let mut connection = GatewayConnection::new(connect(&endpoint));
    let asked = Cell::new(0);
    let mut output = vec![];
    let result = fetch(
        &mut connection,
        &NoPause,
        file,
        out,
        &|_| {
            asked.set(asked.get() + 1);
            json!({"enrollment":{"phase":"terminal"}})
        },
        json!({"phase":"active"}),
        &mut output,
    );
    drop(connection);
    if peer.join().is_err() {
        panic!("peer script failed; fetch result was {result:?}");
    }
    (result, line(&output), asked.get())
}
fn entries(directory: &Path) -> usize {
    fs::read_dir(directory).unwrap().count()
}

#[test]
fn a_fetched_file_is_kept_whole_with_one_line_and_nothing_left_beside_it() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let served = bytes();
    let file = held(&served);
    let (result, value, asked) = run(&out, &file, move |socket| {
        serve(socket, &served);
        serve(socket, &served);
    });
    assert!(result.is_ok());
    assert_eq!(value["successful"], true);
    assert_eq!(value["ranges"], "2");
    assert_eq!(value["recheck"], Value::Null);
    assert_eq!(asked, 0);
    assert_eq!(fs::read(&out).unwrap(), bytes());
    assert_eq!(entries(root.path()), 1);
}

#[test]
fn a_refused_fetch_leaves_no_output_and_no_empty_part() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let (result, value, asked) = run(&out, &held(&bytes()), |socket| {
        refuse(socket, ConversationErrorCode::AttachmentNotFound.as_str())
    });
    assert!(matches!(result, Err(CommandError::OnlineRefused)));
    assert_eq!(value["successful"], false);
    assert_eq!(value["connectionCheck"], "performed");
    assert_eq!(
        value["fetchFailure"]["transportFailure"]["productCode"],
        "attachment_not_found"
    );
    assert_eq!(value["recheck"], Value::Null);
    assert_eq!(asked, 0);
    assert!(!out.exists());
    assert_eq!(entries(root.path()), 0);
}

#[test]
fn a_forbidden_range_asks_the_status_again() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let (result, value, asked) = run(&out, &held(&bytes()), |socket| refuse(socket, "forbidden"));
    assert!(matches!(result, Err(CommandError::OnlineRefused)));
    assert_eq!(asked, 1);
    assert_eq!(value["recheck"], json!({"enrollment":{"phase":"terminal"}}));
    assert_eq!(value["enrollment"], json!({"phase":"active"}));
    assert!(!out.exists());
}

#[test]
fn a_range_busy_past_the_bound_keeps_the_part_and_asks_nothing_more() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let served = bytes();
    let file = held(&served);
    let (result, value, asked) = run(&out, &file, move |socket| {
        serve(socket, &served);
        for _ in 0..=BUSY_RETRY_DELAYS_MS.len() {
            refuse(
                socket,
                ConversationErrorCode::TemporarilyUnavailable.as_str(),
            );
        }
    });
    assert!(matches!(result, Err(CommandError::OnlineRefused)));
    assert_eq!(value["recheck"], Value::Null);
    assert_eq!(asked, 0);
    assert_eq!(value["busyRetries"], "3");
    assert_eq!(value["reached"], "32768");
    assert!(!out.exists());
    // The first range waits in the part for the next run.
    assert_eq!(entries(root.path()), 1);
}

#[test]
fn an_output_already_there_answers_before_anything_is_dialled() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(&bytes());
    let mut output = vec![];
    assert!(kept(&file, &out, &mut output).is_none());
    assert!(output.is_empty());

    // A part an earlier run left, held by no one, goes once the output is
    // there and is the file.
    DiskFiles::new(&out, &file)
        .unwrap()
        .append(b"partial")
        .unwrap();
    assert_eq!(entries(root.path()), 1);
    fs::write(&out, bytes()).unwrap();
    let answer = kept(&file, &out, &mut output).unwrap();
    assert!(answer.is_ok());
    let value = line(&output);
    assert_eq!(value["successful"], true);
    assert_eq!(value["connectionCheck"], "notPerformed");
    assert_eq!(value["ranges"], "0");
    assert_eq!(value["recheck"], Value::Null);
    // Only the output is left.
    assert_eq!(entries(root.path()), 1);

    fs::write(&out, b"someone's own file").unwrap();
    let mut output = vec![];
    let answer = kept(&file, &out, &mut output).unwrap();
    assert!(matches!(answer, Err(CommandError::LocalFile)));
    assert_eq!(
        line(&output)["fetchFailure"],
        json!({"code":"outputExists"})
    );
    assert_eq!(fs::read(&out).unwrap(), b"someone's own file");
}

/// Every line has the same keys, answered before or after dialling.
#[test]
fn a_line_answered_before_dialling_has_the_keys_of_one_after() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let served = bytes();
    let file = held(&served);
    let (_, fetched, _) = run(&out, &file, move |socket| {
        serve(socket, &served);
        serve(socket, &served);
    });
    let mut output = vec![];
    kept(&file, &out, &mut output).unwrap().unwrap();
    let answered = line(&output);
    let keys = |value: &Value| {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.retain(|key| key != "enrollment");
        keys
    };
    assert_eq!(keys(&answered), keys(&fetched));
}

#[test]
fn an_output_name_with_no_room_for_a_part_is_refused_before_dialling() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("n".repeat(218));
    let mut output = vec![];
    let answer = kept(&held(&bytes()), &out, &mut output).unwrap();
    assert!(matches!(answer, Err(CommandError::LocalFile)));
    let value = line(&output);
    assert_eq!(value["connectionCheck"], "notPerformed");
    assert_eq!(value["fetchFailure"], json!({"code":"file","step":"name"}));
    assert_eq!(entries(root.path()), 0);
}
