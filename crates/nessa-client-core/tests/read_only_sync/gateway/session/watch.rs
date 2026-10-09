//! The session's one record watch against a real native peer: registration,
//! the inbox that keeps hints across operations, and the waits that read them
//! (committed change watches, rows W2, W3, W6, W10, W11, W15).
use super::*;
use crate::read_only_sync::application::watch::Wait;
use nessa_protocol::product::generated::{
    ConversationWatchRecordsParams, MAX_CHANGE_WATCH_ID_BYTES,
};
use nessa_protocol::product_contract::generated::{ChangeWatchEndReason, ChangeWatchErrorCode};

const WATCH: &str = "00000000-0000-4000-8000-000000000001-1";

fn params() -> ConversationWatchRecordsParams {
    ConversationWatchRecordsParams {
        conversation_id: "conversation".into(),
        receiver_id: Some("receiver".into()),
        access_epoch: Some("3".into()),
    }
}
fn event_bytes(name: &str, payload: serde_json::Value) -> Vec<u8> {
    let text = OutgoingMessage::Event(EventFrame::push(name, &payload, 2, 0).unwrap())
        .to_wire_text()
        .unwrap();
    encode_frame(MAX_RECORD_RESPONSE_BYTES, text.as_bytes()).unwrap()
}
fn changed(watch: &str) -> Vec<u8> {
    event_bytes(
        product_event::CONVERSATION_CHANGED,
        json!({ "watchId": watch }),
    )
}
fn response_bytes(id: &str, payload: serde_json::Value) -> Vec<u8> {
    let text = OutgoingMessage::Response(ResponseFrame::success(id, &payload).unwrap())
        .to_wire_text()
        .unwrap();
    encode_frame(MAX_RECORD_RESPONSE_BYTES, text.as_bytes()).unwrap()
}
/// The gateway's side of an admitted registration.
fn admit(socket: &mut PeerSocket) {
    let watch = request(socket);
    assert_eq!(watch.method, product_method::CONVERSATION_WATCH_RECORDS);
    assert_eq!(
        watch.params,
        json!({"conversationId":"conversation","receiverId":"receiver","accessEpoch":"3"})
    );
    socket.write_raw(&response_bytes(&watch.id, json!({ "watchId": WATCH })));
}
fn register(session: &mut Session) {
    session.begin().unwrap();
    assert_eq!(session.watch_records(&params()).unwrap(), WATCH);
    assert_eq!(session.finish().unwrap().failure, None);
}
fn head(session: &mut Session) -> Result<serde_json::Value, GatewayError> {
    session.rpc(
        product_method::CONVERSATION_RECORDS_HEAD,
        &json!({}),
        RpcKind::Record,
    )
}

/// Row W2: the generated request goes out and the generated result is the
/// watch identity; a result outside the generated shape, or longer than the
/// published identity bound, is a protocol failure.
#[test]
fn watch_registration_decodes_the_generated_result() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket);
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    register(&mut session);
    drop(session);
    gateway.join().unwrap();

    for result in [
        json!({"watchId": WATCH, "head": "9"}),
        json!({"watchId": "w".repeat(MAX_CHANGE_WATCH_ID_BYTES + 1)}),
    ] {
        let (endpoint, gateway) = peer(move |socket| {
            let watch = request(socket);
            socket.write_raw(&response_bytes(&watch.id, result));
            while !socket.ended() {}
        });
        let mut session = connect(&endpoint);
        session.begin().unwrap();
        assert_eq!(
            session.watch_records(&params()),
            Err(GatewayError::Protocol)
        );
        assert_eq!(
            session.finish().unwrap().failure,
            Some(GatewayError::Protocol)
        );
        drop(session);
        gateway.join().unwrap();
    }
}

/// Row W2: an acknowledgement whose identity is within the byte bound but
/// outside the generated `CHANGE_WATCH_ID_PATTERN` is a protocol failure, so
/// the watch loop ends before any `registered` line.
#[test]
fn watch_registration_refuses_an_identity_outside_the_published_pattern() {
    for watch_id in [
        "w",
        "00000000-0000-4000-8000-00000000000A-1",
        "00000000-0000-4000-8000-000000000001-0",
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000001-1 ",
    ] {
        assert!(watch_id.len() <= MAX_CHANGE_WATCH_ID_BYTES);
        let (endpoint, gateway) = peer(move |socket| {
            let watch = request(socket);
            socket.write_raw(&response_bytes(&watch.id, json!({ "watchId": watch_id })));
            while !socket.ended() {}
        });
        let mut session = connect(&endpoint);
        session.begin().unwrap();
        assert_eq!(
            session.watch_records(&params()),
            Err(GatewayError::Protocol),
            "{watch_id:?}"
        );
        assert_eq!(
            session.finish().unwrap().failure,
            Some(GatewayError::Protocol)
        );
        drop(session);
        gateway.join().unwrap();
    }
}

