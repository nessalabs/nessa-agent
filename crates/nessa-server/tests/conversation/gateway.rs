/// Product conversation boundaries exercise current authorization before any agent effect.
mod gateway {
    use super::super::conversation_support;
    use super::*;
    use crate::conversation::application::ConversationLimits;
    use crate::conversation::domain::ConversationId;
    use crate::protocol::ResponseFrame;
    use std::collections::HashMap;
    use tokio::sync::oneshot;

    struct ChatAuthority {
        snapshots: HashMap<CredentialId, AccessSnapshot>,
    }
    impl CredentialVerifier for ChatAuthority {
        fn verify<'a>(
            &'a self,
            evidence: &'a CredentialEvidence,
            _: &'a AudienceId,
        ) -> PortFuture<'a, VerifiedCredential> {
            Box::pin(async move {
                let id = std::str::from_utf8(evidence.expose_bytes())
                    .map_err(|_| AccessError::InvalidCredential)?;
                let id = CredentialId::new(id).map_err(|_| AccessError::InvalidCredential)?;
                if !self.snapshots.contains_key(&id) {
                    return Err(AccessError::InvalidCredential);
                }
                Ok(VerifiedCredential {
                    credential_id: id,
                    expires_at: Some(200),
                })
            })
        }
    }
    impl AccessReader for ChatAuthority {
        fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async move {
                self.snapshots
                    .get(id)
                    .cloned()
                    .ok_or(AccessError::InvalidCredential)
            })
        }
    }
    fn chat_snapshot(
        credential: &str,
        principal: &str,
        organization: &str,
        chat: bool,
    ) -> AccessSnapshot {
        let organization = OrganizationId::new(organization).unwrap();
        let principal = PrincipalId::new(principal).unwrap();
        let mut grants = vec![Grant::new(
            Action::new("server.read").unwrap(),
            Resource::new(
                organization.clone(),
                ResourceId::new("gateway-resource").unwrap(),
            ),
        )];
        if chat {
            grants.push(Grant::new(
                Action::new("conversation.write").unwrap(),
                Resource::new(
                    organization.clone(),
                    ResourceId::new("gateway-resource").unwrap(),
                ),
            ));
        }
        AccessSnapshot {
            credential: Credential::new(
                CredentialId::new(credential).unwrap(),
                principal.clone(),
                organization.clone(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                grants,
            )
            .unwrap(),
            membership: Membership::new(
                MembershipId::new(format!("member-{}", principal.as_str())).unwrap(),
                principal,
                organization,
                MembershipRole::Member,
                MembershipStatus::Active,
            ),
            revision: 1,
        }
    }
    fn chat_state() -> ProductRouteState {
        let (mut state, _) = fixture(MembershipRole::Member);
        let snapshots = [
            chat_snapshot("owner-phone", "owner", "organization", true),
            chat_snapshot("owner-panel", "owner", "organization", true),
            chat_snapshot("other", "other", "organization", true),
            chat_snapshot("foreign", "owner", "foreign", true),
            chat_snapshot("reader", "owner", "organization", false),
        ]
        .into_iter()
        .map(|snapshot| (snapshot.credential.id().clone(), snapshot))
        .collect();
        let authority = Arc::new(ChatAuthority { snapshots });
        state.access = authority.clone();
        state.verifier = authority;
        state
    }
    async fn chat_session(state: &ProductRouteState, credential: &str) -> AuthenticatedSession {
        AuthenticateSession {
            verifier: state.verifier.as_ref(),
            access: state.access.as_ref(),
            clock: state.clock.as_ref(),
        }
        .execute(
            &CredentialEvidence::new(credential.as_bytes().to_vec()).unwrap(),
            state.audience(),
        )
        .await
        .unwrap()
    }
    async fn chat_request(
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        method: &str,
        params: serde_json::Value,
    ) -> ResponseFrame {
        let mut frame = request("command", method);
        frame.params = params;
        let OutgoingMessage::Response(response) = dispatch(state, session, frame).await else {
            panic!("response expected")
        };
        response
    }
    #[tokio::test]
    async fn conversation_requires_chat_grant_and_a_gateway_that_runs_conversations() {
        let state = chat_state();
        for (credential, expected) in [
            ("reader", "forbidden"),
            ("owner-phone", "conversations_not_configured"),
            ("foreign", "forbidden"),
        ] {
            let session = chat_session(&state, credential).await;
            for method in [
                "conversation.create",
                "conversation.read",
                "conversation.send",
                "conversation.steer",
                "conversation.remove",
                "conversation.reorder",
                "conversation.answer",
                "conversation.cancel",
                "conversation.close",
            ] {
                let response = chat_request(&state, &session, method, json!({})).await;
                assert!(!response.ok);
                assert!(response.payload.is_none());
                assert_eq!(
                    response.error.unwrap().code,
                    expected,
                    "{credential} {method}"
                );
            }
        }
    }
    fn send_command(peer: &TestPeer, id: &str, method: &str, params: serde_json::Value) {
        peer.input
            .send(Ok(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            )))
            .unwrap();
    }
    async fn response(peer: &mut TestPeer) -> serde_json::Value {
        let Message::Text(text) = peer.message().await else {
            panic!("text response expected")
        };
        serde_json::from_str(&text).unwrap()
    }
    #[tokio::test]
    async fn normal_capacity_cannot_consume_reserved_conversation_control_capacity() {
        let state = chat_state();
        let held = state
            .requests
            .clone()
            .acquire_many_owned(128)
            .await
            .unwrap();
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        send_command(&peer, "normal", "server.health", json!({}));
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        send_command(
            &peer,
            "control",
            "conversation.close",
            json!({"conversationId":"00000000-0000-4000-8000-000000000001","requestId":"close"}),
        );
        let value = response(&mut peer).await;
        assert_eq!(value["id"], "control");
        assert_eq!(
            value["error"]["code"], "conversations_not_configured",
            "reserved control capacity must reach the handler"
        );
        drop(peer.input);
        task.await.unwrap();
        drop(held);
        assert_eq!(state.controls.available_permits(), 32);
    }
    #[tokio::test]
    async fn client_metadata_cannot_replace_verified_principal() {
        for spoof_field in [false, true] {
            let state = chat_state();
            let (socket, mut peer) = test_socket(None);
            let task = tokio::spawn(handle_socket(socket, state));
            let challenge = response(&mut peer).await;
            let client = if spoof_field {
                json!({"id":"victim","principalId":"victim"})
            } else {
                json!({"id":"victim"})
            };
            send_command(
                &peer,
                "authenticate",
                "session.authenticate",
                json!({"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],"credential":"owner-phone","client":client}),
            );
            let reply = response(&mut peer).await;
            if spoof_field {
                assert_eq!(reply["ok"], false);
                assert!(reply.get("payload").is_none());
            } else {
                assert_eq!(reply["ok"], true);
                assert_eq!(reply["payload"]["principalId"], "owner");
                assert_eq!(reply["payload"]["credentialId"], "owner-phone");
            }
            drop(peer.input);
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn authenticated_surfaces_share_one_agent_but_other_owners_cannot_read() {
        let (service, provider, repository, _) =
            conversation_support::fixture(ConversationLimits::default());
        let state = chat_state().with_conversations(Arc::new(service));
        let id = "00000000-0000-4000-8000-000000000002";
        for credential in ["owner-phone", "owner-panel"] {
            let session = chat_session(&state, credential).await;
            assert!(
                chat_request(
                    &state,
                    &session,
                    "conversation.create",
                    json!({"conversationId":id,"requestId":credential})
                )
                .await
                .ok
            );
            assert!(
                chat_request(
                    &state,
                    &session,
                    "conversation.read",
                    json!({"conversationId":id})
                )
                .await
                .ok
            );
        }
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
        let record = repository
            .records
            .lock()
            .unwrap()
            .get(&ConversationId::new(id).unwrap())
            .unwrap()
            .clone();
        assert_eq!(record.owner().as_str(), "owner");
        assert_eq!(record.creator_surface(), "owner-phone");
        assert_eq!(record.creation_action(), "owner-phone");
        for (credential, code) in [
            ("other", "conversation_not_found"),
            ("foreign", "forbidden"),
        ] {
            let session = chat_session(&state, credential).await;
            let reply = chat_request(
                &state,
                &session,
                "conversation.read",
                json!({"conversationId":id}),
            )
            .await;
            assert!(!reply.ok);
            assert!(reply.payload.is_none());
            assert_eq!(reply.error.unwrap().code, code);
        }
    }

    #[tokio::test]
    async fn conversation_wire_enforces_canonical_and_utf8_byte_limits() {
        let (service, _, _, _) = conversation_support::fixture(ConversationLimits::default());
        let state = chat_state().with_conversations(Arc::new(service));
        let session = chat_session(&state, "owner-phone").await;
        let id = "00000000-0000-4000-8000-000000000004";
        assert!(
            chat_request(
                &state,
                &session,
                "conversation.create",
                json!({"conversationId":id,"requestId":"create"}),
            )
            .await
            .ok
        );
        for params in [
            json!({"conversationId":"00000000-0000-4000-8000-00000000000A","requestId":"send","executionId":"execution","text":"hello"}),
            json!({"conversationId":id,"requestId":"😀".repeat(65),"executionId":"execution","text":"hello"}),
            json!({"conversationId":id,"requestId":"send","executionId":"😀".repeat(65),"text":"hello"}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"😀".repeat(2049)}),
        ] {
            let response =
                chat_request(&state, &session, "conversation.send", params).await;
            assert!(!response.ok);
            assert_eq!(response.error.unwrap().code, "invalid_request");
        }
        assert!(
            chat_request(
                &state,
                &session,
                "conversation.send",
                json!({"conversationId":id,"requestId":"😀".repeat(64),"executionId":"😀".repeat(64),"text":"😀".repeat(2048)}),
            )
            .await
            .ok
        );
    }

    #[tokio::test]
    async fn disconnect_keeps_admitted_open_and_capacity_until_it_finishes() {
        let (service, provider, _, _) =
            conversation_support::fixture(ConversationLimits::default());
        let state = chat_state().with_conversations(Arc::new(service));
        let (release, gate) = oneshot::channel();
        *provider.open_gate.lock().unwrap() = Some(gate);
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        let id = "00000000-0000-4000-8000-000000000003";
        send_command(
            &peer,
            "create",
            "conversation.create",
            json!({"conversationId":id,"requestId":"create"}),
        );
        timeout(Duration::from_secs(1), provider.opening.notified())
            .await
            .unwrap();
        assert_eq!(state.requests.available_permits(), 127);
        // The provider's opening future is blocked, but this socket remains responsive.
        send_command(&peer, "health", "server.health", json!({}));
        assert_eq!(response(&mut peer).await["id"], "health");
        drop(peer.input);
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.requests.available_permits(),
            127,
            "disconnect must not release an admitted task's capacity"
        );
        release.send(()).unwrap();
        let second = chat_session(&state, "owner-panel").await;
        let read = timeout(
            Duration::from_secs(1),
            chat_request(
                &state,
                &second,
                "conversation.read",
                json!({"conversationId":id}),
            ),
        )
        .await
        .unwrap();
        assert!(read.ok);
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
        timeout(Duration::from_secs(1), async {
            while state.requests.available_permits() != 128 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sixteen_blocked_reads_leave_socket_capacity_for_controls() {
        let (service, provider, _, _) =
            conversation_support::fixture(ConversationLimits::default());
        let state = chat_state().with_conversations(Arc::new(service));
        let (release, gate) = oneshot::channel();
        *provider.open_gate.lock().unwrap() = Some(gate);
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        let id = "00000000-0000-4000-8000-000000000004";
        send_command(
            &peer,
            "create",
            "conversation.create",
            json!({"conversationId":id,"requestId":"create"}),
        );
        timeout(Duration::from_secs(1), provider.opening.notified())
            .await
            .unwrap();
        for n in 0..15 {
            send_command(
                &peer,
                &format!("read-{n}"),
                "conversation.read",
                json!({"conversationId":id}),
            );
        }
        timeout(Duration::from_secs(1), async {
            while state.requests.available_permits() != 112 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        send_command(&peer, "full", "server.health", json!({}));
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        // An unrelated missing conversation proves the control is dispatched,
        // without waiting for this deliberately blocked provider context.
        send_command(
            &peer,
            "close",
            "conversation.close",
            json!({"conversationId":"00000000-0000-4000-8000-000000000005","requestId":"close"}),
        );
        let control = response(&mut peer).await;
        assert_eq!(control["id"], "close");
        assert_eq!(control["error"]["code"], "conversation_not_found");
        drop(peer.input);
        task.await.unwrap();
        assert_eq!(state.requests.available_permits(), 112);
        release.send(()).unwrap();
        timeout(Duration::from_secs(1), async {
            while state.requests.available_permits() != 128 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn authenticated_reorder_changes_provider_dispatch_order_atomically() {
        let (service, provider, _, _) =
            conversation_support::fixture(ConversationLimits::default());
        let state = chat_state().with_conversations(Arc::new(service));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        let id = "00000000-0000-4000-8000-000000000006";
        send_command(
            &peer,
            "create",
            "conversation.create",
            json!({"conversationId":id,"requestId":"create"}),
        );
        assert_eq!(response(&mut peer).await["ok"], true);
        let (release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        for execution in ["running", "first", "second"] {
            send_command(
                &peer,
                execution,
                "conversation.send",
                json!({"conversationId":id,"requestId":execution,"executionId":execution,"text":execution}),
            );
            assert_eq!(response(&mut peer).await["ok"], true);
            if execution == "running" {
                timeout(
                    Duration::from_secs(1),
                    provider.execution_started.notified(),
                )
                .await
                .unwrap();
            }
        }
        // Another principal cannot reorder a known conversation identity.
        let other = chat_session(&state, "other").await;
        let denied = chat_request(&state, &other, "conversation.reorder", json!({"conversationId":id,"requestId":"foreign-reorder","executionIds":["second","first"]})).await;
        assert!(!denied.ok);
        assert_eq!(denied.error.unwrap().code, "conversation_not_found");
        send_command(
            &peer,
            "before",
            "conversation.read",
            json!({"conversationId":id}),
        );
        let before = response(&mut peer).await;
        assert_eq!(
            before["payload"]["pending"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["executionId"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );

        send_command(
            &peer,
            "reorder",
            "conversation.reorder",
            json!({"conversationId":id,"requestId":"reorder-action","executionIds":["second","first"]}),
        );
        let reordered = response(&mut peer).await;
        assert_eq!(
            reordered["payload"],
            json!({"requestId":"reorder-action","outcome":"applied"})
        );
        send_command(
            &peer,
            "stale",
            "conversation.reorder",
            json!({"conversationId":id,"requestId":"stale-action","executionIds":["first"]}),
        );
        assert_eq!(
            response(&mut peer).await["payload"]["outcome"],
            "queue_changed"
        );
        send_command(
            &peer,
            "after",
            "conversation.read",
            json!({"conversationId":id}),
        );
        let after = response(&mut peer).await;
        assert_eq!(
            after["payload"]["pending"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["executionId"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(*provider.executions.lock().unwrap(), ["running"]);
        release.send(()).unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                let started = provider.execution_started.notified();
                if provider.executions.lock().unwrap().len() == 3 {
                    break;
                }
                started.await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            *provider.executions.lock().unwrap(),
            ["running", "second", "first"]
        );
        drop(peer.input);
        task.await.unwrap();
    }
}
