//! Direct tests through the opt-in SDK entry and its injected HTTP boundary.
use async_trait::async_trait;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::clock::RuntimeClock;
use nessa_sdk::infrastructure::mcp::{
    Bearer, ExperimentalEventsError, HttpBody, HttpChunks, HttpExchange, HttpFailure, HttpRequest,
    HttpResponse, McpError, McpOwner, McpServers, NoAuthorization, RemoteAuthorization,
    RemoteMcpServer,
};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

const NAME: &str = "library.book_shelved";
const VERSION: &str = "2026-07-28";
const LIMIT: usize = 256 * 1024;
const MAX_CURSOR: usize = 4096;
const SCOPE_CHALLENGE: &str = "Bearer error=\"insufficient_scope\", scope=\"library:read\"";
const EXTENSION: &str = "io.modelcontextprotocol/events";

#[derive(Clone, Copy)]
enum CursorDisappearance {
    Omitted,
    Null,
}

#[derive(Default)]
struct Peer {
    seen: Mutex<Vec<HttpRequest>>,
    entered: Notify,
    finish: Notify,
    hold_poll: bool,
    annotated: bool,
    nested_annotation: bool,
    disabled: bool,
    sse: bool,
    nonreplay: bool,
    invalid_bootstrap: bool,
    expects_bearer: bool,
    denied: Option<u16>,
    max_escaped_cursor: bool,
    replay_ends: Option<CursorDisappearance>,
}

struct SseChunks(std::vec::IntoIter<Vec<u8>>);

#[async_trait]
impl HttpChunks for SseChunks {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        Ok(Some(self.0.next().expect(
            "SDK must not wait for EOF after a complete reply",
        )))
    }
}

#[async_trait]
impl HttpExchange for Peer {
    async fn exchange(&self, req: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let method = body["method"].as_str().unwrap();
        let header = |name| {
            req.headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("Mcp-Method"), Some(method));
        assert_eq!(
            header("Authorization"),
            self.expects_bearer
                .then_some("Bearer synthetic-fixture-token")
        );
        assert_eq!(header("MCP-Protocol-Version"), Some(VERSION));
        assert_eq!(header("Mcp-Session-Id"), None);
        assert_eq!(
            body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            VERSION
        );
        assert_eq!(
            body["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"]["extensions"]
                [EXTENSION],
            json!({})
        );
        assert_eq!(
            header("Mcp-Name"),
            if method == "events/poll" {
                Some(NAME)
            } else {
                None
            }
        );
        self.seen.lock().unwrap().push(req);
        let result = match method {
            "server/discover" => json!({"resultType":"complete","supportedVersions":[VERSION],
                "capabilities":{"extensions":if self.disabled {json!({})} else {json!({(EXTENSION):{}})}}}),
            "events/list" => {
                let properties = if self.annotated {
                    json!({"room":{"type":"string","x-mcp-header":"Room"}})
                } else if self.nested_annotation {
                    json!({"location":{"type":"object","properties":{
                        "room":{"type":"string","x-mcp-header":"Room"}}}})
                } else {
                    json!({})
                };
                json!({"resultType":"complete","events":[{"name":NAME,"delivery":["poll"],
                    "inputSchema":{"type":"object","properties":properties},"payloadSchema":{"type":"object"}}]})
            }
            "events/poll" => {
                if self.hold_poll {
                    self.entered.notify_one();
                    self.finish.notified().await;
                }
                if let Some(status) = self.denied {
                    return Ok(HttpResponse {
                        status,
                        headers: vec![("WWW-Authenticate".into(), SCOPE_CHALLENGE.into())],
                        body: HttpBody::Buffered(Vec::new()),
                    });
                }
                let events = json!([{"name":NAME,"eventId":"book-7",
                    "timestamp":"2026-11-24T18:00:00Z","data":{"book":"The Moon Atlas"}}]);
                if self.nonreplay {
                    assert!(body["params"]["cursor"].is_null());
                    json!({"resultType":"complete","events":events,
                        "truncated":false,"hasMore":false,"nextPollMs":1200})
                } else if self.max_escaped_cursor {
                    let checkpoint = "\0".repeat(MAX_CURSOR);
                    if !body["params"]["cursor"].is_null() {
                        assert_eq!(body["params"]["cursor"], checkpoint);
                    }
                    json!({"resultType":"complete","events":[],"cursor":checkpoint,
                        "truncated":false,"hasMore":false,"nextPollMs":1000})
                } else if body["params"]["cursor"].is_null() {
                    json!({"resultType":"complete",
                        "events":if self.invalid_bootstrap {events} else {json!([])},
                        "cursor":"opaque-1","truncated":false,"hasMore":false,"nextPollMs":10})
                } else if body["params"]["cursor"] == "opaque-1" {
                    match self.replay_ends {
                        Some(CursorDisappearance::Omitted) => json!({
                            "resultType":"complete","events":[],"truncated":false,
                            "hasMore":false,"nextPollMs":1200
                        }),
                        Some(CursorDisappearance::Null) => json!({
                            "resultType":"complete","events":[],"cursor":null,
                            "truncated":false,"hasMore":false,"nextPollMs":1200
                        }),
                        None => json!({"resultType":"complete","events":events,
                            "cursor":"opaque-2","truncated":false,"hasMore":false,"nextPollMs":1200}),
                    }
                } else {
                    assert_eq!(body["params"]["cursor"], "opaque-2");
                    json!({"resultType":"complete","events":[],
                        "cursor":"opaque-3","truncated":true,"hasMore":false,"nextPollMs":1200})
                }
            }
            _ => panic!("unexpected wire method"),
        };
        let response = json!({"jsonrpc":"2.0","id":body["id"],"result":result});
        let (kind, body) = if self.sse && method == "events/poll" {
            (
                "text/event-stream",
                HttpBody::Stream(Box::new(SseChunks(
                    format!(": fixture heartbeat\r\n\r\ndata: {response}\r\n\r\n")
                        .as_bytes()
                        .chunks(7)
                        .map(<[u8]>::to_vec)
                        .collect::<Vec<_>>()
                        .into_iter(),
                ))),
            )
        } else {
            (
                "application/json",
                HttpBody::Buffered(serde_json::to_vec(&response).unwrap()),
            )
        };
        Ok(HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), kind.into())],
            body,
        })
    }
}

