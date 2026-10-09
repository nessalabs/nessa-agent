/// Product conversation boundaries exercise current authorization before any agent effect.
mod gateway {
    use super::super::conversation_support;
    use super::*;
    use crate::conversation::application::ConversationLimits;
    use nessa_protocol::conversation::domain::ConversationId;
    use nessa_protocol::protocol::ResponseFrame;
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
    pub(super) fn chat_state() -> ProductRouteState {
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
        state.with_agents_catalog(
            serde_json::from_value(serde_json::json!({
                "agents": [{
                    "agent": "claude",
                    "defaultModel": "test",
                    "models": [{
                        "modelId": "test",
                        "displayName": "Test model",
                        "maxContextWindowTokens": 100000,
                        "reasoning": false,
                        "imageInput": false,
                        "approvalModes": [{
                            "id": "ask",
                            "name": "Provider asks",
                            "description": "The provider requests approval where required."
                        }]
                    }]
                }]
            }))
            .unwrap(),
        )
    }
    pub(super) async fn chat_session(state: &ProductRouteState, credential: &str) -> AuthenticatedSession {
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
    pub(super) async fn chat_request(
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
    /// The socket admits creation, submit, and stop only through the shared
    /// command store. The directory is retained for the process: these tests
    /// keep the service, and dropping the directory under it would make later
    /// commands fail for a reason they are not asserting.
    fn bind_command_store(service: &crate::conversation::application::ConversationService) {
        let directory = tempfile::tempdir().expect("command store");
        let storage = Arc::new(
            nessa_sdk::infrastructure::session_storage::RecordStorage::new(
                directory.path().join("records"),
            )
            .expect("command store"),
        );
        service.bind_commands(storage);
        std::mem::forget(directory);
    }
    fn chat_fixture() -> (
        crate::conversation::application::ConversationService,
        Arc<conversation_support::ProviderFactory>,
        Arc<conversation_support::MemoryRepository>,
        Arc<nessa_sdk::infrastructure::session_storage::InMemoryStorage>,
    ) {
        let built = conversation_support::fixture(ConversationLimits::default());
        bind_command_store(&built.0);
        built
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
                "conversation.list",
                "conversation.send",
                "conversation.steer",
                "conversation.remove",
                "conversation.reorder",
                "conversation.answer",
                "conversation.cancel",
                "conversation.close",
                "conversation.stop",
                "conversation.receipt",
                "conversation.archive",
                "conversation.unarchive",
                "conversation.delete",
                "conversation.setApprovalMode",
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
    #[tokio::test]
    async fn the_agent_catalog_requires_the_gateway_read_grant() {
        let state = chat_state();
        for (credential, allowed) in [
            ("reader", true),
            ("owner-phone", true),
            ("foreign", false),
        ] {
            let session = chat_session(&state, credential).await;
            let response = chat_request(&state, &session, "agents.list", json!({})).await;
            assert_eq!(response.ok, allowed, "{credential}");
            if allowed {
                assert_eq!(response.payload.unwrap()["agents"][0]["agent"], "claude");
            } else {
                assert_eq!(response.error.unwrap().code, "forbidden");
            }
        }
    }
    pub(super) fn send_command(peer: &TestPeer, id: &str, method: &str, params: serde_json::Value) {
        peer.input
            .send(Ok(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            )))
            .unwrap();
    }
    pub(super) async fn response(peer: &mut TestPeer) -> serde_json::Value {
        // Durable create and submit write their receipt before answering.
        // A loaded Windows runner can take longer than the one-second bound
        // used for socket deadlines that must stay prompt.
        let Message::Text(text) = timeout(Duration::from_secs(10), peer.output.recv())
            .await
            .expect("conversation command response")
            .expect("socket closed")
        else {
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
        // Archiving is a control too: a person tidying their list is not made
        // to wait behind reads and provider opens. So is deleting, in a pool
        // of its own.
        for method in [
            "conversation.close",
            "conversation.stop",
            "conversation.archive",
            "conversation.unarchive",
            "conversation.delete",
        ] {
            send_command(
                &peer,
                method,
                method,
                json!({"conversationId":"00000000-0000-4000-8000-000000000001","requestId":"control"}),
            );
            let value = response(&mut peer).await;
            assert_eq!(value["id"], method);
            assert_eq!(
                value["error"]["code"], "conversations_not_configured",
                "reserved control capacity must reach the handler for {method}"
            );
        }
        drop(peer.input);
        task.await.unwrap();
        drop(held);
        assert_eq!(state.controls.available_permits(), 32);
    }
    #[tokio::test]
    async fn deletes_and_controls_never_take_each_others_place() {
        let state = chat_state();
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        let target = |request: &str| {
            json!({"conversationId":"00000000-0000-4000-8000-000000000001","requestId":request})
        };
        let answer = json!({"conversationId":"00000000-0000-4000-8000-000000000001","requestId":"answer","executionId":"turn","permissionId":"p","optionId":"allow"});
        let cancel = json!({"conversationId":"00000000-0000-4000-8000-000000000001","requestId":"cancel","executionId":"turn","permissionId":"p","reason":"no"});

        // Every delete slot taken, as a burst of slow deletes would leave
        // them: another delete is turned away, and a permission answer, a
        // cancel and a close are still admitted and reach the service.
        let deletes = state.deletions.clone().acquire_many_owned(8).await.unwrap();
        send_command(&peer, "delete", "conversation.delete", target("delete"));
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        for (id, method, params) in [
            ("answer", "conversation.answer", answer.clone()),
            ("cancel", "conversation.cancel", cancel),
            ("close", "conversation.close", target("close")),
        ] {
            send_command(&peer, id, method, params);
            assert_eq!(
                response(&mut peer).await["error"]["code"],
                "conversations_not_configured",
                "{method} was not admitted"
            );
        }
        assert_eq!(state.controls.available_permits(), 32);
        drop(deletes);

        // And the other way about: with every control slot taken, a delete is
        // still admitted.
        let controls = state.controls.clone().acquire_many_owned(32).await.unwrap();
        send_command(&peer, "answer-2", "conversation.answer", answer);
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        // Archiving and unarchiving are controls: they wait for the control
        // pool, not the requests' or the deletes'.
        for method in ["conversation.archive", "conversation.unarchive"] {
            send_command(&peer, method, method, target(method));
            assert_eq!(
                response(&mut peer).await["error"]["code"],
                "temporarily_unavailable",
                "{method} is a control"
            );
        }
        send_command(&peer, "delete-2", "conversation.delete", target("delete-2"));
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "conversations_not_configured"
        );
        drop(controls);
        drop(peer.input);
        task.await.unwrap();
        assert_eq!(state.deletions.available_permits(), 8);
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
                json!({"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],"credential":"owner-phone","client":client,"surface":{"kind":"cli","instance":"probe"}}),
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
            chat_fixture();
        let state = chat_state().with_conversations(Arc::new(service));
        let id = "00000000-0000-4000-8000-000000000002";
        let phone = chat_session(&state, "owner-phone").await;
        assert!(
            chat_request(
                &state,
                &phone,
                "conversation.create",
                json!({"conversationId":id,"requestId":"owner-phone"})
            )
            .await
            .ok
        );
        let panel = chat_session(&state, "owner-panel").await;
        assert!(
            chat_request(
                &state,
                &panel,
                "conversation.create",
                json!({"conversationId":id,"requestId":"owner-panel"}),
            )
            .await
            .ok
        );
        for session in [&phone, &panel] {
            assert!(
                chat_request(
                    &state,
                    session,
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
    async fn read_joins_the_fixed_selection_to_the_authenticated_catalog() {
        let (_, provider, repository, storage) =
            chat_fixture();
        let service = crate::conversation::application::ConversationService::new(
            crate::conversation::application::ConversationDependencies {
                agents: conversation_support::only(Arc::new(conversation_support::Provider::new(
                    provider,
                ))),
                storage,
                metadata: repository.clone(),
                creation_audit: Arc::new(conversation_support::AcceptingCreationAudit),
                mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
                file_link_audit: Arc::new(
                    conversation_support::RecordingFileLinkAudit::default(),
                ),
                deletion_audit: Arc::new(conversation_support::AcceptingDeletionAudit),
                attachments: None,
                summaries: Arc::new(conversation_support::MemorySummaries::default()),
                listing: Arc::new(conversation_support::Unlisted),
                provider_sessions:
                    crate::conversation::application::ProviderSessionErasers::default(),
                deletion_budgets: conversation_support::DELETION_BUDGETS,
                message_commit_clock: Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
                clock: Arc::new(conversation_support::TestClock),
                environment: crate::conversation::infrastructure::in_process_environment(),
            },
            ConversationLimits::default(),
            Some("/workspace".into()),
        )
        .unwrap();
        bind_command_store(&service);
        let state = chat_state().with_conversations(Arc::new(service));
        let owner = chat_session(&state, "owner-phone").await;
        let id = "00000000-0000-4000-8000-000000000010";
        assert!(chat_request(
            &state,
            &owner,
            "conversation.create",
            json!({"conversationId": id, "requestId": "create"}),
        )
        .await
        .ok);
        let response = chat_request(
            &state,
            &owner,
            "conversation.read",
            json!({"conversationId": id}),
        )
        .await;
        let runtime = &response.payload.unwrap()["runtime"];
        assert_eq!(runtime["agent"], "claude");
        assert_eq!(runtime["model"], "test");
        assert_eq!(runtime["modelName"], "Test model");
        assert_eq!(runtime["contextWindowTokens"], 100000);
        assert_eq!(runtime["reasoning"], false);
    }

    #[tokio::test]
    async fn listing_answers_each_caller_with_their_own_conversations_only() {
        let (service, provider, _, _) =
            chat_fixture();
        let state = chat_state().with_conversations(Arc::new(service));
        let owner = chat_session(&state, "owner-phone").await;
        let id = "00000000-0000-4000-8000-000000000007";
        assert!(
            chat_request(
                &state,
                &owner,
                "conversation.create",
                json!({"conversationId":id,"requestId":"create"}),
            )
            .await
            .ok
        );
        // Nothing said yet: nothing to list, and a read says there is no
        // title yet — `null`, not left out.
        let listed = chat_request(&state, &owner, "conversation.list", json!({})).await;
        assert_eq!(
            listed.payload.unwrap(),
            json!({"conversations": [], "complete": true})
        );
        let read = chat_request(&state, &owner, "conversation.read", json!({"conversationId":id})).await;
        let view = read.payload.unwrap();
        assert_eq!(view["title"], serde_json::Value::Null);
        assert_eq!(view["approvalMode"], "ask");
        assert_eq!(view["approvalModes"][0]["name"], "Provider asks");
        assert!(
            chat_request(
                &state,
                &owner,
                "conversation.send",
                json!({"conversationId":id,"requestId":"send","executionId":"turn","text":"Plan the trip","attachments":[],"files":[]}),
            )
            .await
            .ok
        );
        let opened = provider.open_calls.load(Ordering::SeqCst);
        // Every surface of the owner sees it, as the wire describes it, with
        // the title a read of it carries too.
        for credential in ["owner-phone", "owner-panel"] {
            let session = chat_session(&state, credential).await;
            let listed = chat_request(&state, &session, "conversation.list", json!({})).await;
            assert!(listed.ok, "{credential}");
            let payload = listed.payload.unwrap();
            assert_eq!(payload["conversations"].as_array().unwrap().len(), 1);
            let row = &payload["conversations"][0];
            assert_eq!(row["conversationId"], id);
            assert_eq!(row["title"], "Plan the trip");
            assert!(row["preview"].is_string());
            assert_eq!(row["archived"], false);
            let read = chat_request(
                &state,
                &session,
                "conversation.read",
                json!({"conversationId":id}),
            )
            .await;
            assert_eq!(read.payload.unwrap()["title"], "Plan the trip");
        }
        // Another principal in the same organization is told of none, and an
        // organization this grant is not for is refused before the service.
        let other = chat_session(&state, "other").await;
        let listed = chat_request(&state, &other, "conversation.list", json!({})).await;
        assert_eq!(
            listed.payload.unwrap(),
            json!({"conversations": [], "complete": true})
        );
        let foreign = chat_session(&state, "foreign").await;
        let refused = chat_request(&state, &foreign, "conversation.list", json!({})).await;
        assert_eq!(refused.error.unwrap().code, "forbidden");
        // Parameters are the generated shape, decoded strictly: nothing else
        // may be named in them, and the filter is a boolean or absent.
        for params in [
            json!({"conversationId":id}),
            json!(null),
            json!({"archived":"yes"}),
        ] {
            let refused = chat_request(&state, &owner, "conversation.list", params).await;
            assert_eq!(refused.error.unwrap().code, "invalid_request");
        }
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), opened);
    }

    #[tokio::test]
    async fn archive_and_delete_answer_as_mutations_and_a_deleted_id_is_refused_by_name() {
        let (service, provider, _, _) =
            chat_fixture();
        let state = chat_state().with_conversations(Arc::new(service));
        let owner = chat_session(&state, "owner-phone").await;
        let id = "00000000-0000-4000-8000-000000000009";
        let target = |request: &str| json!({"conversationId":id,"requestId":request});
        assert!(
            chat_request(&state, &owner, "conversation.create", target("create"))
                .await
                .ok
        );
        // Nothing said yet, so nothing listed, and nothing to archive.
        let reply = chat_request(&state, &owner, "conversation.archive", target("archive-0")).await;
        assert_eq!(
            reply.payload,
            Some(json!({"requestId": "archive-0", "applied": false}))
        );
        assert!(
            chat_request(
                &state,
                &owner,
                "conversation.send",
                json!({"conversationId":id,"requestId":"send","executionId":"turn","text":"hello","attachments":[],"files":[]}),
            )
            .await
            .ok
        );
        for (method, request, applied) in [
            ("conversation.archive", "archive-1", true),
            ("conversation.archive", "archive-1", false),
            ("conversation.unarchive", "unarchive-1", true),
            ("conversation.archive", "archive-2", true),
        ] {
            let reply = chat_request(&state, &owner, method, target(request)).await;
            assert_eq!(
                reply.payload,
                Some(json!({"requestId": request, "applied": applied})),
                "{method} {request}"
            );
        }
        let listed = chat_request(&state, &owner, "conversation.list", json!({})).await;
        assert_eq!(
            listed.payload.unwrap(),
            json!({"conversations": [], "complete": true})
        );
        let listed = chat_request(
            &state,
            &owner,
            "conversation.list",
            json!({"archived": true}),
        )
        .await
        .payload
        .unwrap();
        assert_eq!(listed["conversations"][0]["conversationId"], id);
        assert_eq!(listed["conversations"][0]["archived"], true);

        // Somebody else cannot delete it, and is told nothing about it.
        let other = chat_session(&state, "other").await;
        let refused = chat_request(&state, &other, "conversation.delete", target("delete")).await;
        assert_eq!(refused.error.unwrap().code, "conversation_not_found");
        let deleted = chat_request(&state, &owner, "conversation.delete", target("delete-1")).await;
        assert_eq!(
            deleted.payload,
            Some(json!({"requestId": "delete-1", "applied": true}))
        );
        // Every later command on it, from any of the owner's surfaces, is
        // refused by name, so a surface holding it knows to let it go.
        let panel = chat_session(&state, "owner-panel").await;
        for (method, params) in [
            ("conversation.create", target("create-again")),
            ("conversation.read", json!({"conversationId":id})),
            (
                "conversation.send",
                json!({"conversationId":id,"requestId":"send","executionId":"turn","text":"hi","attachments":[],"files":[]}),
            ),
            ("conversation.close", target("close")),
            ("conversation.archive", target("archive-3")),
        ] {
            let refused = chat_request(&state, &panel, method, params).await;
            assert_eq!(refused.error.unwrap().code, "conversation_deleted", "{method}");
        }
        for archived in [false, true] {
            let listed = chat_request(
                &state,
                &panel,
                "conversation.list",
                json!({"archived": archived}),
            )
            .await;
            assert_eq!(
            listed.payload.unwrap(),
            json!({"conversations": [], "complete": true})
        );
        }
        // The deciding request, repeated, answers as it did; a later one did
        // not delete it.
        for (request, applied) in [("delete-1", true), ("delete-2", false)] {
            let repeated =
                chat_request(&state, &owner, "conversation.delete", target(request)).await;
            assert_eq!(
                repeated.payload,
                Some(json!({"requestId": request, "applied": applied}))
            );
        }
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn conversation_wire_enforces_canonical_and_utf8_byte_limits() {
        let (service, _, _, _) = chat_fixture();
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
            json!({"conversationId":"00000000-0000-4000-8000-00000000000A","requestId":"send","executionId":"execution","text":"hello","attachments":[],"files":[]}),
            json!({"conversationId":id,"requestId":"😀".repeat(65),"executionId":"execution","text":"hello","attachments":[],"files":[]}),
            json!({"conversationId":id,"requestId":"send","executionId":"😀".repeat(65),"text":"hello","attachments":[],"files":[]}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"😀".repeat(2049),"attachments":[],"files":[]}),
            // One contract: the attachment list is always present, and a message is never empty.
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello"}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":" \n","attachments":[],"files":[]}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[{"digest":"sha256:00","mimeType":"image/png","size":1}],"files":[]}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[{"digest":format!("sha256:{}", "0".repeat(64)),"mimeType":"image/svg+xml","size":1}],"files":[]}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[{"digest":format!("sha256:{}", "0".repeat(64)),"mimeType":"image/png","size":0}],"files":[]}),
            // A linked file names an absolute path that can be carried in a
            // link, and nothing else reaches the agent.
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[],"files":[{"path":"report.pdf"}]}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[],"files":[{"path":"/tmp/a\nb.pdf"}]}),
            // Shaped like a path the schema accepts, but naming a directory,
            // which only the gateway's domain rule refuses.
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[],"files":[{"path":"/tmp/"}]}),
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[],"files":[{"path":"/tmp/.."}]}),
            // A name beside the path would be a second thing to disagree with it.
            json!({"conversationId":id,"requestId":"send","executionId":"execution","text":"hello","attachments":[],"files":[{"path":"/tmp/a.pdf","name":"a.pdf"}]}),
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
                json!({"conversationId":id,"requestId":"😀".repeat(64),"executionId":"😀".repeat(64),"text":"😀".repeat(2048),"attachments":[],"files":[]}),
            )
            .await
            .ok
        );
    }

    #[tokio::test]
    async fn disconnect_keeps_admitted_open_after_request_capacity_is_released() {
        let (service, provider, _, _) =
            chat_fixture();
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
        timeout(Duration::from_secs(10), provider.opening.notified())
            .await
            .expect("provider open follows the durable creation attempt");
        // Readiness waits for the original attachment, so this response is
        // still outstanding. The socket stays responsive, and the request
        // permit stays with the creation task.
        send_command(&peer, "health", "server.health", json!({}));
        assert_eq!(response(&mut peer).await["id"], "health");
        assert_eq!(state.requests.available_permits(), 127);
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
        drop(peer.input);
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.requests.available_permits(),
            127,
            "dropping the socket leaves the original creation task holding its permit"
        );
        release.send(()).unwrap();
        timeout(Duration::from_secs(1), async {
            while state.requests.available_permits() != 128 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn same_socket_stalled_response_does_not_hold_close_effect() {
        let (service, provider, _, _) =
            chat_fixture();
        let state = chat_state().with_conversations(Arc::new(service));
        let session = chat_session(&state, "owner-phone").await;
        let id = "00000000-0000-4000-8000-000000000009";
        assert!(chat_request(
            &state,
            &session,
            "conversation.create",
            json!({"conversationId":id,"requestId":"create"}),
        )
        .await
        .ok);
        timeout(Duration::from_secs(1), async {
            loop {
                let read = chat_request(
                    &state,
                    &session,
                    "conversation.read",
                    json!({"conversationId":id}),
                )
                .await;
                if read.payload.as_ref().is_some_and(|value| value["lifecycle"]["phase"] == "attached") {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (release, gate) = oneshot::channel();
        let (socket, mut peer) = test_socket(Some(gate));
        let task = tokio::spawn(run_authenticated(socket, state, session));
        peer.request("slow");
        timeout(Duration::from_secs(1), peer.writing.recv())
            .await
            .unwrap()
            .unwrap();
        send_command(
            &peer,
            "stop",
            "conversation.close",
            json!({"conversationId":id,"requestId":"stop"}),
        );
        timeout(Duration::from_secs(1), async {
            while provider.close_calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("close effect must run while the first response is blocked");
        assert!(peer.output.try_recv().is_err());
        release.send(()).unwrap();
        assert_eq!(response(&mut peer).await["id"], "slow");
        let stop = response(&mut peer).await;
        assert_eq!(stop["id"], "stop");
        assert_eq!(stop["ok"], true);
        drop(peer.input);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn reads_during_startup_return_without_consuming_control_capacity() {
        let (service, provider, _, _) =
            chat_fixture();
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
        timeout(Duration::from_secs(10), provider.opening.notified())
            .await
            .expect("provider open follows the durable creation attempt");
        assert_eq!(state.requests.available_permits(), 127);
        for n in 0..15 {
            send_command(
                &peer,
                &format!("read-{n}"),
                "conversation.read",
                json!({"conversationId":id}),
            );
        }
        for n in 0..15 {
            let read = response(&mut peer).await;
            assert_eq!(read["id"], format!("read-{n}"));
            assert_eq!(read["ok"], true);
        }
        assert_eq!(state.requests.available_permits(), 127);
        send_command(&peer, "full", "server.health", json!({}));
        assert_eq!(response(&mut peer).await["id"], "full");
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
        assert_eq!(state.requests.available_permits(), 127);
        release.send(()).unwrap();
        let created = response(&mut peer).await;
        assert_eq!(created["id"], "create");
        assert_eq!(created["ok"], true);
        drop(peer.input);
        task.await.unwrap();
        assert_eq!(state.requests.available_permits(), 128);
    }
    #[tokio::test]
    async fn authenticated_reorder_changes_provider_dispatch_order_atomically() {
        let (service, provider, _, _) =
            chat_fixture();
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
                json!({"conversationId":id,"requestId":execution,"executionId":execution,"text":execution,"attachments":[],"files":[]}),
            );
            assert_eq!(response(&mut peer).await["ok"], true);
            if execution == "running" {
                timeout(
                    Duration::from_secs(10),
                    provider.execution_started.notified(),
                )
                .await
                .expect("the running turn starts after its receipt");
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

    #[tokio::test]
    async fn stop_withdraws_the_named_queued_turn_without_closing() {
        let (service, provider, _, _) = chat_fixture();
        let state = chat_state().with_conversations(Arc::new(service));
        let session = chat_session(&state, "owner-phone").await;
        let id = "00000000-0000-4000-8000-000000000011";
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
        let (release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        assert!(
            chat_request(
                &state,
                &session,
                "conversation.send",
                json!({"conversationId":id,"requestId":"running","executionId":"running","text":"running","attachments":[],"files":[]}),
            )
            .await
            .ok
        );
        let looked_up = chat_request(
            &state,
            &session,
            "conversation.receipt",
            json!({"conversationId":id,"requestId":"running","operation":"submit","executionId":"running","text":"running"}),
        )
        .await;
        assert!(looked_up.ok, "{looked_up:?}");
        assert_eq!(looked_up.payload.as_ref().unwrap()["found"], true);
        timeout(
            Duration::from_secs(1),
            provider.execution_started.notified(),
        )
        .await
        .unwrap();
        assert!(
            chat_request(
                &state,
                &session,
                "conversation.send",
                json!({"conversationId":id,"requestId":"queued","executionId":"queued","text":"queued","attachments":[],"files":[]}),
            )
            .await
            .ok
        );
        let absent = chat_request(
            &state,
            &session,
            "conversation.receipt",
            json!({"conversationId":id,"requestId":"stop-queued","operation":"stop","executionId":"queued"}),
        )
        .await;
        assert!(absent.ok, "{absent:?}");
        assert_eq!(
            absent.payload.unwrap(),
            json!({"found": false, "requestId": "stop-queued"})
        );
        let stopped = chat_request(
            &state,
            &session,
            "conversation.stop",
            json!({"conversationId":id,"requestId":"stop-queued","executionId":"queued"}),
        )
        .await;
        assert!(stopped.ok, "{stopped:?}");
        let withdrawn = json!({"requestId":"stop-queued","stage":"settled","outcome":"withdrawn"});
        assert_eq!(stopped.payload.unwrap(), withdrawn);
        let again = chat_request(
            &state,
            &session,
            "conversation.stop",
            json!({"conversationId":id,"requestId":"stop-queued","executionId":"queued"}),
        )
        .await;
        assert_eq!(again.payload.unwrap(), withdrawn);
        let found = chat_request(
            &state,
            &session,
            "conversation.receipt",
            json!({"conversationId":id,"requestId":"stop-queued","operation":"stop","executionId":"queued"}),
        )
        .await;
        assert_eq!(
            found.payload.unwrap(),
            json!({"found": true, "requestId": "stop-queued", "stage": "settled", "outcome": "withdrawn"})
        );
        assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
        let read = chat_request(
            &state,
            &session,
            "conversation.read",
            json!({"conversationId":id}),
        )
        .await;
        let pending = read.payload.unwrap()["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["executionId"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(pending.is_empty(), "{pending:?}");
        assert_eq!(*provider.executions.lock().unwrap(), ["running"]);
        release.send(()).unwrap();
    }
}
