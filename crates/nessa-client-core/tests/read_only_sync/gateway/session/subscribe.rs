//! `conversation.subscribe` on the pinned session: the reply, a view that
//! arrives before that reply, the end that follows every kept view, and the
//! follow loop's wait for the newest view.
use super::*;
use crate::read_only_sync::application::follow::{Ended, SubscriptionEnd, ViewCursor, Wait};
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
        model_change: None,
        model_change_effect: None,
        model: None,
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
    send_cursor(socket, id, conversation, "inc", position, text);
}
fn send_cursor(
    socket: &mut PeerSocket,
    id: &str,
    conversation: &str,
    incarnation: &str,
    position: &str,
    text: &str,
) {
    send_state(
        socket,
        id,
        conversation,
        incarnation,
        position,
        text,
        "complete",
    );
}
fn send_state(
    socket: &mut PeerSocket,
    id: &str,
    conversation: &str,
    incarnation: &str,
    position: &str,
    text: &str,
    transcript_state: &str,
) {
    let mut value = viewed(id, position, text);
    value["cursor"]["incarnation"] = json!(incarnation);
    value["view"]["conversationId"] = json!(conversation);
    value["view"]["transcriptState"] = json!(transcript_state);
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
fn a_subscription_id_outside_the_contract_is_a_protocol_failure() {
    for id in ["", "0", "01", "10a", &"1".repeat(21)] {
        let refused = id.to_owned();
        let (endpoint, gateway) = peer(move |socket| {
            let opened = request(socket);
            send(
                socket,
                OutgoingMessage::Response(
                    ResponseFrame::success(&opened.id, &json!({"subscriptionId": refused}))
                        .unwrap(),
                ),
            );
            while !socket.ended() {}
        });
        let mut session = connect(&endpoint);
        session.begin().unwrap();
        assert_eq!(
            session.subscribe(&params(None)),
            Err(GatewayError::Protocol),
            "{id}"
        );
        assert_eq!(
            session.finish().unwrap().failure,
            Some(GatewayError::Protocol)
        );
        drop(session);
        gateway.join().unwrap();
    }
}

/// A view behind the subscribe cursor in the same incarnation is not delivered,
/// including one parked with the reply. An equal position is. Another
/// incarnation replaces the floor, and a lower position in that incarnation
/// is then skipped.
#[test]
fn a_view_behind_the_resume_cursor_is_skipped() {
    let (endpoint, gateway) = peer(|socket| {
        let opened = request(socket);
        assert_eq!(opened.params["after"]["incarnation"], "inc");
        assert_eq!(opened.params["after"]["position"], "10");
        send_cursor(socket, "1", "conversation", "inc", "9", "parked behind");
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&opened.id, &json!({"subscriptionId": "1"})).unwrap(),
            ),
        );
        send_cursor(socket, "1", "conversation", "inc", "9", "behind");
        send_cursor(socket, "1", "conversation", "inc", "10", "equal");
        send_cursor(socket, "1", "conversation", "next", "1", "replaced");
        send_cursor(
            socket,
            "1",
            "conversation",
            "next",
            "0",
            "behind replacement",
        );
        send_end(socket, "1");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert!(session
        .subscribe(&params(Some(ConversationViewCursor {
            incarnation: "inc".into(),
            position: "10".into(),
        })))
        .is_ok());
    session.finish().unwrap();
    session.begin().unwrap();
    let mut texts = Vec::new();
    loop {
        match session.next_delivery() {
            Ok(SubscriptionDelivery::View(viewed)) => {
                texts.push(viewed.view.messages[0].user_text.clone())
            }
            Ok(SubscriptionDelivery::Ended(_)) => break,
            Err(GatewayError::Transport | GatewayError::Closed(_)) => break,
            Err(error) => panic!("{error:?}"),
        }
    }
    assert_eq!(texts, ["equal", "replaced"]);
    drop(session);
    gateway.join().unwrap();
}

/// A transcript state outside the published set is not queued. `unknown` is
/// a published state and is delivered.
#[test]
fn a_view_with_an_unpublished_transcript_state_is_skipped() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        // Position 5 is ahead of the view that follows. Skipping the
        // unpublished state leaves the floor unset, so position 1 arrives.
        send_state(socket, "1", "conversation", "inc", "5", "future", "future");
        send_state(socket, "1", "conversation", "inc", "1", "kept", "unknown");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => {
            assert_eq!(viewed.view.messages[0].user_text, "kept");
            assert_eq!(viewed.view.transcript_state, "unknown");
        }
        SubscriptionDelivery::Ended(_) => panic!("the published state is delivered"),
    }
    drop(session);
    gateway.join().unwrap();
}

