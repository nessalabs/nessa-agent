//! `artifact.read` against a real native peer: the request names the file as
//! the view does, a busy answer keeps the connection for the same range,
//! refusals stay typed, and only canonical base64 is decoded.
use super::super::super::held_files::decode;
use super::*;
use crate::read_only_sync::application::held_file::{HeldFileSource, RangeAnswer};
use crate::read_only_sync::application::HeldFileRefusal;
use crate::read_only_sync::domain::{HeldFile, HELD_FILE_RANGE_BYTES, MAX_HELD_FILE_BYTES};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};

const CONVERSATION: &str = "00000000-0000-4000-8000-000000000001";

fn file() -> HeldFile {
    HeldFile::new(
        ConversationId::new(CONVERSATION).unwrap(),
        Sha256Digest::from_bytes([0xab; 32]),
        MediaType::parse("image/png").unwrap(),
        40_000,
    )
    .unwrap()
}
fn answer(socket: &mut PeerSocket, payload: serde_json::Value) {
    let read = request(socket);
    send(
        socket,
        OutgoingMessage::Response(ResponseFrame::success(&read.id, &payload).unwrap()),
    );
}
fn refuse(socket: &mut PeerSocket, code: &str) {
    let read = request(socket);
    assert_eq!(read.method, product_method::ARTIFACT_READ);
    send(
        socket,
        OutgoingMessage::Response(ResponseFrame::failure(&read.id, code, "refused")),
    );
}

/// The range size and the largest file are written once in the domain, as
/// the schema publishes them; this fails when the two differ.
#[test]
fn ranges_and_sizes_are_the_schema_bounds() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    );
    let schema: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let params = &schema["$defs"]["ArtifactReadParams"]["properties"];
    assert_eq!(params["maxBytes"]["maximum"], json!(HELD_FILE_RANGE_BYTES));
    assert_eq!(params["size"]["minimum"], json!(1));
    assert_eq!(params["size"]["maximum"], json!(MAX_HELD_FILE_BYTES));
}

#[test]
fn a_range_names_the_file_as_the_view_does_and_its_answer_is_decoded() {
    let (endpoint, peer) = peer(|socket| {
        let read = request(socket);
        assert_eq!(read.method, product_method::ARTIFACT_READ);
        assert_eq!(
            read.params,
            json!({
                "conversationId": CONVERSATION,
                "digest": format!("sha256:{}", "ab".repeat(32)),
                "mimeType": "image/png",
                "size": 40_000,
                "offset": HELD_FILE_RANGE_BYTES,
                "maxBytes": HELD_FILE_RANGE_BYTES,
            })
        );
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&read.id, &json!({"offset": 32768, "data": "aGVsbG8="}))
                    .unwrap(),
            ),
        );
    });
    let mut connection = GatewayConnection::new(connect(&endpoint));
    let range = file().range_at(HELD_FILE_RANGE_BYTES).unwrap();
    assert_eq!(
        connection.read(&file(), range),
        Ok(RangeAnswer {
            offset: HELD_FILE_RANGE_BYTES,
            bytes: b"hello".to_vec()
        })
    );
    drop(connection);
    peer.join().unwrap();
}

#[test]
fn a_busy_range_keeps_the_connection_for_the_same_range_again() {
    let (endpoint, peer) = peer(|socket| {
        refuse(
            socket,
            ConversationErrorCode::TemporarilyUnavailable.as_str(),
        );
        answer(socket, json!({"offset": 0, "data": "AA=="}));
    });
    let mut connection = GatewayConnection::new(connect(&endpoint));
    let range = file().range_at(0).unwrap();
    assert_eq!(
        connection.read(&file(), range),
        Err(GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::TemporarilyUnavailable
        )))
    );
    assert_eq!(
        connection.read(&file(), range),
        Ok(RangeAnswer {
            offset: 0,
            bytes: vec![0]
        })
    );
    drop(connection);
    peer.join().unwrap();
}

#[test]
fn refusals_are_typed_and_end_the_connection() {
    for (code, expected) in [
        (
            ConversationErrorCode::AttachmentNotFound.as_str(),
            GatewayError::HeldFile(HeldFileRefusal::Conversation(
                ConversationErrorCode::AttachmentNotFound,
            )),
        ),
        (
            ConversationErrorCode::ConversationDeleted.as_str(),
            GatewayError::HeldFile(HeldFileRefusal::Conversation(
                ConversationErrorCode::ConversationDeleted,
            )),
        ),
        (
            "forbidden",
            GatewayError::HeldFile(HeldFileRefusal::Forbidden),
        ),
        (
            "unauthorized",
            GatewayError::HeldFile(HeldFileRefusal::Unauthorized),
        ),
        // An older gateway without `artifact.read`: its own case.
        (
            ConversationErrorCode::UnknownMethod.as_str(),
            GatewayError::HeldFile(HeldFileRefusal::UnknownMethod),
        ),
        // A code this client was not taught is not passed on as if known.
        ("constructor", GatewayError::Protocol),
    ] {
        let (endpoint, peer) = peer(move |socket| refuse(socket, code));
        let mut connection = GatewayConnection::new(connect(&endpoint));
        let range = file().range_at(0).unwrap();
        assert_eq!(connection.read(&file(), range), Err(expected), "{code}");
        assert_eq!(connection.begin(), Err(GatewayError::Transport), "{code}");
        drop(connection);
        peer.join().unwrap();
    }
}

#[test]
fn an_undecodable_answer_fails_its_operation_and_closes_the_connection() {
    let (endpoint, peer) = peer(|socket| answer(socket, json!({"offset": 0, "data": "AA"})));
    let mut connection = GatewayConnection::new(connect(&endpoint));
    let range = file().range_at(0).unwrap();
    assert_eq!(connection.read(&file(), range), Err(GatewayError::Protocol));
    assert_eq!(connection.begin(), Err(GatewayError::Transport));
    drop(connection);
    peer.join().unwrap();
}

#[test]
fn only_canonical_padded_base64_is_decoded() {
    let decoded = |data: &str| decode(json!({"offset": 3, "data": data}));
    assert_eq!(
        decoded("QQ=="),
        Ok(RangeAnswer {
            offset: 3,
            bytes: b"A".to_vec()
        })
    );
    assert_eq!(decoded("").map(|answer| answer.bytes), Ok(vec![]));
    for spelling in [
        "QQ", "QR==", "QQ=", "QQ===", " QQ==", "QQ==\n", "Q-==", "QQ==QQ==",
    ] {
        assert_eq!(
            decoded(spelling),
            Err(GatewayError::Protocol),
            "{spelling:?}"
        );
    }
    for shape in [
        json!({"offset": 0}),
        json!({"offset": -1, "data": ""}),
        json!({"offset": 0, "data": "", "more": true}),
        json!({"offset": 0, "data": 1}),
    ] {
        assert_eq!(decode(shape), Err(GatewayError::Protocol));
    }
}
