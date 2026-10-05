/// `GET /mcp-resources` through the real router, over a real connection.
mod mcp_resource_gateway {
    use super::*;
    use crate::mcp_servers::infrastructure::ticket_test_support::{app, held, issued, Fixture, CONVERSATION};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PAGE: &[u8] = b"<!doctype html><title>chart</title>";

    /// One HTTP/1.1 exchange over a real connection to the real router.
    async fn exchange(address: std::net::SocketAddr, head: String) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        String::from_utf8_lossy(&reply).into_owned()
    }

    /// Everything traced while it is the default, whatever its level or
    /// source: this crate's, axum's, hyper's.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }

    #[tokio::test]
    async fn a_redeemed_ticket_appears_in_no_log_line_or_trace() {
        let captured = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        // A current-thread runtime: the server's tasks run on this thread,
        // under this default.
        let _traced = tracing::subscriber::set_default(subscriber);
        tracing::info!("capture is live");

        let fixture = Fixture::new();
        let (state, _) = fixture_state();
        let state = state.with_resource_tickets(fixture.store.clone(), fixture.audit.clone());
        let ticket = issued(&fixture.store, held(CONVERSATION, app("call-1", "mount-1"), PAGE))
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, crate::server::entrypoint::http::router(state))
                .await
                .unwrap();
        });

        let preflight = exchange(
            address,
            "OPTIONS /mcp-resources HTTP/1.1\r\nHost: gateway\r\nOrigin: tauri://localhost\r\n\
             Access-Control-Request-Method: GET\r\n\
             Access-Control-Request-Headers: x-nessa-resource-ticket\r\nConnection: close\r\n\r\n"
                .into(),
        )
        .await;
        assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
        assert!(preflight.contains("access-control-allow-headers: x-nessa-resource-ticket"));

        let request = format!(
            "GET /mcp-resources HTTP/1.1\r\nHost: gateway\r\nOrigin: tauri://localhost\r\n\
             x-nessa-resource-ticket: {ticket}\r\nConnection: close\r\n\r\n"
        );
        let served = exchange(address, request.clone()).await;
        assert!(served.starts_with("HTTP/1.1 200"), "{served}");
        assert!(served.contains("content-type: text/html;profile=mcp-app"));
        assert!(served.ends_with(std::str::from_utf8(PAGE).unwrap()), "{served}");
        let spent = exchange(address, request).await;
        assert!(spent.starts_with("HTTP/1.1 404"), "{spent}");
        assert!(spent.contains("content-length: 0"), "{spent}");
        // A ticket in the URL is not one: it is not read from there.
        let fresh = issued(&fixture.store, held(CONVERSATION, app("call-1", "mount-1"), PAGE))
            .unwrap();
        let in_url = exchange(
            address,
            format!(
                "GET /mcp-resources?ticket={fresh} HTTP/1.1\r\nHost: gateway\r\n\
                 Connection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(in_url.starts_with("HTTP/1.1 404"), "{in_url}");
        server.abort();

        let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("capture is live"), "{logged}");
        assert!(!logged.contains(&ticket), "{logged}");
        assert!(!logged.contains(&fresh), "{logged}");
    }

    /// The product route's dependencies, with nothing composed but auth.
    fn fixture_state() -> (ProductRouteState, Arc<Authority>) {
        fixture(MembershipRole::Member)
    }
}

/// An MCP App's calls on the socket (#348): their own lane, which held calls
/// fill without ever keeping out the read and the answer that would end them,
/// and a bound that counts each call until its own task ends.
mod mcp_app_lane {
    use super::gateway::{chat_session, chat_state, response, send_command};
    use super::*;
    use crate::app_call_test_support::{Fixture, INSTANCE, SERVER};
    use crate::conversation::application::{ConversationCaller, MAX_APP_CALLS};
    use nessa_protocol::product::generated::MAX_MCP_CONTEXT_BYTES;
    use nessa_protocol::product_contract::generated::{ConversationErrorCode, MAX_MCP_MESSAGE_BYTES};
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use std::collections::HashMap;

    /// The conversation of `owner-phone`'s principal, as the socket names it.
    async fn owners_fixture() -> Fixture {
        Fixture::for_owner(ConversationCaller {
            organization_id: OrganizationId::new("organization").unwrap(),
            principal_id: PrincipalId::new("owner").unwrap(),
            surface_id: "owner-phone".into(),
            action_id: "fixture".into(),
        })
        .await
    }