/// A view whose cursor follow would reject is skipped. A position of
/// `u64::MAX` and the longest published incarnation are still delivered.
#[test]
fn a_view_cursor_outside_the_contract_is_skipped() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        send_cursor(socket, "1", "conversation", "inc", "abc", "letters");
        send_cursor(socket, "1", "conversation", "", "1", "empty");
        send_cursor(socket, "1", "conversation", &"a".repeat(129), "1", "long");
        send_cursor(
            socket,
            "1",
            "conversation",
            "inc",
            "99999999999999999999",
            "overflow",
        );
        send_cursor(
            socket,
            "1",
            "conversation",
            &"é".repeat(128),
            "18446744073709551615",
            "kept",
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => {
            assert_eq!(viewed.view.messages[0].user_text, "kept");
            assert_eq!(viewed.cursor.position, "18446744073709551615");
            assert_eq!(viewed.cursor.incarnation.chars().count(), 128);
        }
        SubscriptionDelivery::Ended(_) => panic!("the valid view is delivered"),
    }
    drop(session);
    gateway.join().unwrap();
}

/// A resume cursor follow can send is kept with the end.
#[test]
fn a_resume_cursor_inside_the_contract_is_kept() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        send(
            socket,
            OutgoingMessage::Event(
                EventFrame::push(
                    product_event::CONVERSATION_SUBSCRIPTION_ENDED,
                    &json!({
                        "subscriptionId": "1",
                        "reason": ConversationSubscriptionEndReason::Lagging,
                        "lastDelivered": {
                            "incarnation": "inc",
                            "position": "18446744073709551615",
                        },
                    }),
                    1,
                    0,
                )
                .unwrap(),
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
        SubscriptionDelivery::Ended(ended) => {
            let cursor = ended.last_delivered.expect("the cursor is kept");
            assert_eq!(cursor.position, "18446744073709551615");
        }
        SubscriptionDelivery::View(_) => panic!("the end is delivered"),
    }
    drop(session);
    gateway.join().unwrap();
}

/// A resume cursor follow would reject is not offered with the end.
#[test]
fn a_resume_cursor_outside_the_contract_is_dropped() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        send(
            socket,
            OutgoingMessage::Event(
                EventFrame::push(
                    product_event::CONVERSATION_SUBSCRIPTION_ENDED,
                    &json!({
                        "subscriptionId": "1",
                        "reason": ConversationSubscriptionEndReason::Lagging,
                        "lastDelivered": {"incarnation": "inc", "position": "abc"},
                    }),
                    1,
                    0,
                )
                .unwrap(),
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
        SubscriptionDelivery::Ended(ended) => assert!(ended.last_delivered.is_none()),
        SubscriptionDelivery::View(_) => panic!("the end is delivered"),
    }
    drop(session);
    gateway.join().unwrap();
}

