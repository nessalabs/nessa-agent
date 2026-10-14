//! `conversation.subscribe` on the pinned session: the reply, a view that
//! arrives before that reply, and the end that follows every kept view.
use super::*;
use crate::read_only_sync::application::{SubscriptionCode, SubscriptionRefusal};
use crate::read_only_sync::infrastructure::gateway::SubscriptionDelivery;
use nessa_protocol::product::generated::{
    ApprovalMode, CompactionReportingSupport, ConversationAgentFeatures, ConversationCapabilities,
    ConversationLifecycle, ConversationLifecyclePhase, ConversationMessage,
    ConversationMessageStatus, ConversationSubscribeParams, ConversationSubscriptionEndReason,
    ConversationView, ConversationViewCursor, ElicitationForwardingSupport,
    IncomingElicitationSupport, ModelSwitchReportingSupport, NativeHookSuppressionSupport,
    NativeHooksChoice, PermissionDeferralSupport, PermissionDenialSupport,
    PolicyCloseSessionSupport, PolicyEndTurnSupport, PreToolPolicySupport,
};

fn params(after: Option<ConversationViewCursor>) -> ConversationSubscribeParams {
    ConversationSubscribeParams {
        conversation_id: "conversation".into(),
        after,
    }
}
fn blank(text: &str) -> ConversationView {
    ConversationView {
        conversation_id: "conversation".into(),
        revision: "rev".into(),
        server_now_ms: 1,
        messages: vec![ConversationMessage {
            execution_id: "exec".into(),
            user_text: text.into(),
            attachments: Vec::new(),
            files: Vec::new(),
            app: None,
            status: ConversationMessageStatus::Completed,
            error: None,
            authentication_required: None,
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
        title: None,
        questions: Vec::new(),
        approval_mode: ApprovalMode::Ask,
        approval_modes: Vec::new(),
        approval_mode_change: None,
    }
}
fn viewed(id: &str, position: &str, text: &str) -> serde_json::Value {
    json!({
        "subscriptionId": id,
        "cursor": {"incarnation": "inc", "position": position},
        "view": blank(text),
    })
}
fn send_view(socket: &mut PeerSocket, id: &str, position: &str, text: &str) {
    send_view_of(socket, id, "conversation", position, text);
}
fn send_view_of(socket: &mut PeerSocket, id: &str, conversation: &str, position: &str, text: &str) {
    let mut value = viewed(id, position, text);
    value["view"]["conversationId"] = json!(conversation);
    send(
        socket,
        OutgoingMessage::Event(
            EventFrame::push(product_event::CONVERSATION_VIEW, &value, 1, 0).unwrap(),
        ),
    );
}
fn send_end(socket: &mut PeerSocket, id: &str) {
    send(
        socket,
        OutgoingMessage::Event(
            EventFrame::push(
                product_event::CONVERSATION_SUBSCRIPTION_ENDED,
                &json!({
                    "subscriptionId": id,
                    "reason": ConversationSubscriptionEndReason::SourceClosed,
                }),
                1,
                0,
            )
            .unwrap(),
        ),
    );
}
fn admit(socket: &mut PeerSocket, id: &str) -> RequestFrame {
    let request = request(socket);
    assert_eq!(request.method, product_method::CONVERSATION_SUBSCRIBE);
    send(
        socket,
        OutgoingMessage::Response(
            ResponseFrame::success(&request.id, &json!({"subscriptionId": id})).unwrap(),
        ),
    );
    request
}

/// A view framed with the subscribe reply is the first delivery, and the
/// request carries the cursor the caller is resuming from.
#[test]
fn a_view_before_the_subscribe_reply_is_delivered_first() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = request(socket);
        assert_eq!(opened.method, product_method::CONVERSATION_SUBSCRIBE);
        assert_eq!(opened.params["conversationId"], "conversation");
        assert!(opened.params.get("after").is_none());
        send_view(socket, "1", "4", "before");
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &json!({"subscriptionId": "1"})).unwrap(),
            ),
        );
        let closing = request(socket);
        assert_eq!(closing.method, product_method::CONVERSATION_UNSUBSCRIBE);
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&closing.id, &json!({"subscriptionId": "1"})).unwrap(),
            ),
        );
        let again = request(socket);
        assert_eq!(again.params["after"]["incarnation"], "inc");
        assert_eq!(again.params["after"]["position"], "4");
        send_view(socket, "2", "4", "resumed");
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&again.id, &json!({"subscriptionId": "2"})).unwrap(),
            ),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(session.subscribe(&params(None)).unwrap(), "1");
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => {
            assert_eq!(viewed.subscription_id, "1");
            assert_eq!(viewed.cursor.position, "4");
            assert_eq!(viewed.view.messages[0].user_text, "before");
        }
        SubscriptionDelivery::Ended(_) => panic!("the parked view comes before the end"),
    }
    session.finish().unwrap();
    session.begin().unwrap();
    session.unsubscribe().unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    assert_eq!(
        session
            .subscribe(&params(Some(ConversationViewCursor {
                incarnation: "inc".into(),
                position: "4".into(),
            })))
            .unwrap(),
        "2"
    );
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => {
            assert_eq!(viewed.view.messages[0].user_text, "resumed")
        }
        SubscriptionDelivery::Ended(_) => panic!("resubscribe delivers its view"),
    }
    drop(session);
    gateway.join().unwrap();
}

