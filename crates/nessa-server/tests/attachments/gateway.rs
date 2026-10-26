/// `attachment.begin` through the authenticated socket, and the upload route
/// through the real router.
mod attachment_gateway {
    use super::gateway::{chat_request, chat_session, chat_state, response, send_command};
    use super::*;
    use crate::attachments::application::{AttachmentAuditRecord, AttachmentLimits};
    use crate::attachments::domain::TicketLimits;
    use crate::attachments_test_support::{
        digest_of, ChannelBody, Fixture, StubNormalizer, CONVERSATION,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const BYTES: &[u8] = b"twenty bytes of file";

    fn begin(bytes: &[u8], media_type: &str) -> serde_json::Value {
        json!({
            "conversationId": CONVERSATION,
            "requestId": "begin-1",
            "digest": digest_of(bytes).to_string(),
            "mimeType": media_type,
            "size": bytes.len(),
        })
    }
    /// The chat fixture's owner owns the conversation uploads go into.
    fn attachment_state(fixture: &Fixture) -> ProductRouteState {
        fixture
            .ownership
            .give(CONVERSATION, "organization", "owner");
        chat_state().with_attachments(fixture.service.clone(), fixture.reads())
    }

    #[tokio::test]
    async fn beginning_an_upload_requires_the_chat_grant_and_a_gateway_that_runs_conversations() {
        let state = chat_state();
        for (credential, expected) in [
            ("reader", "forbidden"),
            ("foreign", "forbidden"),
            // Not `agent_not_configured`. The attachment store is composed only
            // alongside conversations, so its absence says this gateway runs
            // none — never that one agent is missing from `agents.runtimes`,
            // which is what that other code now means.
            ("owner-phone", "conversations_not_configured"),
        ] {
            let session = chat_session(&state, credential).await;
            let reply =
                chat_request(&state, &session, "attachment.begin", begin(BYTES, "text/plain"))
                    .await;
            assert!(!reply.ok);
            assert!(reply.payload.is_none());
            assert_eq!(reply.error.unwrap().code, expected, "{credential}");
        }
        // The grant is checked before the method is, so nothing under this
        // prefix is reachable without it.
        let session = chat_session(&state, "owner-phone").await;
        let reply = chat_request(&state, &session, "attachment.unknown", json!({})).await;
        assert_eq!(reply.error.unwrap().code, "unknown_method");
    }

    #[tokio::test]
    async fn the_socket_answers_with_a_ticket_and_then_with_the_stored_reference() {
        let fixture = Fixture::with_normalizer(
            AttachmentLimits::default(),
            StubNormalizer::producing(b"small jpeg", "image/jpeg"),
        );
        let state = attachment_state(&fixture);
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));

