//! A settled view followed by a revoked unsubscribe still asks status, and the
//! socket is gone before `take_settled` returns.
use super::*;
use crate::infrastructure::{Monotonic, Wall};
use nessa_auth::adapters::pairing::{NativeIdentity, NativeTransport, OsEntropy};
use nessa_client_core::retained::{ReaderAccess, ReaderRole, RetainedCache};
use nessa_protocol::pairing::{
    encode_frame, wire::decode_request, EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES,
};
use nessa_protocol::product::generated::{
    product_event, product_method, ApprovalMode, CompactionReportingSupport,
    ConversationAgentFeatures, ConversationCapabilities, ConversationLifecycle,
    ConversationLifecyclePhase, ConversationMessage, ConversationMessageStatus, ConversationView,
    ElicitationForwardingSupport, IncomingElicitationSupport, ModelSwitchReportingSupport,
    NativeHookSuppressionSupport, NativeHooksChoice, PermissionDeferralSupport,
    PermissionDenialSupport, PolicyCloseSessionSupport, PolicyEndTurnSupport, PreToolPolicySupport,
    ProductSessionReady, SessionChallenge, MAX_RECORD_RESPONSE_BYTES, PRODUCT_VERSION,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage, RequestFrame, ResponseFrame};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Peer(Option<JoinHandle<()>>);
impl Peer {
    fn join(mut self) -> thread::Result<()> {
        self.0.take().unwrap().join()
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(peer) = self.0.take() {
            let result = peer.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "test peer panicked");
            }
        }
    }
}
struct PeerSocket {
    transport: NativeTransport<std::net::TcpStream>,
    frames: FrameReader,
}
impl PeerSocket {
    fn write_raw(&mut self, bytes: &[u8]) {
        self.transport.write_all(bytes).unwrap();
        self.transport.flush().unwrap();
    }
    fn read_text(&mut self) -> Option<String> {
        loop {
            let buffer = self.frames.unfilled().ok()?;
            let count = self
                .transport
                .read(buffer)
                .ok()
                .filter(|count| *count > 0)?;
            if let Some(body) = self.frames.filled(count).ok()? {
                return String::from_utf8(body).ok();
            }
        }
    }
    fn client_shut(&mut self) -> bool {
        let buffer = self.frames.unfilled().expect("frame buffer");
        match self.transport.read(buffer) {
            Ok(0) => true,
            Err(error) => !matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock),
            Ok(_) => false,
        }
    }
}
fn send(socket: &mut PeerSocket, message: OutgoingMessage) {
    let text = message.to_wire_text().unwrap();
    let frame = encode_frame(MAX_RECORD_RESPONSE_BYTES, text.as_bytes()).unwrap();
    socket.write_raw(&frame);
}
fn request(socket: &mut PeerSocket) -> RequestFrame {
    RequestFrame::decode(&socket.read_text().expect("text request")).unwrap()
}
fn ready() -> ProductSessionReady {
    ProductSessionReady {
        version: PRODUCT_VERSION,
        gateway_id: "gateway".into(),
        principal_id: "owner".into(),
        organization_id: "org".into(),
        membership_id: "membership".into(),
        credential_id: "credential".into(),
        audience_id: "gateway".into(),
        expires_at: None,
        grants: vec![],
        methods: vec![],
    }
}
fn settled_view(conversation: &str) -> ConversationView {
    ConversationView {
        model_change: None,
        model_change_effect: None,
        usage_limit_auto_continue: None,
        conversation_id: conversation.into(),
        revision: "rev".into(),
        server_now_ms: 1,
        messages: vec![ConversationMessage {
            execution_id: "exec".into(),
            user_text: "settled".into(),
            attachments: Vec::new(),
            files: Vec::new(),
            app: None,
            status: ConversationMessageStatus::Completed,
            error: None,
            authentication_required: None,
            usage_limit: None,
            required_work_failed: None,
            steering_target: None,
            parts: Vec::new(),
            steering_offset: None,
        }],
        pending: Vec::new(),
        permissions: Vec::new(),
        tools: Vec::new(),
        capabilities: ConversationCapabilities {
            queue: false,
            steer: false,
            resume: false,
            permissions: false,
            image_input: false,
            agent_features: ConversationAgentFeatures {
                permission_denial: PermissionDenialSupport::Unknown,
                native_hook_suppression: NativeHookSuppressionSupport::Unknown,
                native_hooks: NativeHooksChoice::Off,
                compaction_reporting: CompactionReportingSupport::UnsupportedNotImplemented,
                model_switch_reporting: ModelSwitchReportingSupport::UnsupportedNotImplemented,
                permission_deferral: PermissionDeferralSupport::UnsupportedNotImplemented,
                elicitation_forwarding: ElicitationForwardingSupport::Unknown,
                pre_tool_policy: PreToolPolicySupport::Unknown,
                policy_end_turn: PolicyEndTurnSupport::Unknown,
                policy_close_session: PolicyCloseSessionSupport::Unknown,
                incoming_elicitation: IncomingElicitationSupport::Unknown,
            },
        },
        lifecycle: ConversationLifecycle {
            phase: ConversationLifecyclePhase::Absent,
            failure: None,
            evidence_failure: None,
        },
        truncated: false,
        interaction_view_error: None,
        queue_complete: true,
        transcript_state: "complete".into(),
        runtime: None,
        lease: None,
        artifacts: Vec::new(),
        notes: Vec::new(),
        unreadable: Vec::new(),
        unreadable_truncated: false,
        title: None,
        questions: Vec::new(),
        approval_mode: ApprovalMode::Ask,
        approval_modes: Vec::new(),
        approval_mode_change: None,
    }
}
fn peer(
    script: impl FnOnce(&mut PeerSocket) + Send + 'static,
) -> (std::net::SocketAddr, [u8; 44], NativeIdentity, Peer) {
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = Peer(Some(thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        // The client dials after it opens its cache. A two-second accept
        // expires on a slow Windows runner, which then reports the wait as
        // WouldBlock (os error 10035).
        let deadline = Instant::now() + Duration::from_secs(15);
        let (stream, _) = loop {
            match listener.accept() {
                Ok(accepted) => {
                    accepted.0.set_nonblocking(false).unwrap();
                    accepted
                        .0
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    accepted
                        .0
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    break accepted;
                }
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("test peer accept failed: {error}"),
            }
        };
        let transport = NativeTransport::accept(stream, &gateway).unwrap();
        let mut selector = EnrollmentChannel::new(transport);
        let first = selector.receive_envelope().unwrap();
        assert!(matches!(
            decode_request(&first).unwrap(),
            nessa_protocol::pairing::wire::NativePairingRequest::OpenProduct
        ));
        let mut socket = PeerSocket {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_REQUEST_BYTES),
        };
        send(
            &mut socket,
            OutgoingMessage::Event(
                EventFrame::push(
                    product_event::SESSION_CHALLENGE,
                    &SessionChallenge {
                        min_version: PRODUCT_VERSION,
                        max_version: PRODUCT_VERSION,
                        nonce: "nonce".into(),
                        expires_at: 100,
                    },
                    0,
                    0,
                )
                .unwrap(),
            ),
        );
        let authentication = request(&mut socket);
        assert_eq!(authentication.method, product_method::SESSION_AUTHENTICATE);
        send(
            &mut socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&authentication.id, &ready()).unwrap(),
            ),
        );
        script(&mut socket);
    })));
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    (address, pin, device, thread)
}