/// A view outside the generated shape is skipped. The next valid view is kept.
#[test]
fn a_malformed_view_is_skipped() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = admit(socket, "1");
        let _ = opened;
        send(
            socket,
            OutgoingMessage::Event(
                EventFrame::push(
                    product_event::CONVERSATION_VIEW,
                    &json!({"subscriptionId": ""}),
                    1,
                    0,
                )
                .unwrap(),
            ),
        );
        send_view(socket, "1", "1", "kept");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(session.subscribe(&params(None)).unwrap(), "1");
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => assert_eq!(viewed.view.messages[0].user_text, "kept"),
        SubscriptionDelivery::Ended(_) => panic!("the valid view is delivered"),
    }
    drop(session);
    gateway.join().unwrap();
}

/// A frame for another subscription is a broken session.
#[test]
fn a_foreign_subscription_id_is_a_protocol_failure() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        send_view(socket, "2", "1", "foreign");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    assert!(matches!(
        session.next_delivery(),
        Err(GatewayError::Protocol)
    ));
    assert_eq!(
        session.finish().unwrap().failure,
        Some(GatewayError::Protocol)
    );
    drop(session);
    gateway.join().unwrap();
}

/// The end comes after views already kept, and a view after that end is not delivered.
#[test]
fn the_end_follows_kept_views_and_a_later_view_is_ignored() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = request(socket);
        send_view(socket, "1", "1", "one");
        send_end(socket, "1");
        send_view(socket, "1", "2", "after");
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &json!({"subscriptionId": "1"})).unwrap(),
            ),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => assert_eq!(viewed.view.messages[0].user_text, "one"),
        SubscriptionDelivery::Ended(_) => panic!("the view comes before the end"),
    }
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::Ended(ended) => {
            assert_eq!(
                ended.reason,
                ConversationSubscriptionEndReason::SourceClosed
            )
        }
        SubscriptionDelivery::View(_) => panic!("the end is next"),
    }
    assert!(matches!(
        session.next_delivery(),
        Err(GatewayError::Closed(None))
    ));
    drop(session);
    gateway.join().unwrap();
}

/// Five views parked before the reply keep the newest four, in arrival order.
#[test]
fn a_burst_keeps_the_newest_views() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = request(socket);
        for text in ["1", "2", "3", "4", "5"] {
            send_view(socket, "1", text, text);
        }
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &json!({"subscriptionId": "1"})).unwrap(),
            ),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    let mut texts = Vec::new();
    for _ in 0..4 {
        match session.next_delivery().unwrap() {
            SubscriptionDelivery::View(viewed) => {
                texts.push(viewed.view.messages[0].user_text.clone())
            }
            SubscriptionDelivery::Ended(_) => panic!("views are still queued"),
        }
    }
    assert_eq!(texts, ["2", "3", "4", "5"]);
    drop(session);
    gateway.join().unwrap();
}

/// Unsubscribe drops the inbox. A second unsubscribe does not invent a server call.
#[test]
fn unsubscribe_drops_the_inbox() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "7");
        let closing = request(socket);
        assert_eq!(closing.method, product_method::CONVERSATION_UNSUBSCRIBE);
        assert_eq!(closing.params["subscriptionId"], "7");
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&closing.id, &json!({"subscriptionId": "7"})).unwrap(),
            ),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    session.unsubscribe().unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    assert_eq!(session.unsubscribe(), Err(GatewayError::Busy));
    assert_eq!(session.finish().unwrap().failure, None);
    drop(session);
    gateway.join().unwrap();
}