/// Row W3: a refusal keeps the gateway's typed watch code; a code outside the
/// generated set is a protocol failure.
#[test]
fn watch_refusals_keep_the_typed_code() {
    for (code, expected) in [
        (
            "watch_capacity",
            GatewayError::Watch(ChangeWatchErrorCode::WatchCapacity),
        ),
        (
            "stale_epoch",
            GatewayError::Watch(ChangeWatchErrorCode::StaleEpoch),
        ),
        ("record_too_large", GatewayError::Protocol),
    ] {
        let (endpoint, gateway) = peer(move |socket| {
            let watch = request(socket);
            send(
                socket,
                OutgoingMessage::Response(ResponseFrame::failure(&watch.id, code, "refused")),
            );
            while !socket.ended() {}
        });
        let mut session = connect(&endpoint);
        session.begin().unwrap();
        assert_eq!(session.watch_records(&params()), Err(expected));
        drop(session);
        gateway.join().unwrap();
    }
}

/// Row W6: twenty hints written with a response in one write, beyond the
/// policy's two unexpected events, are one dirty bit. The next wait returns
/// it at once as kept during a pass; the wait after that reads a new hint.
#[test]
fn hints_during_an_rpc_set_one_dirty_bit_and_do_not_count_as_events() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket);
        let read = request(socket);
        let mut bytes: Vec<u8> = (0..20).flat_map(|_| changed(WATCH)).collect();
        bytes.extend(response_bytes(&read.id, json!({"head":"7"})));
        bytes.extend(changed(WATCH));
        socket.write_raw(&bytes);
        // The trailing hint stays unread until the client's second wait.
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    register(&mut session);
    session.begin().unwrap();
    assert_eq!(head(&mut session).unwrap(), json!({"head":"7"}));
    assert_eq!(session.finish().unwrap().failure, None);
    session.begin().unwrap();
    assert_eq!(session.wait_hint(), Ok(Wait::Hint { during_pass: true }));
    session.finish().unwrap();
    session.begin().unwrap();
    assert_eq!(session.wait_hint(), Ok(Wait::Hint { during_pass: false }));
    assert_eq!(session.finish().unwrap().failure, None);
    drop(session);
    gateway.join().unwrap();
}

/// Row W10: the watch's end, read during an RPC, is kept and comes before a
/// hint kept with it.
#[test]
fn watch_ended_is_kept_in_the_inbox() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket);
        let read = request(socket);
        let mut bytes = changed(WATCH);
        bytes.extend(event_bytes(
            product_event::CONVERSATION_WATCH_ENDED,
            json!({"watchId": WATCH, "reason": "closed"}),
        ));
        bytes.extend(response_bytes(&read.id, json!({})));
        socket.write_raw(&bytes);
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    register(&mut session);
    session.begin().unwrap();
    head(&mut session).unwrap();
    session.finish().unwrap();
    for _ in 0..2 {
        session.begin().unwrap();
        assert_eq!(
            session.wait_hint(),
            Ok(Wait::Ended(ChangeWatchEndReason::Closed))
        );
        session.finish().unwrap();
    }
    drop(session);
    gateway.join().unwrap();
}

/// Row W11: the gateway closing the connection while the client waits is an
/// untyped close; the native profile carries no reason.
#[test]
fn peer_close_during_wait_is_closed_none() {
    let (endpoint, gateway) = peer(admit);
    let mut session = connect(&endpoint);
    register(&mut session);
    session.begin().unwrap();
    assert_eq!(session.wait_hint(), Err(GatewayError::Closed(None)));
    assert_eq!(
        session.finish().unwrap().failure,
        Some(GatewayError::Closed(None))
    );
    drop(session);
    gateway.join().unwrap();
}

/// Row W15: a watch event naming an identity this connection did not mint,
/// or arriving before any registration, is a protocol failure; a response
/// while only waiting is a correlation failure.
#[test]
fn foreign_watch_id_and_stray_response_are_protocol_failures() {
    let foreign = "00000000-0000-4000-8000-000000000002-1";
    // A foreign identity while waiting.
    let (endpoint, gateway) = peer(move |socket| {
        admit(socket);
        socket.write_raw(&changed(foreign));
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    register(&mut session);
    session.begin().unwrap();
    assert_eq!(session.wait_hint(), Err(GatewayError::Protocol));
    drop(session);
    gateway.join().unwrap();
    // A watch event during an RPC on a connection that registered nothing.
    let (endpoint, gateway) = peer(|socket| {
        let read = request(socket);
        let mut bytes = changed(WATCH);
        bytes.extend(response_bytes(&read.id, json!({})));
        socket.write_raw(&bytes);
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(head(&mut session), Err(GatewayError::Protocol));
    drop(session);
    gateway.join().unwrap();
    // A response while nothing is pending.
    let (endpoint, gateway) = peer(|socket| {
        admit(socket);
        socket.write_raw(&response_bytes("9", json!({})));
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    register(&mut session);
    session.begin().unwrap();
    assert_eq!(session.wait_hint(), Err(GatewayError::Correlation));
    drop(session);
    gateway.join().unwrap();
}
