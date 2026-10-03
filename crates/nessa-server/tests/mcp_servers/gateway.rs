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
        json!({
            "conversationId": fixture.id.to_string(),
            "requestId": request,
            "app": {
                "executionId": fixture.execution_id,
                "toolId": fixture.tool_id,
                "instanceId": INSTANCE,
            },
            "server": SERVER,
            "tool": tool,
        })
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

        // Four destructive calls, each held on its review: the lane is full.
        for request in ["c1", "c2", "c3", "c4"] {
            send_command(&peer, request, "mcp.callTool", call(&fixture, request, "delete_rows"));
        }
        until(async || fixture.app_reviews().await.len() == 4).await;
        send_command(&peer, "c5", "mcp.callTool", call(&fixture, "c5", "delete_rows"));
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

        // The answer still reaches it, and lets its call go.
        let review = &permissions[0];
        send_command(
            &peer,
            "answer",
            "conversation.answer",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "answer",
                "executionId": review["executionId"],
                "permissionId": review["permissionId"],
                "optionId": "allow",
            }),
        );
        let answered = responses(&mut peer, 2).await;
        assert_eq!(answered["answer"]["ok"], true);
        let sent: Vec<_> = answered.keys().filter(|id| id.starts_with('c')).collect();
        assert_eq!(sent.len(), 1, "one call let go: {answered:?}");
        assert_eq!(answered[sent[0]]["ok"], true);
        assert_eq!(fixture.apps.calls(), 1);

        // Its slot is free again.
        send_command(&peer, "c6", "mcp.callTool", call(&fixture, "c6", "delete_rows"));
        until(async || fixture.app_reviews().await.len() == 4).await;

        // Releasing the mount, on the control lane, ends all four as
        // cancelled and frees the lane.
        send_command(
            &peer,
            "release",
            "mcp.releaseApp",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "release",
                "app": {
                    "executionId": fixture.execution_id,
                    "toolId": fixture.tool_id,
                    "instanceId": INSTANCE,
                },
            }),
        );
        let released = responses(&mut peer, 5).await;
        assert_eq!(released["release"]["payload"]["applied"], true);
        for (id, reply) in &released {
            if id != "release" {
                assert_eq!(reply["error"]["code"], "mcp_cancelled", "{id}");
            }
        }
        assert!(fixture.app_reviews().await.is_empty());
        // The released mount opens nothing again; a new mount of the same
        // tool call does.
        send_command(&peer, "c7", "mcp.callTool", call(&fixture, "c7", "delete_rows"));
        assert_eq!(response(&mut peer).await["error"]["code"], "mcp_cancelled");
        let mut fresh = call(&fixture, "c8", "delete_rows");
        fresh["app"]["instanceId"] = json!(crate::app_call_test_support::OTHER_INSTANCE);
        send_command(&peer, "c8", "mcp.callTool", fresh);
        until(async || fixture.app_reviews().await.len() == 1).await;

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
                send_command(&peer, &request, "mcp.callTool", call(&fixture, &request, "read_rows"));
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
        let message = |request: &str, text: &str| {
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": request,
                "app": app(&fixture, INSTANCE),
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

        // Four first messages, each waiting on the person: the lane is full,
        // for a context as much as a call.
        for request in ["m1", "m2", "m3", "m4"] {
            send_command(&peer, request, "mcp.sendMessage", message(request, "hello"));
        }
        until(async || fixture.app_reviews().await.len() == 4).await;
        send_command(&peer, "ctx", "mcp.updateModelContext", context("ctx"));
        let refused = response(&mut peer).await;
        assert_eq!(refused["id"], "ctx");
        assert_eq!(refused["error"]["code"], "temporarily_unavailable");

        // Allowing one allows the mount: all four are answered, each with the
        // turn it became, or `turn_running` behind another's — none waits on
        // the person again.
        let reviews = fixture.app_reviews().await;
        send_command(
            &peer,
            "answer",
            "conversation.answer",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "answer",
                "executionId": reviews[0].execution_id,
                "permissionId": reviews[0].permission_id,
                "optionId": "allow",
            }),
        );
        let answered = responses(&mut peer, 5).await;
        assert_eq!(answered["answer"]["ok"], true);
        let sent: Vec<_> = ["m1", "m2", "m3", "m4"]
            .into_iter()
            .filter(|id| {
                let reply = &answered[*id];
                assert!(
                    reply["ok"] == true || reply["error"]["code"] == "turn_running",
                    "{reply}"
                );
                reply["ok"] == true
            })
            .collect();
        assert!(!sent.is_empty(), "{answered:?}");
        assert!(fixture.app_reviews().await.is_empty());
        let execution = answered[sent[0]]["payload"]["executionId"]
            .as_str()
            .unwrap()
            .to_owned();

        // A part past the schema's own bound is refused before anything
        // parses it, and before the service: nothing is on record of it.
        let records = fixture.audit.phases().len();
        let mut long = context("long");
        long["structuredContentJson"] = json!(format!("{{\"a\":{}1}}", " ".repeat(8192)));
        send_command(&peer, "long", "mcp.updateModelContext", long);
        let refused = loop {
            let reply = response(&mut peer).await;
            if reply["id"] == "long" {
                break reply;
            }
        };
        assert_eq!(refused["error"]["code"], "invalid_request");
        assert_eq!(fixture.audit.phases().len(), records);

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
}