/// Typed subscribe refusals leave the connection usable. An empty id does not.
#[test]
fn typed_refusals_keep_the_connection() {
    let (endpoint, gateway) = peer(|socket| {
        for _ in 0..4 {
            let opened = request(socket);
            let code = opened.params["conversationId"].as_str().unwrap().to_owned();
            send(
                socket,
                OutgoingMessage::Response(ResponseFrame::failure(&opened.id, &code, "refused")),
            );
        }
        let head = request(socket);
        assert_eq!(head.method, product_method::CONVERSATION_RECORDS_HEAD);
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&head.id, &json!({"ok": true})).unwrap(),
            ),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    let cases = [
        (
            "forbidden",
            GatewayError::Subscription(SubscriptionRefusal::Forbidden),
        ),
        (
            "conversation_not_found",
            GatewayError::Subscription(SubscriptionRefusal::Conversation(
                nessa_protocol::product_contract::generated::ConversationErrorCode::ConversationNotFound,
            )),
        ),
        (
            "cursor_ahead",
            GatewayError::Subscription(SubscriptionRefusal::Subscription(SubscriptionCode::CursorAhead)),
        ),
        (
            "subscription_duplicate",
            GatewayError::Subscription(SubscriptionRefusal::Subscription(SubscriptionCode::Duplicate)),
        ),
    ];
    for (code, expected) in cases {
        session.begin().unwrap();
        assert_eq!(
            session.subscribe(&ConversationSubscribeParams {
                conversation_id: code.into(),
                after: None,
            }),
            Err(expected)
        );
        session.finish().unwrap();
    }
    session.begin().unwrap();
    session
        .rpc(
            product_method::CONVERSATION_RECORDS_HEAD,
            &json!({}),
            RpcKind::Record,
        )
        .unwrap();
    drop(session);
    gateway.join().unwrap();
}

#[test]
fn an_empty_subscription_id_is_a_protocol_failure() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = request(socket);
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &json!({"subscriptionId": ""})).unwrap(),
            ),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(
        session.subscribe(&params(None)),
        Err(GatewayError::Protocol)
    );
    assert_eq!(
        session.finish().unwrap().failure,
        Some(GatewayError::Protocol)
    );
    drop(session);
    gateway.join().unwrap();
}

/// A view whose subscription id matches, and whose conversation does not, is
/// not delivered.
#[test]
fn a_view_for_another_conversation_is_a_protocol_failure() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        send_view_of(socket, "1", "other", "1", "foreign");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    assert!(matches!(
        session.next_delivery(),
        Err(GatewayError::Protocol)
    ));
    drop(session);
    gateway.join().unwrap();
}

/// A view parked with the subscribe reply is rejected before the inbox keeps it.
#[test]
fn a_parked_view_for_another_conversation_fails_the_subscribe() {
    let (endpoint, gateway) = peer(|socket| {
        let _opened = request(socket);
        send_view_of(socket, "1", "other", "1", "foreign");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(
        session.subscribe(&params(None)),
        Err(GatewayError::Protocol)
    );
    drop(session);
    gateway.join().unwrap();
}

/// Unsubscribe `forbidden` is returned, and the socket is gone before the feed
/// is dropped, so the server subscription does not stay live.
#[test]
fn a_forbidden_unsubscribe_closes_the_socket_and_asks_status() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = request(socket);
        let conversation = opened.params["conversationId"].as_str().unwrap().to_owned();
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &json!({"subscriptionId": "1"})).unwrap(),
            ),
        );
        send_view_of(socket, "1", &conversation, "1", "settled");
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
        Arc::new(Time(Instant::now())),
    )
    .unwrap();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let stop = ReadStop::new();
    let mut feed = cache
        .subscribe(
            ReaderAccess {
                role: ReaderRole::GrantedPeer,
                address: endpoint.address,
                relay: None,
                identity: &endpoint.device,
                pin: endpoint.pin,
                credential: "credential",
                receiver: "receiver",
                access_epoch: 3,
                client_id: "example",
            },
            &conversation,
            None,
            &stop,
            Duration::from_secs(2),
        )
        .unwrap_or_else(|error| panic!("subscribe failed: {error:?}"));
    assert!(matches!(
        feed.next_event().unwrap(),
        crate::retained::FeedEvent::View { .. }
    ));
    let error = feed.close().unwrap_err();
    assert!(error.asks_status());
    assert_eq!(error.failure(), ReadFailure::Refused);
    drop(feed);
    gateway.join().unwrap();
}