    fn call(fixture: &Fixture, request: &str, tool: &str) -> serde_json::Value {
        call_on(fixture, request, tool, INSTANCE)
    }

    fn call_on(fixture: &Fixture, request: &str, tool: &str, instance: &str) -> serde_json::Value {
        json!({
            "conversationId": fixture.id.to_string(),
            "requestId": request,
            "app": {
                "executionId": fixture.execution_id,
                "toolId": fixture.tool_id,
                "instanceId": instance,
            },
            "server": SERVER,
            "tool": tool,
        })
    }

    /// One mount's lowercase instance id, distinct for `n`.
    fn mount_id(n: usize) -> String {
        format!("{n:08x}-0000-4000-8000-{n:012x}")
    }

    /// The next `count` responses, by request id.
    async fn responses(peer: &mut TestPeer, count: usize) -> HashMap<String, serde_json::Value> {
        let mut answered = HashMap::new();
        for _ in 0..count {
            let reply = response(peer).await;
            answered.insert(reply["id"].as_str().unwrap().to_owned(), reply);
        }
        answered
    }

    async fn until(mut done: impl AsyncFnMut() -> bool) {
        timeout(Duration::from_secs(5), async {
            while !done().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn what_the_wire_cannot_carry_is_refused_or_left_out_on_the_way() {
        let fixture = owners_fixture().await;
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));

        // A URI past the schema's bound is refused before the service: its
        // refusal is not even on record, as no record could hold it.
        let mut long = call(&fixture, "long", "read_rows");
        long.as_object_mut().unwrap().remove("tool");
        long.as_object_mut().unwrap().remove("argumentsJson");
        long["uri"] = json!(format!("ui://charts/{}", "x".repeat(2048)));
        send_command(&peer, "long", "mcp.readResource", long);
        assert_eq!(response(&mut peer).await["error"]["code"], "invalid_request");
        assert!(fixture.audit.phases().is_empty());

        // A JSON-RPC code past what a JSON number keeps: the code, without
        // details the schema could not carry.
        fixture.apps.answers.lock().unwrap().push(Err(
            crate::conversation::application::McpAppFailure::Remote {
                code: i64::MAX,
                message: "bad".into(),
            },
        ));
        send_command(&peer, "far", "mcp.callTool", call(&fixture, "far", "read_rows"));
        let refused = response(&mut peer).await;
        assert_eq!(refused["error"]["code"], "mcp_remote_error");
        assert!(refused["error"].get("details").is_none());
        drop(peer.input);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn held_calls_fill_their_lane_and_never_keep_out_the_read_and_answer() {
        let fixture = owners_fixture().await;
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));

        // Four destructive calls, each from its own mount: the lane is full.
        // One mount may hold only three (`one_mount_cannot_fill_the_app_lane`).
        let mut opened = Vec::<nessa_protocol::conversation::view::ConversationPermission>::new();
        for (n, request) in ["c1", "c2", "c3", "c4"].into_iter().enumerate() {
            send_command(
                &peer,
                request,
                "mcp.callTool",
                call_on(&fixture, request, "delete_rows", &mount_id(n + 1)),
            );
            let count = n + 1;
            until(async || fixture.app_reviews().await.len() == count).await;
            let review = fixture
                .app_reviews()
                .await
                .into_iter()
                .find(|review| {
                    !opened
                        .iter()
                        .any(|opened| opened.permission_id == review.permission_id)
                })
                .expect("the call opened its review");
            opened.push(review);
        }
        send_command(
            &peer,
            "c5",
            "mcp.callTool",
            call_on(&fixture, "c5", "delete_rows", &mount_id(5)),
        );
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "c5");
        assert_eq!(refused["error"]["code"], "temporarily_unavailable");

        // The read still shows every review, as the app's.
        let read = json!({"conversationId": fixture.id.to_string()});
        send_command(&peer, "read", "conversation.read", read);
        let view = response(&mut peer).await;
        let permissions = view["payload"]["permissions"].as_array().unwrap();
        assert_eq!(permissions.len(), 4);
        assert!(permissions
            .iter()
            .all(|review| review["origin"]["kind"] == "app" && review["origin"]["server"] == SERVER));