fn install(peer: Arc<Peer>) -> (McpServers, RemoteMcpServer, McpOwner) {
    let servers = McpServers::new(vec![], Arc::new(RuntimeClock::new())).unwrap();
    let remote =
        RemoteMcpServer::new(Uuid::from_u128(1), "library", "http://127.0.0.1:39123/mcp").unwrap();
    servers.set_remote_transport(peer, Arc::new(NoAuthorization));
    servers.replace_all(vec![], vec![remote.clone()]).unwrap();
    (
        servers,
        remote,
        McpOwner::new(SessionId::new("fixture-conversation").unwrap()),
    )
}

#[tokio::test]
async fn actual_sdk_bootstraps_then_returns_one_scoped_occurrence_and_gap() {
    for sse in [false, true] {
        let peer = Arc::new(Peer {
            sse,
            ..Peer::default()
        });
        let (servers, remote, owner) = install(peer.clone());
        let mut poll = servers
            .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
            .await
            .unwrap();
        let init = poll.poll().await.unwrap();
        assert!(init.events.is_empty() && init.replay_available);
        assert_eq!(init.next_poll_ms, 1000);
        let replay = poll.poll().await.unwrap();
        assert_eq!(replay.events[0].event_id, "book-7");
        assert_eq!(replay.events[0].data["book"], "The Moon Atlas");
        assert!(!replay.gap && replay.replay_available && !replay.has_more);
        let gap = poll.poll().await.unwrap();
        assert!(gap.gap && gap.replay_available && gap.events.is_empty());
        assert_eq!(peer.seen.lock().unwrap().len(), 5);
    }
}

#[tokio::test]
async fn unsupported_servers_and_routed_arguments_never_send_poll() {
    for (disabled, annotated, nested_annotation, expected, requests) in [
        (true, false, false, ExperimentalEventsError::Unsupported, 1),
        (
            false,
            true,
            false,
            ExperimentalEventsError::RoutingArgumentsUnsupported,
            2,
        ),
        (
            false,
            false,
            true,
            ExperimentalEventsError::RoutingArgumentsUnsupported,
            2,
        ),
    ] {
        let peer = Arc::new(Peer {
            disabled,
            annotated,
            nested_annotation,
            ..Peer::default()
        });
        let (servers, remote, owner) = install(peer.clone());
        let result = servers
            .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
            .await;
        assert!(matches!(result,Err(error) if error==expected));
        assert_eq!(peer.seen.lock().unwrap().len(), requests);
    }
}

