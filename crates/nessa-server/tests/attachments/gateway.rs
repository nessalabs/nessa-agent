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
        chat_state().with_attachments(fixture.service.clone())
    }

    #[tokio::test]
    async fn beginning_an_upload_requires_the_chat_grant_and_a_configured_agent() {
        let state = chat_state();
        for (credential, expected) in [
            ("reader", "forbidden"),
            ("foreign", "forbidden"),
            ("owner-phone", "agent_not_configured"),
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
}