        // The answer still reaches the first mount's review, and lets its call go.
        let review = &opened[0];
        send_command(
            &peer,
            "answer",
            "conversation.answer",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "answer",
                "executionId": review.execution_id,
                "permissionId": review.permission_id,
                "optionId": "allow",
            }),
        );
        let answered = responses(&mut peer, 2).await;
        assert_eq!(answered["answer"]["ok"], true);
        let sent: Vec<_> = answered.keys().filter(|id| id.starts_with('c')).collect();
        assert_eq!(sent.len(), 1, "one call let go: {answered:?}");
        assert_eq!(answered[sent[0]]["ok"], true);
        assert_eq!(fixture.apps.calls(), 1);

        // Its slot is free again, for a mount that is not already at its cap.
        send_command(
            &peer,
            "c6",
            "mcp.callTool",
            call_on(&fixture, "c6", "delete_rows", &mount_id(6)),
        );
        until(async || fixture.app_reviews().await.len() == 4).await;

        // Releasing one mount still waiting, on the control lane, ends that
        // call. The other three stay, and the lane has a slot again.
        send_command(
            &peer,
            "release",
            "mcp.releaseApp",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "release",
                "app": app(&fixture, &mount_id(2)),
            }),
        );
        let released = responses(&mut peer, 2).await;
        assert_eq!(released["release"]["payload"]["applied"], true);
        assert_eq!(released["c2"]["error"]["code"], "mcp_cancelled");
        assert_eq!(fixture.app_reviews().await.len(), 3);
        // The released mount opens nothing again, while a slot is free for
        // the service to say so. A full lane would refuse it first.
        send_command(
            &peer,
            "c7",
            "mcp.callTool",
            call_on(&fixture, "c7", "delete_rows", &mount_id(2)),
        );
        assert_eq!(response(&mut peer).await["error"]["code"], "mcp_cancelled");
        send_command(
            &peer,
            "c8",
            "mcp.callTool",
            call_on(&fixture, "c8", "delete_rows", &mount_id(8)),
        );
        until(async || fixture.app_reviews().await.len() == 4).await;

        // The socket goes: its held call is withdrawn, on record.
        drop(peer.input);
        task.await.unwrap();
        until(async || fixture.app_reviews().await.is_empty()).await;
        until(async || {
            fixture.audit.phases().iter().any(|phase| {
                matches!(
                    phase,
                    crate::conversation::application::McpAppAuditPhase::Withdrawn {
                        cause: crate::conversation::application::McpAppWithdrawal::RequestCancelled,
                        ..
                    }
                )
            })
        })
        .await;
        assert_eq!(fixture.apps.calls(), 1);
    }

    #[tokio::test]
    async fn calls_left_running_by_sockets_that_went_still_count_against_a_new_one() {
        let fixture = owners_fixture().await;
        fixture.apps.hold.store(true, Ordering::SeqCst);
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        // Every call the gateway runs at once, four to a socket, each sent and
        // held by its server.
        let mut sockets = Vec::new();
        for socket_number in 0..MAX_APP_CALLS / 4 {
            let session = chat_session(&state, "owner-phone").await;
            let (socket, peer) = test_socket(None);
            let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
            for slot in 0..4 {
                let request = format!("s{socket_number}-{slot}");
                let instance = mount_id(socket_number * 4 + slot + 1);
                send_command(
                    &peer,
                    &request,
                    "mcp.callTool",
                    call_on(&fixture, &request, "read_rows", &instance),
                );
            }
            sockets.push((peer, task));
        }
        until(async || fixture.apps.calls() == MAX_APP_CALLS).await;
        // Every socket goes; every call runs on.
        for (peer, task) in sockets {
            drop(peer.input);
            task.await.unwrap();
        }
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        send_command(&peer, "again", "mcp.callTool", call(&fixture, "again", "read_rows"));
        assert_eq!(
            response(&mut peer).await["error"]["code"],
            "temporarily_unavailable"
        );
        // Once they end, there is room again.
        fixture.apps.hold.store(false, Ordering::SeqCst);
        fixture.apps.gate.0.add_permits(1);
        until(async || {
            fixture
                .audit
                .phases()
                .iter()
                .filter(|phase| {
                    matches!(
                        phase,
                        crate::conversation::application::McpAppAuditPhase::Completed(_)
                    )
                })
                .count()
                == MAX_APP_CALLS
        })
        .await;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let request = format!("after-{attempt}");
            send_command(&peer, &request, "mcp.callTool", call(&fixture, &request, "read_rows"));
            let reply = response(&mut peer).await;
            if reply["ok"] == true {
                break;
            }
            assert_eq!(reply["error"]["code"], "temporarily_unavailable");
            assert!(attempt < 100, "no room after every call ended");
            tokio::task::yield_now().await;
        }
        drop(peer.input);
        task.await.unwrap();
    }

    fn app(fixture: &Fixture, instance: &str) -> serde_json::Value {
        json!({
            "executionId": fixture.execution_id,
            "toolId": fixture.tool_id,
            "instanceId": instance,
        })
    }

    #[tokio::test]
    async fn an_apps_messages_and_contexts_travel_on_its_lane_and_land_as_its_own() {
        let fixture = owners_fixture().await;
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        let message = |request: &str, text: &str, instance: &str| {
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": request,
                "app": app(&fixture, instance),
                "server": SERVER,
                "text": text,
            })
        };
        let context = |request: &str| {
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": request,
                "app": app(&fixture, INSTANCE),
                "server": SERVER,
                "text": "Showing April",
                "structuredContentJson": "{\"month\":4}",
            })
        };

        // Four messages, each from its own mount, waiting on the person: the
        // lane is full, for a context as much as a call (rows M1, C1).
        for (n, request) in ["m1", "m2", "m3", "m4"].into_iter().enumerate() {
            send_command(
                &peer,
                request,
                "mcp.sendMessage",
                message(request, request, &mount_id(n + 1)),
            );
        }
        until(async || fixture.app_reviews().await.len() == 4).await;
        send_command(&peer, "ctx", "mcp.updateModelContext", context("ctx"));
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "ctx");
        assert_eq!(refused["error"]["code"], "temporarily_unavailable");

        // Each message is its own review: allowing one sends that one, with
        // the turn it became, and the others still wait on theirs.
        let answer = |request: &str, review: &serde_json::Value, option: &str| {
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": request,
                "executionId": review["executionId"],
                "permissionId": review["permissionId"],
                "optionId": option,
            })
        };
        let reviews = fixture.app_reviews().await;
        let first = serde_json::to_value(&reviews[0]).unwrap();
        let sent_text = serde_json::from_str::<serde_json::Value>(
            first["argumentsJson"].as_str().unwrap(),
        )
        .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned();
        send_command(&peer, "allow", "conversation.answer", answer("allow", &first, "allow"));
        let answered = responses(&mut peer, 2).await;
        assert_eq!(answered["allow"]["ok"], true);
        let execution = answered[sent_text.as_str()]["payload"]["executionId"]
            .as_str()
            .unwrap()
            .to_owned();
        let waiting = fixture.app_reviews().await;
        assert_eq!(waiting.len(), 3);
        for (n, review) in waiting.iter().enumerate() {
            let request = format!("deny-{n}");
            let review = serde_json::to_value(review).unwrap();
            send_command(&peer, &request, "conversation.answer", answer(&request, &review, "deny"));
        }
        let denied = responses(&mut peer, 6).await;
        for (id, reply) in &denied {
            if id.starts_with('m') {
                assert_eq!(reply["error"]["code"], "mcp_approval_denied", "{id}");
            } else {
                assert_eq!(reply["ok"], true, "{id}");
            }
        }
        assert!(fixture.app_reviews().await.is_empty());

        // Text past the schema's own bound is refused at the wire, with
        // nothing recorded (row M4).
        let records = fixture.audit.phases().len();
        send_command(
            &peer,
            "long-message",
            "mcp.sendMessage",
            message(
                "long-message",
                &"x".repeat(MAX_MCP_MESSAGE_BYTES + 1),
                INSTANCE,
            ),
        );
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "long-message");
        assert_eq!(refused["error"]["code"], "invalid_request");
        assert_eq!(fixture.audit.phases().len(), records);
        assert!(fixture.app_reviews().await.is_empty());

        // Each context part past the schema's own bound is refused at the
        // wire too, with nothing recorded: one rule for every schema bound of
        // both methods (row C3).
        for part in ["text", "structuredContentJson"] {
            let mut long = context("long");
            long[part] = json!(format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_MCP_CONTEXT_BYTES)));
            send_command(&peer, "long", "mcp.updateModelContext", long);
            let refused = response(&mut peer).await;
            assert_eq!(refused["id"], "long");
            assert_eq!(refused["error"]["code"], "invalid_request", "{part}");
            assert_eq!(fixture.audit.phases().len(), records, "{part}");
        }
        // Both within it, and together past what one context may hold: the
        // gateway's own bound, on record.
        let mut long = context("long");
        long["text"] = json!("x".repeat(MAX_MCP_CONTEXT_BYTES / 2 + 1));
        long["structuredContentJson"] =
            json!(format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_MCP_CONTEXT_BYTES / 2)));
        send_command(&peer, "long", "mcp.updateModelContext", long);
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "long");
        assert_eq!(refused["error"]["code"], "mcp_request_too_large");
        assert_eq!(
            fixture.audit.phases()[records..],
            [crate::conversation::application::McpAppAuditPhase::Refused(
                ConversationErrorCode::McpRequestTooLarge
            )]
        );

        // A part at the schema's bound exactly, counted in bytes — two to a
        // character here — is applied; one byte past it is refused at the
        // wire, with nothing recorded.
        let at_bound = |request: &str, text: String| {
            let mut part = context(request);
            part["text"] = json!(text);
            part.as_object_mut().unwrap().remove("structuredContentJson");
            part
        };
        let records = fixture.audit.phases().len();
        let over = format!("{}x", "é".repeat(MAX_MCP_CONTEXT_BYTES / 2));
        assert_eq!(over.len(), MAX_MCP_CONTEXT_BYTES + 1);
        send_command(&peer, "over", "mcp.updateModelContext", at_bound("over", over));
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "over");
        assert_eq!(refused["error"]["code"], "invalid_request");
        assert_eq!(fixture.audit.phases().len(), records);
        let exact = "é".repeat(MAX_MCP_CONTEXT_BYTES / 2);
        assert_eq!(exact.len(), MAX_MCP_CONTEXT_BYTES);
        send_command(&peer, "exact", "mcp.updateModelContext", at_bound("exact", exact));
        let applied = response(&mut peer).await;
        assert_eq!(applied["id"], "exact", "{applied}");
        assert_eq!(applied["payload"], json!({"requestId": "exact", "applied": true}));
        assert_eq!(
            fixture.audit.phases()[records..],
            [crate::conversation::application::McpAppAuditPhase::ContextHeld {
                bytes: MAX_MCP_CONTEXT_BYTES
            }]
        );

        // The context now has room, and is applied.
        send_command(&peer, "ctx2", "mcp.updateModelContext", context("ctx2"));
        let applied = response(&mut peer).await;
        assert_eq!(applied["id"], "ctx2", "{applied}");
        assert_eq!(applied["payload"], json!({"requestId": "ctx2", "applied": true}));

        // The transcript says who wrote it, on the wire.
        until(async || {
            send_command(
                &peer,
                "read",
                "conversation.read",
                json!({"conversationId": fixture.id.to_string()}),
            );
            let view = loop {
                let reply = response(&mut peer).await;
                if reply["id"] == "read" {
                    break reply;
                }
            };
            view["payload"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["executionId"] == execution.as_str()
                        && message["app"]
                            == json!({
                                "executionId": fixture.execution_id,
                                "toolId": fixture.tool_id,
                                "server": SERVER,
                                "tool": crate::app_call_test_support::UI_TOOL,
                            })
                })
        })
        .await;
        drop(peer.input);
        task.await.unwrap();
    }

    /// Row M3: an empty message is outside the schema's own bound
    /// (`minLength`), refused at the wire with nothing recorded and nobody
    /// asked; a blank one — whitespace only — is within it, and is the
    /// conversation's to refuse, on record.
    #[tokio::test]
    async fn m3_an_empty_message_is_refused_at_the_wire_and_a_blank_one_on_record() {
        let fixture = owners_fixture().await;
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        let message = |request: &str, text: &str| {
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": request,
                "app": app(&fixture, INSTANCE),
                "server": SERVER,
                "text": text,
            })
        };
        let records = fixture.audit.phases().len();
        send_command(&peer, "empty", "mcp.sendMessage", message("empty", ""));
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "empty");
        assert_eq!(refused["error"]["code"], "invalid_request");
        assert_eq!(fixture.audit.phases().len(), records);
        assert!(fixture.app_reviews().await.is_empty());

        send_command(&peer, "blank", "mcp.sendMessage", message("blank", " \n\t"));
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "blank");
        assert_eq!(refused["error"]["code"], "invalid_request");
        assert_eq!(
            fixture.audit.phases()[records..],
            [crate::conversation::application::McpAppAuditPhase::Refused(
                ConversationErrorCode::InvalidRequest
            )]
        );
        assert!(fixture.app_reviews().await.is_empty());
        drop(peer.input);
        task.await.unwrap();
    }

    /// Four destructive calls from one mount: three may wait, and the fourth
    /// is refused, leaving a slot another mount can take. A message from that
    /// mount is the same cap.
    #[tokio::test(flavor = "current_thread")]
    async fn one_mount_cannot_fill_the_app_lane() {
        let (captured, _log) = super::limit_log();
        let fixture = owners_fixture().await;
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));

        for request in ["a1", "a2", "a3", "a4"] {
            send_command(
                &peer,
                request,
                "mcp.callTool",
                call(&fixture, request, "delete_rows"),
            );
        }
        let mut refusals = Vec::new();
        timeout(Duration::from_secs(5), async {
            loop {
                if fixture.app_reviews().await.len() + refusals.len() == 4 {
                    break;
                }
                tokio::select! {
                    reply = response(&mut peer) => {
                        assert_eq!(reply["error"]["code"], "temporarily_unavailable", "{reply}");
                        refusals.push(reply);
                    }
                    _ = tokio::task::yield_now() => {}
                }
            }
        })
        .await
        .expect("four calls from one mount settle as reviews or refusals");
        assert_eq!(
            fixture.app_reviews().await.len(),
            3,
            "one mount holds the lane's last slot"
        );
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0]["id"], "a4");
        // The cap is the mount's, across every app method, and it took no slot.
        send_command(
            &peer,
            "msg",
            "mcp.sendMessage",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "msg",
                "app": app(&fixture, INSTANCE),
                "server": SERVER,
                "text": "still this mount",
            }),
        );
        let message = response(&mut peer).await;
        assert_eq!(message["id"], "msg");
        assert_eq!(message["error"]["code"], "temporarily_unavailable");
        assert_eq!(fixture.app_reviews().await.len(), 3);
        super::named(&captured, "socket.app_mount");

        let mut other = call(&fixture, "b1", "delete_rows");
        other["app"]["instanceId"] = json!(crate::app_call_test_support::OTHER_INSTANCE);
        send_command(&peer, "b1", "mcp.callTool", other);
        until(async || fixture.app_reviews().await.len() == 4).await;

        let mut third = call(&fixture, "c1", "delete_rows");
        third["app"]["instanceId"] = json!(mount_id(3));
        send_command(&peer, "c1", "mcp.callTool", third);
        let lane = response(&mut peer).await;
        assert_eq!(lane["id"], "c1");
        assert_eq!(lane["error"]["code"], "temporarily_unavailable");
        super::named(&captured, "socket.app_calls");

        // Denying the four waiting calls ends them and gives their slots
        // back. Releasing the mount would fence it, which is a different rule.
        let waiting = fixture.app_reviews().await;
        assert_eq!(waiting.len(), 4);
        for (n, review) in waiting.iter().enumerate() {
            let request = format!("deny-{n}");
            send_command(
                &peer,
                &request,
                "conversation.answer",
                json!({
                    "conversationId": fixture.id.to_string(),
                    "requestId": request,
                    "executionId": review.execution_id,
                    "permissionId": review.permission_id,
                    "optionId": "deny",
                }),
            );
        }
        let denied = responses(&mut peer, 8).await;
        for (id, reply) in &denied {
            if id.starts_with("deny-") {
                assert_eq!(reply["ok"], true, "{id}");
            } else {
                assert_eq!(reply["error"]["code"], "mcp_approval_denied", "{id}");
            }
        }
        assert!(fixture.app_reviews().await.is_empty());

        for request in ["a5", "a6", "a7"] {
            send_command(
                &peer,
                request,
                "mcp.callTool",
                call(&fixture, request, "delete_rows"),
            );
        }
        until(async || fixture.app_reviews().await.len() == 3).await;
        send_command(
            &peer,
            "a8",
            "mcp.callTool",
            call(&fixture, "a8", "delete_rows"),
        );
        let again = response(&mut peer).await;
        assert_eq!(again["id"], "a8");
        assert_eq!(again["error"]["code"], "temporarily_unavailable");
        send_command(
            &peer,
            "b2",
            "mcp.callTool",
            call_on(
                &fixture,
                "b2",
                "delete_rows",
                crate::app_call_test_support::OTHER_INSTANCE,
            ),
        );
        until(async || fixture.app_reviews().await.len() == 4).await;
        drop(peer.input);
        task.await.unwrap();
    }

    /// A mount turned away because the lane is full is not left at its own
    /// cap: the reservation ends with the slot it did not get.
    #[tokio::test(flavor = "current_thread")]
    async fn a_lane_refusal_does_not_stick_to_the_mount() {
        let fixture = owners_fixture().await;
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        for (n, request) in ["h1", "h2", "h3", "h4"].into_iter().enumerate() {
            send_command(
                &peer,
                request,
                "mcp.callTool",
                call_on(&fixture, request, "delete_rows", &mount_id(n + 1)),
            );
        }
        until(async || fixture.app_reviews().await.len() == 4).await;
        let stuck = mount_id(9);
        for request in ["s1", "s2", "s3"] {
            send_command(
                &peer,
                request,
                "mcp.callTool",
                call_on(&fixture, request, "delete_rows", &stuck),
            );
            let refused = response(&mut peer).await;
            assert_eq!(refused["id"], request);
            assert_eq!(refused["error"]["code"], "temporarily_unavailable");
        }
        for (n, request) in ["h1", "h2", "h3", "h4"].into_iter().enumerate() {
            let release = format!("release-{request}");
            send_command(
                &peer,
                &release,
                "mcp.releaseApp",
                json!({
                    "conversationId": fixture.id.to_string(),
                    "requestId": release,
                    "app": app(&fixture, &mount_id(n + 1)),
                }),
            );
        }
        let released = responses(&mut peer, 8).await;
        for request in ["h1", "h2", "h3", "h4"] {
            assert_eq!(
                released[&format!("release-{request}")]["payload"]["applied"],
                true
            );
            assert_eq!(released[request]["error"]["code"], "mcp_cancelled");
        }
        assert!(fixture.app_reviews().await.is_empty());
        for request in ["s4", "s5", "s6"] {
            send_command(
                &peer,
                request,
                "mcp.callTool",
                call_on(&fixture, request, "delete_rows", &stuck),
            );
        }
        until(async || fixture.app_reviews().await.len() == 3).await;
        send_command(
            &peer,
            "s7",
            "mcp.callTool",
            call_on(&fixture, "s7", "delete_rows", &stuck),
        );
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "s7");
        assert_eq!(refused["error"]["code"], "temporarily_unavailable");
        drop(peer.input);
        task.await.unwrap();
    }

    /// `mcp.releaseApp` is a control, so it fences a call still in admission
    /// and that call opens no review. A later call of the mount is cancelled
    /// too; another mount is not.
    #[tokio::test]
    async fn a_release_on_the_control_lane_fences_a_call_still_in_admission() {
        let fixture = owners_fixture().await;
        let hold = crate::app_call_test_support::Hold::default();
        *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
        let state = chat_state().with_conversations(Arc::new(fixture.service.clone()));
        let session = chat_session(&state, "owner-phone").await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        send_command(
            &peer,
            "c1",
            "mcp.callTool",
            call(&fixture, "c1", "delete_rows"),
        );
        timeout(Duration::from_secs(5), hold.waiting.notified())
            .await
            .expect("the call waits in its first record");
        assert!(fixture.app_reviews().await.is_empty());
        send_command(
            &peer,
            "release",
            "mcp.releaseApp",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "release",
                "app": app(&fixture, INSTANCE),
            }),
        );
        let released = response(&mut peer).await;
        assert_eq!(released["id"], "release");
        assert_eq!(released["payload"]["applied"], true);
        assert!(fixture.app_reviews().await.is_empty());
        hold.go.add_permits(1);
        let cancelled = response(&mut peer).await;
        assert_eq!(cancelled["id"], "c1");
        assert_eq!(cancelled["error"]["code"], "mcp_cancelled");
        assert!(fixture.app_reviews().await.is_empty());
        send_command(
            &peer,
            "c2",
            "mcp.callTool",
            call(&fixture, "c2", "delete_rows"),
        );
        let again = response(&mut peer).await;
        assert_eq!(again["id"], "c2");
        assert_eq!(again["error"]["code"], "mcp_cancelled");
        assert!(fixture.app_reviews().await.is_empty());
        send_command(
            &peer,
            "c3",
            "mcp.callTool",
            call_on(
                &fixture,
                "c3",
                "delete_rows",
                crate::app_call_test_support::OTHER_INSTANCE,
            ),
        );
        until(async || fixture.app_reviews().await.len() == 1).await;
        drop(peer.input);
        task.await.unwrap();
    }
}