/// A view or end whose subscription id is outside the contract is skipped.
/// The longest published id is still accepted.
#[test]
fn a_view_id_outside_the_contract_is_skipped_and_a_full_length_id_is_kept() {
    let id = format!("9{}", "0".repeat(19));
    let live = id.clone();
    let (endpoint, gateway) = peer(move |socket| {
        admit(socket, &live);
        send_end(socket, "0");
        send_view(socket, "01", "1", "skipped");
        send_view(socket, &live, "1", "kept");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(session.subscribe(&params(None)).unwrap(), id);
    session.finish().unwrap();
    session.begin().unwrap();
    match session.next_delivery().unwrap() {
        SubscriptionDelivery::View(viewed) => assert_eq!(viewed.view.messages[0].user_text, "kept"),
        SubscriptionDelivery::Ended(_) => panic!("the valid view is delivered"),
    }
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

/// The dial registers a socket before the handshake. A failed open drops that
/// clone, so the peer sees the client shut the connection while the stop lives.
#[test]
fn a_failed_open_releases_the_stop_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = thread::spawn(move || {
        let (mut socket, _) = super::bounded_accept(&listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        let mut buffer = [0; 64];
        loop {
            match socket.read(&mut buffer) {
                Ok(0) => return,
                Ok(_) => continue,
                Err(error) if error.kind() == ErrorKind::ConnectionReset => return,
                Err(error)
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
                {
                    panic!("the stop still holds the socket")
                }
                Err(error) => panic!("peer read: {error}"),
            }
        }
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
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = NativeIdentity::generate(&mut OsEntropy)
        .unwrap()
        .public_spki();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let stop = ReadStop::new();
    let failed = match cache.subscribe(
        ReaderAccess {
            role: ReaderRole::GrantedPeer,
            address,
            relay: None,
            identity: &device,
            pin,
            credential: "credential",
            receiver: "receiver",
            access_epoch: 3,
            client_id: "example",
        },
        &conversation,
        None,
        &stop,
        Duration::from_millis(800),
    ) {
        Ok(_) => panic!("handshake against a silent peer opened"),
        Err(error) => error,
    };
    assert_ne!(failed.failure(), ReadFailure::Away);
    peer.join().unwrap();
    drop(stop);
}

fn send_refused_end(socket: &mut PeerSocket, id: &str, code: &str) {
    send(
        socket,
        OutgoingMessage::Event(
            EventFrame::push(
                product_event::CONVERSATION_SUBSCRIPTION_ENDED,
                &json!({
                    "subscriptionId": id,
                    "reason": ConversationSubscriptionEndReason::Refused,
                    "code": code,
                }),
                1,
                0,
            )
            .unwrap(),
        ),
    );
}
fn head(session: &mut Session) -> Result<serde_json::Value, GatewayError> {
    session.rpc(
        product_method::CONVERSATION_RECORDS_HEAD,
        &json!({}),
        RpcKind::Record,
    )
}
fn at(position: &str) -> ViewCursor {
    ViewCursor {
        incarnation: "inc".into(),
        position: position.into(),
    }
}
fn subscribed(session: &mut Session) {
    session.begin().unwrap();
    session.subscribe(&params(None)).unwrap();
    assert_eq!(session.finish().unwrap().failure, None);
}

/// Views that arrive during another operation, beyond the policy's
/// unexpected events, are kept and do not count as events. The next wait
/// returns the newest at once as kept during a pass and drops the older ones;
/// the wait after that reads a new view.
#[test]
fn views_during_a_read_come_out_newest_first_and_once() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        let read = request(socket);
        for position in ["2", "3", "4", "5", "6"] {
            send_view(socket, "1", position, position);
        }
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&read.id, &json!({"head":"7"})).unwrap(),
            ),
        );
        send_view(socket, "1", "7", "seven");
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    subscribed(&mut session);
    session.begin().unwrap();
    assert_eq!(head(&mut session).unwrap(), json!({"head":"7"}));
    assert_eq!(session.finish().unwrap().failure, None);
    session.begin().unwrap();
    assert_eq!(
        session.wait_view(),
        Ok(Wait::View {
            cursor: at("6"),
            during_pass: true
        })
    );
    session.finish().unwrap();
    session.begin().unwrap();
    assert_eq!(
        session.wait_view(),
        Ok(Wait::View {
            cursor: at("7"),
            during_pass: false
        })
    );
    assert_eq!(session.finish().unwrap().failure, None);
    drop(session);
    gateway.join().unwrap();
}

/// The end comes before views kept with it, and a later wait finds the
/// subscription closed. A refused end carries its typed refusal, so an
/// authority refusal asks the pinned status again; other ends carry none.
#[test]
fn the_end_comes_first_and_a_refused_end_keeps_its_code() {
    for (code, cause) in [
        (
            Some("forbidden"),
            Some(GatewayError::Subscription(SubscriptionRefusal::Forbidden)),
        ),
        (
            Some("conversation_not_found"),
            Some(GatewayError::Subscription(
                SubscriptionRefusal::Conversation(
                    nessa_protocol::product_contract::generated::ConversationErrorCode::ConversationNotFound,
                ),
            )),
        ),
        (None, None),
    ] {
        let (endpoint, gateway) = peer(move |socket| {
            admit(socket, "1");
            // A view and then the end, both during another read.
            let read = request(socket);
            send_view(socket, "1", "2", "two");
            match code {
                Some(code) => send_refused_end(socket, "1", code),
                None => send_end(socket, "1"),
            }
            send(
                socket,
                OutgoingMessage::Response(
                    ResponseFrame::success(&read.id, &json!({"head":"2"})).unwrap(),
                ),
            );
            while !socket.ended() {}
        });
        let mut session = connect(&endpoint);
        subscribed(&mut session);
        session.begin().unwrap();
        assert_eq!(head(&mut session).unwrap(), json!({"head":"2"}));
        assert_eq!(session.finish().unwrap().failure, None);
        session.begin().unwrap();
        let reason = if code.is_some() {
            SubscriptionEnd::Refused
        } else {
            SubscriptionEnd::SourceClosed
        };
        let Ok(Wait::Ended(ended)) = session.wait_view() else {
            panic!("the end comes before the view kept with it")
        };
        assert_eq!(ended, Ended { reason, cause });
        session.finish().unwrap();
        session.begin().unwrap();
        assert_eq!(session.wait_view(), Err(GatewayError::Closed(None)));
        drop(session);
        gateway.join().unwrap();
    }
}

/// The gateway closing the connection while the client waits is an untyped
/// close (row W11); a response while only waiting is a correlation failure;
/// a wait with no subscription is refused before any read.
#[test]
fn a_wait_fails_on_close_stray_response_or_no_subscription() {
    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
    });
    let mut session = connect(&endpoint);
    subscribed(&mut session);
    session.begin().unwrap();
    assert_eq!(session.wait_view(), Err(GatewayError::Closed(None)));
    assert_eq!(
        session.finish().unwrap().failure,
        Some(GatewayError::Closed(None))
    );
    drop(session);
    gateway.join().unwrap();

    let (endpoint, gateway) = peer(|socket| {
        admit(socket, "1");
        send(
            socket,
            OutgoingMessage::Response(ResponseFrame::success("9", &json!({})).unwrap()),
        );
        while !socket.ended() {}
    });
    let mut session = connect(&endpoint);
    subscribed(&mut session);
    session.begin().unwrap();
    assert_eq!(session.wait_view(), Err(GatewayError::Correlation));
    drop(session);
    gateway.join().unwrap();

    let (endpoint, gateway) = peer(|socket| while !socket.ended() {});
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(session.wait_view(), Err(GatewayError::Busy));
    drop(session);
    gateway.join().unwrap();
}