#[tokio::test]
async fn a_different_configured_endpoint_or_existing_revocation_never_sends() {
    let peer = Arc::new(Peer::default());
    let (servers, remote, owner) = install(peer.clone());
    let other =
        RemoteMcpServer::new(remote.id(), remote.name(), "http://127.0.0.1:39124/mcp").unwrap();
    let result = servers
        .open_remote_events_experimental_as(&other, owner.clone(), NAME.into(), Map::new())
        .await;
    assert!(matches!(
        result,
        Err(ExperimentalEventsError::Mcp(McpError::ConfigurationChanged))
    ));
    servers.revoke(&owner);
    let result = servers
        .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
        .await;
    assert!(matches!(
        result,
        Err(ExperimentalEventsError::Mcp(McpError::Closed))
    ));
    assert!(peer.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_held_reply_cannot_publish_after_revoke_or_config_replacement() {
    for revoke in [true, false] {
        let peer = Arc::new(Peer {
            hold_poll: true,
            ..Peer::default()
        });
        let (servers, remote, owner) = install(peer.clone());
        let mut poll = servers
            .open_remote_events_experimental_as(&remote, owner.clone(), NAME.into(), Map::new())
            .await
            .unwrap();
        let change = async {
            peer.entered.notified().await;
            if revoke {
                servers.revoke(&owner);
            } else {
                servers.replace_all(vec![], vec![remote.clone()]).unwrap();
            }
            peer.finish.notify_one();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(poll.poll(), change)
        })
        .await
        .expect("held poll and revocation must both complete");
        let expected = if revoke {
            McpError::Closed
        } else {
            McpError::ConfigurationChanged
        };
        assert!(matches!(result,Err(ExperimentalEventsError::Mcp(error)) if error==expected));
        assert!(
            matches!(poll.poll().await,Err(ExperimentalEventsError::Mcp(error)) if error==expected)
        );
        assert_eq!(peer.seen.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn omitted_cursor_poll_is_supported_without_replay() {
    let peer = Arc::new(Peer {
        nonreplay: true,
        ..Peer::default()
    });
    let (servers, remote, owner) = install(peer.clone());
    let mut poll = servers
        .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
        .await
        .unwrap();
    for _ in 0..2 {
        let batch = poll.poll().await.unwrap();
        assert_eq!(batch.events[0].event_id, "book-7");
        assert!(!batch.replay_available && !batch.gap);
    }
    assert_eq!(peer.seen.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn replay_bootstrap_refuses_nonempty_batch_without_advancing_cursor() {
    let peer = Arc::new(Peer {
        invalid_bootstrap: true,
        ..Peer::default()
    });
    let (servers, remote, owner) = install(peer.clone());
    let mut poll = servers
        .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            poll.poll().await,
            Err(ExperimentalEventsError::InvalidReply)
        ));
    }
    let requests = peer.seen.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for request in &requests[2..] {
        assert!(
            serde_json::from_slice::<Value>(&request.body).unwrap()["params"]["cursor"].is_null()
        );
    }
}

struct FixtureAuthorization(AtomicU64, Mutex<Vec<(Uuid, String)>>);

#[async_trait]
impl RemoteAuthorization for FixtureAuthorization {
    async fn bearer(&self, server: Uuid) -> Result<Option<Bearer>, McpError> {
        assert_eq!(server, Uuid::from_u128(1));
        Ok(Some(Bearer::new(
            "synthetic-fixture-token",
            self.0.load(Ordering::SeqCst),
        )))
    }

    async fn rejected(&self, _: Uuid, _: &str) -> Result<Option<Bearer>, McpError> {
        Err(McpError::Unauthorized)
    }

    async fn insufficient_scope(&self, server: Uuid, challenge: &str) {
        self.1.lock().unwrap().push((server, challenge.to_owned()));
    }
}

#[tokio::test]
async fn old_poll_handle_refuses_a_changed_bearer_generation_before_next_send() {
    let peer = Arc::new(Peer {
        expects_bearer: true,
        ..Peer::default()
    });
    let (servers, remote, owner) = install(peer.clone());
    let authorization = Arc::new(FixtureAuthorization(
        AtomicU64::new(1),
        Mutex::new(Vec::new()),
    ));
    servers.set_authorization(authorization.clone());
    let mut poll = servers
        .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
        .await
        .unwrap();
    assert!(poll.poll().await.unwrap().replay_available);
    authorization.0.store(2, Ordering::SeqCst);
    assert!(matches!(
        poll.poll().await,
        Err(ExperimentalEventsError::Mcp(McpError::Closed))
    ));
    assert_eq!(peer.seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn outbound_arguments_are_capped_before_discovery_or_poll() {
    let peer = Arc::new(Peer::default());
    let (servers, remote, owner) = install(peer.clone());
    for padding in [
        "x".repeat(LIMIT),
        "x".repeat(LIMIT - 4),
        "\n".repeat(LIMIT / 2),
    ] {
        let arguments = Map::from_iter([("padding".into(), json!(padding))]);
        assert!(matches!(
            servers
                .open_remote_events_experimental_as(&remote, owner.clone(), NAME.into(), arguments)
                .await,
            Err(ExperimentalEventsError::Limit)
        ));
    }
    assert!(peer.seen.lock().unwrap().is_empty());
    let arguments = Map::from_iter([("padding".into(), json!("a".repeat(LIMIT / 2)))]);
    let mut handle = servers
        .open_remote_events_experimental_as(&remote, owner, NAME.into(), arguments)
        .await
        .unwrap();
    assert!(handle.poll().await.unwrap().replay_available);
    let seen = peer.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!((LIMIT / 2..=LIMIT).contains(&seen[2].body.len()));
}

#[tokio::test]
async fn exact_argument_budget_preserves_room_for_maximally_escaped_cursor() {
    let peer = Arc::new(Peer {
        max_escaped_cursor: true,
        ..Peer::default()
    });
    let (servers, remote, owner) = install(peer.clone());
    let arguments =
        |padding: usize| Map::from_iter([("padding".into(), json!("a".repeat(padding)))]);
    let mut baseline = servers
        .open_remote_events_experimental_as(&remote, owner.clone(), NAME.into(), arguments(0))
        .await
        .unwrap();
    assert!(baseline.poll().await.unwrap().replay_available);
    assert!(baseline.poll().await.unwrap().replay_available);
    let overhead = peer.seen.lock().unwrap()[3].body.len();
    // Request 4 in the fixture is one digit. Admission covers all 20 digits.
    let remaining_id_digits = u64::MAX.to_string().len() - 1;
    let largest_padding = LIMIT - overhead - remaining_id_digits;
    let count = peer.seen.lock().unwrap().len();
    assert!(matches!(
        servers
            .open_remote_events_experimental_as(
                &remote,
                owner.clone(),
                NAME.into(),
                arguments(largest_padding + 1)
            )
            .await,
        Err(ExperimentalEventsError::Limit)
    ));
    assert_eq!(peer.seen.lock().unwrap().len(), count);
    let mut exact = servers
        .open_remote_events_experimental_as(&remote, owner, NAME.into(), arguments(largest_padding))
        .await
        .unwrap();
    assert!(exact.poll().await.unwrap().replay_available);
    assert!(exact.poll().await.unwrap().replay_available);
    let seen = peer.seen.lock().unwrap();
    assert_eq!(seen.len(), count + 4);
    assert_eq!(seen.last().unwrap().body.len(), LIMIT - remaining_id_digits);
}

#[tokio::test]
async fn scope_refusal_records_challenge_without_replay_or_late_diagnostic() {
    for (status, revoke) in [(403, false), (403, true), (401, false)] {
        let peer = Arc::new(Peer {
            hold_poll: true,
            expects_bearer: true,
            denied: Some(status),
            ..Peer::default()
        });
        let (servers, remote, owner) = install(peer.clone());
        let auth = Arc::new(FixtureAuthorization(
            AtomicU64::new(1),
            Mutex::new(Vec::new()),
        ));
        servers.set_authorization(auth.clone());
        let mut handle = servers
            .open_remote_events_experimental_as(&remote, owner.clone(), NAME.into(), Map::new())
            .await
            .unwrap();
        let change = async {
            peer.entered.notified().await;
            if revoke {
                servers.revoke(&owner);
            }
            peer.finish.notify_one();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(handle.poll(), change)
        })
        .await
        .expect("held refusal must complete");
        let expected = match (status, revoke) {
            (_, true) => McpError::Closed,
            (403, false) => McpError::InsufficientScope,
            _ => McpError::Unauthorized,
        };
        assert!(matches!(result, Err(ExperimentalEventsError::Mcp(error)) if error == expected));
        let seen_challenges = auth.1.lock().unwrap();
        if status == 403 && !revoke {
            assert_eq!(
                *seen_challenges,
                [(remote.id(), SCOPE_CHALLENGE.to_owned())]
            );
        } else {
            assert!(seen_challenges.is_empty());
        }
        assert_eq!(peer.seen.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn refused_mode_change_does_not_discard_the_established_wire_cursor() {
    for replay_ends in [CursorDisappearance::Omitted, CursorDisappearance::Null] {
        let peer = Arc::new(Peer {
            replay_ends: Some(replay_ends),
            ..Peer::default()
        });
        let (servers, remote, owner) = install(peer.clone());
        let mut poll = servers
            .open_remote_events_experimental_as(&remote, owner, NAME.into(), Map::new())
            .await
            .unwrap();
        assert!(poll.poll().await.unwrap().replay_available);
        for _ in 0..2 {
            assert!(matches!(
                poll.poll().await,
                Err(ExperimentalEventsError::ReplayModeChanged)
            ));
        }
        let seen = peer.seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        for request in &seen[3..] {
            assert_eq!(
                serde_json::from_slice::<Value>(&request.body).unwrap()["params"]["cursor"],
                "opaque-1"
            );
        }
    }
}
