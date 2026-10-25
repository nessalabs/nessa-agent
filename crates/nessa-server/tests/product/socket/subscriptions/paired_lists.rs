//! Grant-scoped catalogue replacements, through the actual socket owner.
use super::*;
use crate::conversation::application::{
    ConversationSummaries, ShareGrantChange, ShareGrantTransition, ShareGrants,
    MAX_LISTED_CONVERSATIONS,
};
use crate::conversation::domain::ShareAccess;
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationModelId, ConversationSummary,
};

async fn stored_row(fixture: &SubscriptionFixture, at: u64, shared: bool) -> ConversationId {
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    fixture
        .metadata
        .create(
            Conversation::new(
                id.clone(),
                OrganizationId::new("organization").unwrap(),
                PrincipalId::new("principal").unwrap(),
                "fixture".into(),
                "seed".into(),
                1,
                AgentId::Claude,
                ConversationModelId::new("test").unwrap(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    fixture
        .metadata
        .record(
            &id,
            ConversationSummary::new(None, None, at, false).unwrap(),
        )
        .await
        .unwrap();
    if shared {
        assert!(
            fixture
                .metadata
                .change(ShareGrantChange {
                    transition: ShareGrantTransition::Grant(ShareAccess::read()),
                    conversation_id: id.clone(),
                    principal_id: Some(PrincipalId::new("grantee-receiver").unwrap()),
                    receiver_id: Some("receiver".into()),
                    credential_id: CredentialId::new("credential").unwrap(),
                    expected_policy_revision: None,
                    initiator: caller(&fixture.session, Uuid::new_v4().to_string()),
                    at_ms: 1,
                })
                .await
                .unwrap()
                .applied
        );
    }
    id
}

async fn open_list(client: &mut Client) -> (String, Value) {
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, before) = client.reply("list").await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(before.is_empty(), "no frame before the reply: {before:?}");
    let id = reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = client.next().await;
    assert_eq!(first["event"], "conversation.listed", "{first}");
    (id, first)
}

async fn wait_for_watches(fixture: &SubscriptionFixture, expected: usize) {
    timeout(EXPECTED, async {
        while fixture.storage.committed_watches() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("watch ownership reaches the expected count");
}

#[tokio::test]
async fn paired_lists_filter_before_the_bound() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let shared = stored_row(&fixture, 1, true).await;
    for n in 0..=MAX_LISTED_CONVERSATIONS {
        stored_row(&fixture, 100 + n as u64, false).await;
    }
    let listed = fixture.call("conversation.list", json!({})).await;
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["payload"]["complete"], true, "{listed}");
    assert_eq!(
        listed["payload"]["conversations"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        listed["payload"]["conversations"][0]["conversationId"],
        shared.to_string()
    );
    let observed = fixture.call("conversation.observe", json!({})).await;
    assert_eq!(observed["ok"], true, "{observed}");
    assert_eq!(
        observed["payload"]["conversations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        observed["payload"]["conversations"][0]["conversationId"],
        shared.to_string()
    );
    let mut client = fixture.connect();
    let (_, first) = open_list(&mut client).await;
    assert_eq!(first["payload"]["list"], listed["payload"]);
    client.close().await;
}

#[tokio::test]
async fn paired_lists_start_empty_without_grants() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    stored_row(&fixture, 1, false).await;
    let mut client = fixture.connect();
    let (id, first) = open_list(&mut client).await;
    assert_eq!(
        first["payload"]["list"],
        json!({"conversations": [], "complete": true})
    );
    stored_row(&fixture, 2, false).await;
    client.quiet(&id).await;
    client.close().await;
}

#[tokio::test]
async fn paired_lists_keep_channel_ids_without_widening_their_scope() {
    use crate::conversation::application::ChannelStore;
    use crate::conversation::domain::{
        Channel, ChannelDefaults, ChannelFolder, ChannelId, ChannelName, ChannelNotes,
        ChannelParts, ChannelPath, ChannelResources,
    };
    use crate::conversation::infrastructure::{LocalChannelFolders, LocalChannelStore};

    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let shared = stored_row(&fixture, 1, true).await;
    let hidden = stored_row(&fixture, 2, false).await;
    let private = fixture._directory.path().join("conversations");
    let channels = Arc::new(LocalChannelStore::open(&private.join("channels.sqlite3")).unwrap());
    fixture.service.bind_channels(
        channels.clone(),
        Arc::new(LocalChannelFolders::new(&private, None).unwrap()),
    );
    let visible_channel = ChannelId::new("ch_aaaaaaaaaa").unwrap();
    for (channel_id, conversation) in [
        (visible_channel.clone(), &shared),
        (ChannelId::new("ch_bbbbbbbbbb").unwrap(), &hidden),
    ] {
        let request = channel_id.as_str().to_owned();
        channels
            .create(
                Channel::new(
                    channel_id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    ChannelParts {
                        name: ChannelName::new("Fixture channel").unwrap(),
                        folder: ChannelFolder {
                            path: ChannelPath::new(private.clone()).unwrap(),
                            made: false,
                        },
                        defaults: ChannelDefaults {
                            agent: AgentId::Claude,
                            model: ConversationModelId::new("test").unwrap(),
                            mode: ConversationApprovalMode::Ask,
                        },
                        notes: ChannelNotes::new("").unwrap(),
                        resources: ChannelResources::new(Vec::new()).unwrap(),
                    },
                    request,
                    1,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        channels.place(conversation, &channel_id).await.unwrap();
    }
    for method in ["conversation.list", "conversation.observe"] {
        let reply = fixture.call(method, json!({})).await;
        assert_eq!(reply["ok"], true, "{reply}");
        let rows = reply["payload"]["conversations"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{reply}");
        assert_eq!(rows[0]["conversationId"], shared.to_string());
        assert_eq!(rows[0]["channelId"], visible_channel.as_str());
    }
    let mut client = fixture.connect();
    let (subscription, first) = open_list(&mut client).await;
    assert_eq!(
        first["payload"]["list"]["conversations"][0]["channelId"],
        visible_channel.as_str()
    );
    fixture
        .metadata
        .record(
            &hidden,
            ConversationSummary::new(None, None, 30, false).unwrap(),
        )
        .await
        .unwrap();
    client.quiet(&subscription).await;
    fixture
        .metadata
        .record(
            &shared,
            ConversationSummary::new(None, None, 40, false).unwrap(),
        )
        .await
        .unwrap();
    let changed = client.next().await;
    assert_eq!(changed["event"], "conversation.listed", "{changed}");
    let rows = changed["payload"]["list"]["conversations"]
        .as_array()
        .unwrap();
    assert_eq!(rows.len(), 1, "{changed}");
    assert_eq!(rows[0]["conversationId"], shared.to_string());
    assert_eq!(rows[0]["channelId"], visible_channel.as_str());
    assert_eq!(rows[0]["updatedAtMs"], 40);
    client.close().await;
}

#[tokio::test]
async fn an_incomplete_paired_list_ignores_ungranted_wakes() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let oldest = stored_row(&fixture, 1, true).await;
    for n in 0..=MAX_LISTED_CONVERSATIONS {
        stored_row(&fixture, 2 + n as u64, true).await;
    }
    let mut client = fixture.connect();
    let (id, first) = open_list(&mut client).await;
    assert_eq!(first["payload"]["list"]["complete"], false);
    stored_row(&fixture, 1000, false).await;
    client.quiet(&id).await;
    fixture
        .metadata
        .record(
            &oldest,
            ConversationSummary::new(None, None, 1, true).unwrap(),
        )
        .await
        .unwrap();
    let changed = client.next().await;
    assert_eq!(changed["event"], "conversation.listed", "{changed}");
    assert_eq!(
        changed["payload"]["list"]["conversations"],
        first["payload"]["list"]["conversations"]
    );
    assert_eq!(changed["payload"]["list"]["complete"], false);
    client.close().await;
}

#[tokio::test]
async fn a_cancelled_paired_list_releases_its_sources_while_capacity_is_held() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let before = fixture.storage.committed_watches();
    let permits = fixture.state.requests.available_permits() as u32;
    let held = fixture
        .state
        .requests
        .clone()
        .acquire_many_owned(permits)
        .await
        .unwrap();
    let client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    wait_for_watches(&fixture, before + 1).await;
    client.close().await;
    wait_for_watches(&fixture, before).await;
    assert_eq!(fixture.state.requests.available_permits(), 0);
    drop(held);
    assert_eq!(fixture.state.requests.available_permits(), permits as usize);
}

#[tokio::test]
async fn a_paired_list_rechecks_access_after_waiting_for_capacity() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let before = fixture.storage.committed_watches();
    let permits = fixture.state.requests.available_permits() as u32;
    let held = fixture
        .state
        .requests
        .clone()
        .acquire_many_owned(permits)
        .await
        .unwrap();
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    wait_for_watches(&fixture, before + 1).await;
    grants(&fixture.authority, &["server.read"]);
    drop(held);
    let (reply, frames) = client.reply("list").await;
    assert_eq!(reply["error"]["code"], "forbidden", "{reply}");
    assert!(
        frames.is_empty(),
        "no list before refused admission: {frames:?}"
    );
    client.close().await;
    wait_for_watches(&fixture, before).await;
    assert_eq!(fixture.state.requests.available_permits(), permits as usize);
}

struct RefusedListRead(AccessDenial);
impl ConversationAccessPolicyEvaluator for RefusedListRead {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        if ask.action == AccessAction::Read {
            AccessDecision::Deny(self.0.clone())
        } else {
            crate::conversation_test_support::access_policy_ref().evaluate(ask)
        }
    }
}

#[tokio::test]
async fn a_paired_list_uses_the_configured_read_policy() {
    for denial in [AccessDenial::NotPermitted, AccessDenial::Unavailable] {
        let fixture = SubscriptionFixture::new()
            .await
            .with_devices_asking(this_device(true), Arc::new(RefusedListRead(denial)));
        grants(&fixture.authority, &["conversation.read", "server.read"]);
        stored_row(&fixture, 1, true).await;
        for method in ["conversation.list", "conversation.observe"] {
            let reply = fixture.call(method, json!({})).await;
            assert_eq!(reply["ok"], false, "{reply}");
            assert!(reply.get("payload").is_none(), "{reply}");
        }
        let held = fixture.storage.committed_watches();
        let mut client = fixture.connect();
        client.send("list", "conversation.subscribeList", json!({}));
        let (reply, before) = client.reply("list").await;
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(before.is_empty(), "{before:?}");
        wait_for_watches(&fixture, held).await;
        client.close().await;
    }
}

#[tokio::test]
async fn a_paired_list_observation_keeps_its_scoped_cursor() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let mut expected = std::collections::HashSet::new();
    for at in 0..=nessa_sync::replication::catalogue::MAX_CATALOGUE_ENTRIES {
        expected.insert(stored_row(&fixture, at as u64, true).await.to_string());
    }
    let first = fixture.call("conversation.observe", json!({})).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["payload"]["complete"], false, "{first}");
    let cursor = first["payload"]["cursor"].clone();
    let rows = first["payload"]["conversations"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        nessa_sync::replication::catalogue::MAX_CATALOGUE_ENTRIES
    );
    for row in rows {
        assert!(expected.remove(row["conversationId"].as_str().unwrap()));
    }
    stored_row(&fixture, 1000, false).await;
    let last = fixture
        .call("conversation.observe", json!({"cursor": cursor}))
        .await;
    assert_eq!(last["ok"], true, "{last}");
    assert_eq!(last["payload"]["complete"], true, "{last}");
    assert!(last["payload"].get("cursor").is_none());
    let rows = last["payload"]["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(expected.remove(rows[0]["conversationId"].as_str().unwrap()));
    assert!(expected.is_empty());
}

#[tokio::test]
async fn a_paired_list_admits_its_scope_before_service_availability() {
    for (owner, expected) in [
        ("grantor", "forbidden"),
        ("principal", "conversations_not_configured"),
    ] {
        let mut fixture = SubscriptionFixture::new()
            .await
            .with_devices(Devices::new(&[("credential", "receiver", owner, true)]));
        grants(&fixture.authority, &["conversation.read", "server.read"]);
        fixture.state.conversations = None;
        for method in ["conversation.list", "conversation.observe"] {
            let reply = fixture.call(method, json!({})).await;
            assert_eq!(reply["error"]["code"], expected, "{method}: {reply}");
            assert!(reply.get("payload").is_none(), "{reply}");
        }
    }
}

#[path = "paired_lists/stress.rs"]
mod stress;

/// A committed change wakes an established paired list even when its SQL
/// head cannot be read: the real socket ends it with the typed storage failure.
#[tokio::test]
async fn a_paired_list_ends_with_storage_failure_after_its_initial_read() {
    use nessa_local_database::rusqlite::Connection;
    use nessa_protocol::conversation::domain::ConversationPreview;
    use tokio_tungstenite::tungstenite::Message as PeerMessage;
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let shared = stored_row(&fixture, 1, true).await;
    let watch_baseline = fixture.storage.committed_watches();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = fixture.state.clone();
    let session = fixture.session.clone();
    let router = axum::Router::new().route(
        "/probe",
        axum::routing::get(move |ws: axum::extract::WebSocketUpgrade| {
            let state = state.clone();
            let session = session.clone();
            async move { ws.on_upgrade(move |socket| run_authenticated(socket, state, session)) }
        }),
    );
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let (mut peer, _) = tokio_tungstenite::connect_async(format!("ws://{address}/probe"))
        .await
        .unwrap();
    let request =
        RequestFrame::new("fault-list", "conversation.subscribeList", &json!({})).unwrap();
    peer.send(PeerMessage::Text(
        serde_json::to_string(&request).unwrap().into(),
    ))
    .await
    .unwrap();
    let reply_text = timeout(EXPECTED, peer.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let reply: Value = serde_json::from_str(&reply_text).unwrap();
    assert_eq!(reply["type"], "res", "{reply}");
    assert_eq!(reply["id"], "fault-list", "{reply}");
    assert_eq!(reply["ok"], true, "{reply}");
    let subscription = reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let initial_text = timeout(EXPECTED, peer.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let initial: Value = serde_json::from_str(&initial_text).unwrap();
    assert_eq!(initial["event"], "conversation.listed", "{initial}");
    assert_eq!(
        initial["payload"]["subscriptionId"], subscription,
        "{initial}"
    );
    assert_eq!(
        initial["payload"]["list"]["conversations"][0]["conversationId"],
        shared.to_string(),
        "{initial}"
    );
    wait_for_watches(&fixture, watch_baseline + 1).await;
    let path = fixture
        ._directory
        .path()
        .join("conversations/metadata.sqlite3");
    let external = Connection::open(&path).unwrap();
    external.execute("ALTER TABLE share_grant_changes RENAME COLUMN revision TO intentionally_unreadable_revision", []).unwrap();
    assert!(external
        .query_row("SELECT MAX(revision) FROM share_grant_changes", [], |row| {
            row.get::<_, i64>(0)
        })
        .is_err());
    fixture
        .metadata
        .record(
            &shared,
            ConversationSummary::new(
                None,
                Some(ConversationPreview::new("durable-after-fault").unwrap()),
                2,
                false,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let preview: String = external
        .query_row(
            "SELECT preview FROM summaries WHERE conversation_id = ?1",
            [shared.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preview, "durable-after-fault");
    let terminal_text = timeout(Duration::from_secs(3), peer.next())
        .await
        .expect("SQL failure must produce a terminal frame, not silent idle")
        .expect("connection remains open")
        .unwrap()
        .into_text()
        .unwrap();
    let terminal: Value = serde_json::from_str(&terminal_text).unwrap();
    assert_eq!(terminal["type"], "event", "{terminal}");
    assert_eq!(
        terminal["event"], "conversation.subscriptionEnded",
        "{terminal}"
    );
    assert_eq!(
        terminal["payload"]["subscriptionId"], subscription,
        "{terminal}"
    );
    assert_eq!(terminal["payload"]["reason"], "refused", "{terminal}");
    assert_eq!(
        terminal["payload"]["code"], "conversation_storage_unavailable",
        "{terminal}"
    );
    wait_for_watches(&fixture, watch_baseline).await;
    // A websocket ping/pong after the terminal proves the transport is alive.
    peer.send(PeerMessage::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    let pong = timeout(EXPECTED, peer.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(pong, PeerMessage::Pong(ref bytes) if bytes.as_ref() == [1, 2, 3]),
        "{pong:?}"
    );
    peer.close(None).await.unwrap();
    drop(peer);
    stop.send(()).unwrap();
    timeout(EXPECTED, server).await.unwrap().unwrap();
}