        send_command(&peer, "first", "attachment.begin", begin(BYTES, "image/png"));
        let reply = response(&mut peer).await;
        assert_eq!(reply["id"], "first");
        assert_eq!(reply["ok"], true, "{reply}");
        let payload = reply["payload"].as_object().unwrap();
        // One shape for both answers: every field present, the unused ones null.
        let mut fields: Vec<_> = payload.keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            ["digest", "expiresAtMs", "mimeType", "requestId", "size", "state", "ticket"]
        );
        let schema: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/product/v1.json"
        )))
        .unwrap();
        for required in schema["$defs"]["AttachmentBeginResult"]["required"]
            .as_array()
            .unwrap()
        {
            assert!(payload.contains_key(required.as_str().unwrap()), "{required}");
        }
        assert_eq!(payload["requestId"], "begin-1");
        assert_eq!(payload["state"], "upload_required");
        let ticket = payload["ticket"].as_str().unwrap().to_owned();
        assert!(ticket.len() == 64 && ticket.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(payload["expiresAtMs"].is_u64());
        for absent in ["digest", "mimeType", "size"] {
            assert!(payload[absent].is_null(), "{absent}");
        }

        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 8))
            .await
            .unwrap();
        send_command(&peer, "second", "attachment.begin", begin(BYTES, "image/png"));
        let reply = response(&mut peer).await;
        let payload = reply["payload"].as_object().unwrap();
        assert_eq!(payload.len(), 7);
        assert_eq!(payload["state"], "stored");
        assert!(payload["ticket"].is_null());
        assert!(payload["expiresAtMs"].is_null());
        // What to refer to is the normalized image, not the one described.
        assert_eq!(payload["digest"], digest_of(b"small jpeg").to_string());
        assert_eq!(payload["mimeType"], "image/jpeg");
        assert_eq!(payload["size"], 10);

        drop(peer.input);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn the_socket_names_each_refusal_and_attributes_the_ticket_to_the_verified_caller() {
        let fixture = Fixture::new(AttachmentLimits {
            tickets: TicketLimits {
                total: 1,
                per_organization: 1,
                per_conversation: 1,
            },
            ..AttachmentLimits::default()
        });
        let state = attachment_state(&fixture);
        let owner = chat_session(&state, "owner-phone").await;
        let mut unknown_field = begin(BYTES, "text/plain");
        unknown_field["principalId"] = json!("victim");
        let mut uppercase = begin(BYTES, "text/plain");
        uppercase["conversationId"] = json!(CONVERSATION.to_uppercase());
        let mut oversized = begin(BYTES, "text/plain");
        oversized["size"] = json!(64 * 1024 * 1024 + 1);
        for params in [
            json!({}),
            unknown_field,
            uppercase,
            oversized,
            begin(BYTES, "Text/Plain"),
        ] {
            let reply = chat_request(&state, &owner, "attachment.begin", params).await;
            assert_eq!(reply.error.unwrap().code, "invalid_request");
        }
        // Same organization, another principal: the conversation is not theirs.
        let other = chat_session(&state, "other").await;
        let reply =
            chat_request(&state, &other, "attachment.begin", begin(BYTES, "text/plain")).await;
        assert_eq!(reply.error.unwrap().code, "conversation_not_found");

        let reply =
            chat_request(&state, &owner, "attachment.begin", begin(BYTES, "text/plain")).await;
        let ticket = reply.payload.unwrap()["ticket"].as_str().unwrap().to_owned();
        let reply =
            chat_request(&state, &owner, "attachment.begin", begin(b"more", "text/plain")).await;
        assert_eq!(reply.error.unwrap().code, "attachment_capacity");

        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 8))
            .await
            .unwrap();
        let records = fixture.audit.taken();
        let [AttachmentAuditRecord::HoldCreated { hold, .. }] = records.as_slice() else {
            panic!("one hold expected, got {records:?}")
        };
        // Session identity, not anything the request said about itself.
        assert_eq!(hold.organization_id().as_str(), "organization");
        assert_eq!(hold.uploaded_by().principal_id().as_str(), "owner");
        assert_eq!(hold.uploaded_by().surface_id(), "owner-phone");
        assert_eq!(hold.uploaded_by().action_id(), "begin-1");

        fixture
            .ownership
            .unavailable
            .store(true, Ordering::SeqCst);
        let reply =
            chat_request(&state, &owner, "attachment.begin", begin(b"more", "text/plain")).await;
        assert_eq!(reply.error.unwrap().code, "temporarily_unavailable");
    }

    #[tokio::test]
    async fn beginning_an_upload_does_not_wait_behind_ordinary_requests() {
        let fixture = Fixture::new(AttachmentLimits::default());
        let state = attachment_state(&fixture);
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
        send_command(&peer, "begin", "attachment.begin", begin(BYTES, "text/plain"));
        let reply = response(&mut peer).await;
        assert_eq!(reply["id"], "begin");
        assert_eq!(reply["payload"]["state"], "upload_required");
        drop(peer.input);
        task.await.unwrap();
        drop(held);
    }

    #[tokio::test]
    async fn beginning_uploads_and_controls_never_take_each_others_place() {
        let fixture = Fixture::new(AttachmentLimits::default());
        let state = attachment_state(&fixture);
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
        let close = || json!({"conversationId": CONVERSATION, "requestId": "close-1"});

        // Every begin slot taken, as a stalled audit sink would leave them: a
        // begin is turned away, and a close is still admitted and answered by
        // its own service rather than by capacity.
        let begins = state.upload_begins.clone().acquire_many_owned(16).await;
        send_command(&peer, "begin", "attachment.begin", begin(BYTES, "text/plain"));
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        assert_eq!(state.controls.available_permits(), 32);
        send_command(&peer, "close", "conversation.close", close());
        // This fixture runs no conversations at all, which is its own answer
        // rather than "no runtime is configured for that agent": what matters
        // here is only that the close was answered by the conversation side and
        // not turned away by the begin route's capacity.
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "conversations_not_configured"
        );
        drop(begins);

        // And the other way about: with every control slot taken, a begin is
        // still admitted.
        let controls = state.controls.clone().acquire_many_owned(32).await;
        send_command(&peer, "close-2", "conversation.close", close());
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        send_command(&peer, "begin-2", "attachment.begin", begin(BYTES, "text/plain"));
        assert_eq!(response(&mut peer).await["payload"]["state"], "upload_required");
        drop(controls);
        drop(peer.input);
        task.await.unwrap();
    }

    /// One HTTP/1.1 exchange over a real connection to the real router.
    async fn exchange(address: std::net::SocketAddr, head: String, body: &[u8]) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        String::from_utf8_lossy(&reply).into_owned()
    }

    #[tokio::test]
    async fn the_router_serves_uploads_past_the_limit_every_other_route_keeps() {
        let large = vec![7_u8; 256 * 1024];
        let fixture = Fixture::new(AttachmentLimits::default());
        let state = attachment_state(&fixture);
        let session = chat_session(&state, "owner-phone").await;
        let reply =
            chat_request(&state, &session, "attachment.begin", begin(&large, "text/plain")).await;
        let ticket = reply.payload.unwrap()["ticket"].as_str().unwrap().to_owned();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, crate::server::entrypoint::http::router(state))
                .await
                .unwrap();
        });

        let preflight = exchange(
            address,
            "OPTIONS /attachments HTTP/1.1\r\nHost: gateway\r\nOrigin: tauri://localhost\r\n\
             Access-Control-Request-Method: PUT\r\nConnection: close\r\n\r\n"
                .into(),
            b"",
        )
        .await;
        assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
        assert!(preflight.contains("access-control-allow-origin: tauri://localhost"));

        // 256 KiB into a router whose other routes stop at 20 KiB.
        let uploaded = exchange(
            address,
            format!(
                "PUT /attachments HTTP/1.1\r\nHost: gateway\r\nOrigin: tauri://localhost\r\n\
                 x-nessa-upload-ticket: {ticket}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                large.len()
            ),
            &large,
        )
        .await;
        assert!(uploaded.starts_with("HTTP/1.1 200"), "{uploaded}");
        assert!(uploaded.contains(&digest_of(&large).to_string()));
        assert_eq!(fixture.store.blob(digest_of(&large)).unwrap(), large);

        // Small enough to be sent whole before the refusal closes the connection.
        let over_the_limit = &large[..24 * 1024];
        let limited = exchange(
            address,
            format!(
                "POST /browser/login HTTP/1.1\r\nHost: gateway\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                over_the_limit.len()
            ),
            over_the_limit,
        )
        .await;
        assert!(limited.starts_with("HTTP/1.1 413"), "{limited}");
        server.abort();
    }

    fn read(stored: &[u8], media_type: &str, offset: usize, max_bytes: usize) -> serde_json::Value {
        json!({
            "conversationId": CONVERSATION,
            "digest": digest_of(stored).to_string(),
            "mimeType": media_type,
            "size": stored.len(),
            "offset": offset,
            "maxBytes": max_bytes,
        })
    }

    /// The chat fixture's owner owns the conversation, which holds `bytes`.
    fn holding(fixture: &Fixture, bytes: &[u8], media_type: &str) -> ProductRouteState {
        fixture.held.hold("organization", CONVERSATION, bytes, media_type);
        attachment_state(fixture)
    }

    #[tokio::test]
    async fn a_reader_reads_a_held_file_back_range_by_range() {
        let fixture = Fixture::new(AttachmentLimits::default());
        let bytes: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let state = holding(&fixture, &bytes, "image/png");
        let device = chat_session(&state, "owner-device").await;
        let mut assembled = Vec::new();
        while assembled.len() < bytes.len() {
            let reply = chat_request(
                &state,
                &device,
                "artifact.read",
                read(&bytes, "image/png", assembled.len(), 32 * 1024),
            )
            .await;
            assert!(reply.ok, "{:?}", reply.error);
            let payload = reply.payload.unwrap();
            let mut fields: Vec<_> = payload.as_object().unwrap().keys().cloned().collect();
            fields.sort_unstable();
            assert_eq!(fields, ["data", "offset"]);
            assert_eq!(payload["offset"], assembled.len());
            let data = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                payload["data"].as_str().unwrap(),
            )
            .unwrap();
            assert!(data.len() <= 32 * 1024);
            assembled.extend(data);
        }
        assert_eq!(assembled, bytes);
    }

    #[tokio::test]
    async fn a_whole_range_answer_fits_the_ordinary_frame() {
        let fixture = Fixture::new(AttachmentLimits::default());
        let bytes = vec![0xff; 32 * 1024];
        let state = holding(&fixture, &bytes, "application/octet-stream");
        let device = chat_session(&state, "owner-device").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state.clone(), device));
        send_command(
            &peer,
            "whole",
            "artifact.read",
            read(&bytes, "application/octet-stream", 0, 32 * 1024),
        );
        let reply = response(&mut peer).await;
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["payload"]["data"].as_str().unwrap().len(), 43_692);
        drop(peer.input);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn reading_back_needs_the_read_grant_and_the_conversation_and_names_each_refusal() {
        let fixture = Fixture::new(AttachmentLimits::default());
        let state = holding(&fixture, BYTES, "text/plain");
        // A credential without the read grant is refused by what it holds.
        for credential in ["owner-phone", "reader", "foreign"] {
            let session = chat_session(&state, credential).await;
            let reply =
                chat_request(&state, &session, "artifact.read", read(BYTES, "text/plain", 0, 4))
                    .await;
            assert_eq!(reply.error.unwrap().code, "forbidden", "{credential}");
        }
        let device = chat_session(&state, "owner-device").await;
        let reply =
            chat_request(&state, &device, "artifact.read", read(BYTES, "text/plain", 0, 4)).await;
        assert!(reply.ok, "{:?}", reply.error);
        // Not this principal's conversation: as if it were not there.
        let other = chat_session(&state, "other-device").await;
        let reply =
            chat_request(&state, &other, "artifact.read", read(BYTES, "text/plain", 0, 4)).await;
        assert_eq!(reply.error.unwrap().code, "conversation_not_found");

        let mut unknown_field = read(BYTES, "text/plain", 0, 4);
        unknown_field["principalId"] = json!("victim");
        for (params, code) in [
            (json!({}), "invalid_request"),
            (unknown_field, "invalid_request"),
            (read(BYTES, "Text/Plain", 0, 4), "invalid_request"),
            (read(BYTES, "text/plain", 0, 0), "invalid_request"),
            (read(BYTES, "text/plain", 0, 32 * 1024 + 1), "invalid_request"),
            (read(BYTES, "text/plain", BYTES.len(), 1), "invalid_request"),
            // The same bytes held as another type are another file.
            (read(BYTES, "image/png", 0, 4), "attachment_not_found"),
            (read(b"never held", "text/plain", 0, 4), "attachment_not_found"),
        ] {
            let reply = chat_request(&state, &device, "artifact.read", params.clone()).await;
            assert_eq!(reply.error.unwrap().code, code, "{params}");
        }
        fixture.held.release_all();
        let reply =
            chat_request(&state, &device, "artifact.read", read(BYTES, "text/plain", 4, 4)).await;
        assert_eq!(reply.error.unwrap().code, "attachment_not_found");

        fixture
            .ownership
            .deleted
            .lock()
            .unwrap()
            .push(crate::attachments_test_support::conversation(CONVERSATION));
        let reply =
            chat_request(&state, &device, "artifact.read", read(BYTES, "text/plain", 0, 4)).await;
        assert_eq!(reply.error.unwrap().code, "conversation_deleted");
        fixture
            .ownership
            .unavailable
            .store(true, Ordering::SeqCst);
        let reply =
            chat_request(&state, &device, "artifact.read", read(BYTES, "text/plain", 0, 4)).await;
        assert_eq!(reply.error.unwrap().code, "temporarily_unavailable");
    }

    #[tokio::test]
    async fn a_gateway_that_runs_no_conversations_reads_nothing_back() {
        let state = chat_state();
        let device = chat_session(&state, "owner-device").await;
        let reply =
            chat_request(&state, &device, "artifact.read", read(BYTES, "text/plain", 0, 4)).await;
        assert_eq!(
            reply.error.unwrap().code,
            "conversations_not_configured"
        );
    }

    #[test]
    fn the_schema_bounds_a_range_where_the_reads_do() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/product/v1.json"
        )))
        .unwrap();
        let max = crate::attachments::application::HeldReads::MAX_RANGE_BYTES;
        let defs = &schema["$defs"];
        assert_eq!(
            defs["ArtifactReadParams"]["properties"]["maxBytes"]["maximum"],
            max
        );
        assert_eq!(
            defs["ArtifactReadResult"]["properties"]["data"]["maxLength"],
            max.div_ceil(3) * 4
        );
    }

    fn piece(ticket: &str, offset: usize, bytes: &[u8], last: bool) -> serde_json::Value {
        use base64::Engine;
        json!({
            "ticket": ticket,
            "offset": offset,
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            "last": last,
        })
    }
    async fn ticket_for(state: &ProductRouteState, session: &AuthenticatedSession) -> String {
        let reply =
            chat_request(state, session, "attachment.begin", begin(BYTES, "image/png")).await;
        assert!(reply.ok, "{:?}", reply.error.map(|e| e.code));
        reply.payload.unwrap()["ticket"].as_str().unwrap().to_owned()
    }
    fn refused(reply: ResponseFrame) -> String {
        assert!(!reply.ok);
        reply.error.unwrap().code
    }

    #[tokio::test]
    async fn pieces_over_the_session_are_kept_and_answered_with_the_stored_file() {
        let fixture = Fixture::with_normalizer(
            AttachmentLimits::default(),
            StubNormalizer::producing(b"small jpeg", "image/jpeg"),
        );
        let state = attachment_state(&fixture);
        let session = chat_session(&state, "owner-phone").await;
        let ticket = ticket_for(&state, &session).await;

        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&ticket, 0, &BYTES[..8], false),
        )
        .await;
        assert!(reply.ok, "{:?}", reply.error.map(|e| e.code));
        let payload = reply.payload.unwrap();
        assert_eq!(payload["received"], 8);
        for absent in ["digest", "mimeType", "size"] {
            assert!(payload[absent].is_null(), "{absent}");
        }
        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&ticket, 8, &BYTES[8..], true),
        )
        .await;
        assert!(reply.ok, "{:?}", reply.error.map(|e| e.code));
        let payload = reply.payload.unwrap();
        assert_eq!(payload["received"], BYTES.len());
        // What to refer to is the normalized image, as on the HTTP route.
        assert_eq!(payload["digest"], digest_of(b"small jpeg").to_string());
        assert_eq!(payload["mimeType"], "image/jpeg");
        assert_eq!(payload["size"], 10);
        assert_eq!(state.socket_uploads.in_progress(), 0);

        // The ticket was spent by its upload.
        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&ticket, 0, BYTES, true),
        )
        .await;
        assert_eq!(refused(reply), "upload_ticket_invalid");
        // And the conversation holds the file, so beginning again needs no upload.
        let reply =
            chat_request(&state, &session, "attachment.begin", begin(BYTES, "image/png")).await;
        assert_eq!(reply.payload.unwrap()["state"], "stored");
    }

    #[tokio::test]
    async fn a_piece_out_of_order_ends_its_upload() {
        let fixture = Fixture::with_normalizer(
            AttachmentLimits::default(),
            StubNormalizer::producing(b"small jpeg", "image/jpeg"),
        );
        let state = attachment_state(&fixture);
        let session = chat_session(&state, "owner-phone").await;
        let ticket = ticket_for(&state, &session).await;
        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&ticket, 0, &BYTES[..8], false),
        )
        .await;
        assert!(reply.ok);
        // A gap: the upload ends here.
        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&ticket, 12, &BYTES[12..], true),
        )
        .await;
        assert_eq!(refused(reply), "upload_failed");
        assert_eq!(state.socket_uploads.in_progress(), 0);
        // The piece that would have been right is now one of no upload.
        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&ticket, 8, &BYTES[8..], true),
        )
        .await;
        assert_eq!(refused(reply), "upload_failed");
        // A later piece with no upload under its ticket starts nothing.
        let other = ticket_for(&state, &session).await;
        let reply = chat_request(
            &state,
            &session,
            "attachment.write",
            piece(&other, 8, &BYTES[8..], true),
        )
        .await;
        assert_eq!(refused(reply), "upload_failed");
        assert_eq!(state.socket_uploads.in_progress(), 0);
    }

    #[tokio::test]
    async fn a_piece_that_cannot_be_read_or_a_ticket_never_issued_is_refused() {
        let fixture = Fixture::new(AttachmentLimits::default());
        let state = attachment_state(&fixture);
        let session = chat_session(&state, "owner-phone").await;
        let unknown = "ab".repeat(32);
        let mut not_base64 = piece(&unknown, 0, BYTES, true);
        not_base64["data"] = json!("not base64!");
        let mut extra = piece(&unknown, 0, BYTES, true);
        extra["conversationId"] = json!(CONVERSATION);
        for (params, expected) in [
            (not_base64, "invalid_request"),
            (extra, "invalid_request"),
            // Only a last piece may be empty.
            (piece(&unknown, 0, b"", false), "invalid_request"),
            (piece(&unknown, 0, &vec![0; 32 * 1024 + 1], false), "invalid_request"),
            (piece("short", 0, BYTES, true), "upload_ticket_invalid"),
            (piece(&unknown, 0, BYTES, true), "upload_ticket_invalid"),
        ] {
            let reply = chat_request(&state, &session, "attachment.write", params).await;
            assert_eq!(refused(reply), expected);
        }
        assert_eq!(state.socket_uploads.in_progress(), 0);
        // Without the chat grant the method is not reachable at all.
        let reader = chat_session(&state, "reader").await;
        let reply =
            chat_request(&state, &reader, "attachment.write", piece(&unknown, 0, BYTES, true))
                .await;
        assert_eq!(refused(reply), "forbidden");
    }
}