#[test]
fn a_revoked_unsubscribe_after_a_settled_view_still_asks_status() {
    let (address, pin, device, gateway) = peer(|socket| {
        let opened = request(socket);
        assert_eq!(opened.method, product_method::CONVERSATION_SUBSCRIBE);
        let conversation = opened.params["conversationId"].as_str().unwrap().to_owned();
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &serde_json::json!({"subscriptionId": "1"}))
                    .unwrap(),
            ),
        );
        send(
            socket,
            OutgoingMessage::Event(
                EventFrame::push(
                    product_event::CONVERSATION_VIEW,
                    &serde_json::json!({
                        "subscriptionId": "1",
                        "cursor": {"incarnation": "inc", "position": "4"},
                        "view": settled_view(&conversation),
                    }),
                    1,
                    0,
                )
                .unwrap(),
            ),
        );
        let closing = request(socket);
        assert_eq!(closing.method, product_method::CONVERSATION_UNSUBSCRIBE);
        send(
            socket,
            OutgoingMessage::Response(ResponseFrame::failure(&closing.id, "forbidden", "revoked")),
        );
        assert!(socket.client_shut(), "unsubscribe refusal drops the socket");
    });
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    let cache = RetainedCache::open(
        &private.join("cache.sqlite3"),
        Arc::new(Wall),
        Arc::new(Monotonic::new()),
    )
    .unwrap();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let stop = ReadStop::new();
    let mut feed = cache
        .subscribe(
            ReaderAccess {
                role: ReaderRole::GrantedPeer,
                address,
                relay: None,
                identity: &device,
                pin,
                credential: "credential",
                receiver: "receiver",
                access_epoch: 3,
                client_id: "nessa-ios",
            },
            &conversation,
            None,
            &stop,
            Duration::from_secs(2),
        )
        .unwrap_or_else(|error| panic!("subscribe failed: {error:?}"));
    let (followed, authority) = super::take_settled(&mut feed);
    assert!(authority);
    assert!(followed
        .unwrap()
        .view_json()
        .contains("\"transcriptState\":\"complete\""));
    drop(feed);
    gateway.join().unwrap();
}
