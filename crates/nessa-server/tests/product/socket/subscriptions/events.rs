//! Committed semantic events over the real socket/store/Agent path.
use super::*;

async fn subscribe(client: &mut Client, id: &ConversationId, options: Value) -> String {
    let mut params = options;
    params["conversationId"] = json!(id.to_string());
    client.send("events", "conversation.subscribeEvents", params);
    let (reply, before) = client.reply("events").await;
    assert!(before.is_empty(), "reply precedes event frames");
    assert_eq!(reply["ok"], true, "{reply}");
    reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn through_end(client: &mut Client, subscription: &str, execution: &str) -> Vec<Value> {
    let frames = client
        .until(subscription, |frame| {
            frame["payload"]["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event["kind"] == "turn.ended" && event["executionId"] == execution)
            })
        })
        .await;
    frames
        .into_iter()
        .flat_map(|frame| frame["payload"]["events"].as_array().unwrap().clone())
        .collect()
}

/// E1/E2/E12: distinct accepted/start/settlement facts, committed before delivery.
#[tokio::test]
async fn committed_events_replay_then_follow_with_stable_identity() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    fixture.turn_settled("first").await;
    let mut client = fixture.connect();
    let id = subscribe(&mut client, &fixture.id, json!({"replay":true})).await;
    let first = through_end(&mut client, &id, "first").await;
    let lifecycle: Vec<_> = first
        .iter()
        .filter(|event| event["executionId"] == "first" && event["kind"] != "state.changed")
        .map(|event| event["kind"].as_str().unwrap())
        .collect();
    assert_eq!(lifecycle, ["turn.accepted", "turn.started", "turn.ended"]);
    assert!(first.iter().all(|event| event["id"] == event["cursor"]));
    assert!(first
        .iter()
        .all(|event| event.get("text").is_none() && event.get("arguments").is_none()));
    let after = first.last().unwrap()["cursor"].clone();
    client.close().await;
    let mut resumed = fixture.connect();
    let id = subscribe(&mut resumed, &fixture.id, json!({"after":after})).await;
    fixture.turn("second").await;
    let second = through_end(&mut resumed, &id, "second").await;
    assert!(second
        .iter()
        .all(|event| !first.iter().any(|old| old["id"] == event["id"])));
    resumed.close().await;
}

/// E3/E9: default future-only and server-side filtering preserve progress.
#[tokio::test]
async fn future_event_subscription_filters_without_replaying_old_turns() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("old").await;
    fixture.turn_settled("old").await;
    let mut client = fixture.connect();
    let id = subscribe(&mut client, &fixture.id, json!({"kinds":["turn.ended"]})).await;
    let initial = client.next().await;
    assert_eq!(initial["event"], "conversation.events");
    assert_eq!(initial["payload"]["events"], json!([]));
    assert!(initial["payload"]["cursor"].is_string());
    fixture.turn("new").await;
    let events = through_end(&mut client, &id, "new").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["outcome"], "completed");
    client.close().await;
}

/// A long streamed reply fills several source pages with text no frame
/// carries. Those pages send nothing: a follower takes an empty frame as a
/// sign it has caught up, so the first frame of a replay is the turn's end.
#[tokio::test]
async fn pages_of_unpublished_text_send_no_frame_before_the_head() {
    use nessa_sdk::application::agent_execution::executions::ExecutionUpdate;
    use nessa_sdk::domain::agent_execution::executions::MessageChunk;
    let fixture = SubscriptionFixture::new().await;
    // Several 16-event source pages of nothing but streamed text.
    fixture.provider.execution_updates.lock().unwrap().extend(
        (0..64).map(|chunk| ExecutionUpdate::Message(MessageChunk::text(format!("w{chunk} ")))),
    );
    fixture.turn("long").await;
    fixture.turn_settled("long").await;
    let mut client = fixture.connect();
    let id = subscribe(
        &mut client,
        &fixture.id,
        json!({"replay":true, "kinds":["turn.ended"]}),
    )
    .await;
    let frames = client
        .until(&id, |frame| {
            frame["payload"]["events"]
                .as_array()
                .is_some_and(|events| !events.is_empty())
        })
        .await;
    assert_eq!(frames.len(), 1, "empty frames before the turn's end: {frames:?}");
    assert_eq!(frames[0]["payload"]["events"][0]["kind"], "turn.ended");
    client.close().await;
}

/// E4: malformed cursors refuse before semantic delivery; no state replacement.
#[tokio::test]
async fn invalid_event_cursor_is_a_typed_refusal() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    client.send(
        "bad",
        "conversation.subscribeEvents",
        json!({"conversationId": fixture.id.to_string(),"after":"not-a-cursor"}),
    );
    let (reply, events) = client.reply("bad").await;
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "event_cursor_invalid");
    assert!(events.is_empty());
    client.close().await;
}

/// E23: raw invalid filters refuse before configuration, admission or source work.
#[tokio::test]
async fn raw_event_filters_use_the_published_cardinality_and_uniqueness_contract() {
    use nessa_protocol::product::generated::{ConversationEventKind, MAX_EVENT_SUBSCRIPTION_KINDS};

    let fixture = SubscriptionFixture::new().await;
    let held = fixture.storage.committed_watches();
    let mut unavailable = fixture.state.clone();
    unavailable.conversations = None;
    // Invalid params must not reach even the missing-service/admission branch.
    grants(&fixture.authority, &["server.read"]);
    let mut client = connect(unavailable, &fixture.session);
    for options in [
        json!({"kinds": []}),
        json!({"kinds": ["turn.ended", "turn.ended"]}),
        json!({"kinds": vec!["turn.ended"; MAX_EVENT_SUBSCRIPTION_KINDS + 1]}),
        json!({"kinds": null}),
        json!({"kinds": "turn.ended"}),
        json!({"kinds": ["future.unknown"]}),
        json!({"after": null}),
        json!({"replay": null}),
        json!({"after": 7}),
        json!({"replay": "true"}),
    ] {
        let mut params = options;
        params["conversationId"] = json!(fixture.id.to_string());
        client.send("invalid-filter", "conversation.subscribeEvents", params);
        let (reply, events) = client.reply("invalid-filter").await;
        assert_eq!(reply["error"]["code"], "invalid_request", "{reply}");
        assert!(events.is_empty());
    }
    client.close().await;
    // No failed raw request retained a source watch or touched an Agent.
    assert_eq!(fixture.storage.committed_watches(), held);
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    grants(&fixture.authority, &["conversation.write", "server.read"]);
    for options in [
        json!({}),
        json!({"kinds": [ConversationEventKind::TurnEnded.as_str()]}),
        json!({"kinds": ["turn.accepted", "turn.started", "turn.ended", "tool.updated", "state.changed"]}),
    ] {
        let mut client = fixture.connect();
        let id = subscribe(&mut client, &fixture.id, options).await;
        let first = client.next().await;
        assert_eq!(first["payload"]["subscriptionId"], id);
        client.close().await;
    }
}

/// E6: identity/grant is checked again before each batch, not only on open.
#[tokio::test]
async fn revoked_event_subscription_ends_before_new_events() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = subscribe(&mut client, &fixture.id, json!({})).await;
    client.next().await;
    grants(&fixture.authority, &["server.read"]);
    fixture
        .service
        .submit(
            fixture.id.clone(),
            caller(&fixture.session, "after-revoke".into()),
            "turn-after".into(),
            SubmittedMessage {
 uploads: Vec::new(),
                text: "question".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let frames = client
        .until(&id, |frame| {
            frame["event"] == "conversation.subscriptionEnded"
        })
        .await;
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["payload"]["code"], "forbidden");
    assert!(frames[0]["payload"]["lastEventCursor"].is_string());
    client.close().await;
}

/// E7: stop retires offered frames and releases source registrations.
#[tokio::test]
async fn unsubscribe_events_allows_a_fresh_subscription() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = subscribe(&mut client, &fixture.id, json!({})).await;
    client.next().await;
    client.send(
        "stop",
        "conversation.unsubscribe",
        json!({"subscriptionId":id}),
    );
    let (reply, _) = client.reply("stop").await;
    assert_eq!(reply["ok"], true);
    let next = subscribe(&mut client, &fixture.id, json!({})).await;
    assert_ne!(next, id);
    client.close().await;
}

/// E6: revocation during capacity wait is checked before touching the reader.
#[tokio::test]
async fn capacity_wait_rechecks_event_admission() {
    use crate::product::subscription::{read_event, Target};
    use nessa_sdk::application::agent_execution::sessions::{
        CommittedEventReader, EventFuture, EventRead, EventReadError, EventReadLimits,
    };

    struct Unread(Arc<std::sync::atomic::AtomicUsize>);
    impl CommittedEventReader for Unread {
        fn read(&mut self, _: EventReadLimits) -> EventFuture<'_, EventRead> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(EventReadError::Closed) })
        }
    }

    let fixture = SubscriptionFixture::new().await;
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut reader = Unread(reads.clone());
    let held = fixture
        .state
        .requests
        .clone()
        .acquire_many_owned(u32::try_from(fixture.state.limits.requests()).unwrap())
        .await
        .unwrap();
    let target = Target::Events {
        conversation: fixture.id.clone(),
        after: None,
        replay: false,
        kinds: Vec::new(),
    };
    let read = read_event(
        &fixture.state,
        &fixture.service,
        &fixture.session,
        &target,
        "waiting",
        &mut reader,
        EventReadLimits::new(16, 256 * 1024 * 1024).unwrap(),
    );
    tokio::pin!(read);
    assert!(futures_util::poll!(&mut read).is_pending());
    grants(&fixture.authority, &["server.read"]);
    drop(held);
    assert!(matches!(read.await, Err(code) if code == "forbidden"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

/// E22: a source reply does not retain the authority held before its read.
#[tokio::test]
async fn event_read_reauthorizes_page_and_idle_before_publication() {
    use crate::conversation::application::{conversation_session, ShareGrantTransition};
    use crate::conversation::domain::ShareAccess;
    use crate::product::subscription::{read_event, Target};
    use nessa_sdk::application::agent_execution::sessions::{
        CommittedEvent, CommittedEventCursor, CommittedEventPage, CommittedEventReader,
        EventFuture, EventRead, EventReadLimits, SessionChange,
    };
    use nessa_sync::replication::domain::Id;

    struct GatedReader {
        entered: Arc<AtomicBool>,
        reply: Option<tokio::sync::oneshot::Receiver<EventRead>>,
    }
    impl CommittedEventReader for GatedReader {
        fn read(&mut self, _: EventReadLimits) -> EventFuture<'_, EventRead> {
            self.entered.store(true, Ordering::SeqCst);
            let reply = self.reply.take().expect("one source read");
            Box::pin(async move { Ok(reply.await.expect("release source reply")) })
        }
    }

    for page in [false, true] {
        for change in [
            "owner-valid",
            "owner-delete",
            "paired-valid",
            "share",
            "credential",
            "paired-delete",
        ] {
            let fixture = SubscriptionFixture::new().await;
            let fixture = if change.starts_with("owner-") {
                fixture
            } else {
                let fixture = fixture.with_devices(this_device(true));
                grants(
                    &fixture.authority,
                    &["conversation.read", "conversation.write", "server.read"],
                );
                fixture
                    .grant_to(ShareGrantTransition::Grant(ShareAccess::read()))
                    .await;
                fixture
            };
            let target = Target::Events {
                conversation: fixture.id.clone(),
                after: None,
                replay: false,
                kinds: Vec::new(),
            };
            let cursor = CommittedEventCursor::new(
                conversation_session(&fixture.id),
                Id::new("source").unwrap(),
                1,
                if page { Some(1) } else { None },
            );
            let output = if page {
                EventRead::Page(
                    CommittedEventPage::new(
                        vec![CommittedEvent::change(
                            cursor.clone(),
                            SessionChange::Opened {
                                id: conversation_session(&fixture.id),
                                provider: ProviderIdentity::new("gateway-test", "test", "test")
                                    .unwrap(),
                                context: ProviderContext::Absent,
                            },
                        )],
                        cursor.clone(),
                        CommittedEventCursor::new(
                            conversation_session(&fixture.id),
                            Id::new("source").unwrap(),
                            1,
                            None,
                        ),
                    )
                    .unwrap(),
                )
            } else {
                EventRead::Idle(cursor.clone())
            };
            let (release, reply) = tokio::sync::oneshot::channel();
            let entered = Arc::new(AtomicBool::new(false));
            let mut reader = GatedReader {
                entered: entered.clone(),
                reply: Some(reply),
            };
            let read = read_event(
                &fixture.state,
                &fixture.service,
                &fixture.session,
                &target,
                "publication",
                &mut reader,
                EventReadLimits::new(16, 256 * 1024 * 1024).unwrap(),
            );
            tokio::pin!(read);
            timeout(EXPECTED, async {
                while !entered.load(Ordering::SeqCst) {
                    assert!(futures_util::poll!(&mut read).is_pending());
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                fixture.state.requests.available_permits(),
                fixture.state.limits.requests() - 1,
                "source read retains request capacity"
            );
            let expected = match change {
                "share" => {
                    fixture.grant_to(ShareGrantTransition::Revoke).await;
                    Some("conversation_not_found")
                }
                "credential" => {
                    grants(&fixture.authority, &["server.read"]);
                    Some("forbidden")
                }
                "owner-delete" | "paired-delete" => {
                    let deleted = owner_call(
                        &fixture,
                        "conversation.delete",
                        json!({"conversationId": fixture.id.to_string(), "requestId": "delete-during-read"}),
                    )
                    .await;
                    assert_eq!(deleted["ok"], true, "{deleted}");
                    Some("conversation_deleted")
                }
                "owner-valid" | "paired-valid" => None,
                _ => unreachable!(),
            };
            release.send(output).unwrap();
            let result = timeout(EXPECTED, read).await.unwrap();
            if let Some(code) = expected {
                // No returned Page/Idle means the caller can publish neither
                // the withheld metadata nor its checkpoint.
                assert!(
                    matches!(result, Err(ref actual) if actual == code),
                    "{change}, page={page}: {result:?}"
                );
            } else {
                match result.unwrap() {
                    EventRead::Page(batch) if page => {
                        assert_eq!(batch.next(), &cursor);
                        assert_eq!(batch.events().len(), 1);
                    }
                    EventRead::Idle(actual) if !page => assert_eq!(actual, cursor),
                    other => panic!("unchanged authority preserves source output: {other:?}"),
                }
            }
            assert_eq!(
                fixture.state.requests.available_permits(),
                fixture.state.limits.requests()
            );
        }
    }
}

/// E3: an explicit replay flag cannot accompany a resume cursor.
#[tokio::test]
async fn resume_with_explicit_false_replay_is_invalid() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    client.send(
        "bad",
        "conversation.subscribeEvents",
        json!({"conversationId": fixture.id.to_string(), "after": "cursor", "replay": false}),
    );
    let (reply, events) = client.reply("bad").await;
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "invalid_request");
    assert!(events.is_empty());
    client.close().await;
}

/// E13: event follows share the view admission owner for paired read credentials.
#[tokio::test]
async fn paired_read_grant_follows_events_only_on_shared_conversations() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let mut client = fixture.connect();
    client.send(
        "missing",
        "conversation.subscribeEvents",
        json!({"conversationId":fixture.id.to_string()}),
    );
    let (reply, events) = client.reply("missing").await;
    assert_eq!(reply["error"]["code"], "conversation_not_found", "{reply}");
    assert!(events.is_empty());
    fixture
        .grant_to(
            crate::conversation::application::ShareGrantTransition::Grant(
                crate::conversation::domain::ShareAccess::read(),
            ),
        )
        .await;
    let id = subscribe(&mut client, &fixture.id, json!({})).await;
    let initial = client.next().await;
    assert_eq!(initial["payload"]["subscriptionId"], id);
    client.send(
        "stop",
        "conversation.unsubscribe",
        json!({"subscriptionId":id}),
    );
    let (reply, _) = client.reply("stop").await;
    assert_eq!(reply["ok"], true, "{reply}");
    client.close().await;
}

/// E21: a pairing-issued read credential can capture and stop its Drive target.
#[tokio::test]
async fn paired_read_credential_peeks_then_stops_only_a_current_drive_target() {
    use crate::conversation::{application::ShareGrantTransition, domain::ShareAccess};

    let fixture = SubscriptionFixture::new().await;
    fixture.provider.turn_cancel.store(true, Ordering::SeqCst);
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture.turn("drive-running").await;
    timeout(EXPECTED, async {
        loop {
            if fixture
                .service
                .active_execution(
                    fixture.id.clone(),
                    caller(&fixture.session, "active".into()),
                )
                .await
                .unwrap()
                .as_deref()
                == Some("drive-running")
                && fixture
                    .provider
                    .executions
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|execution| execution == "drive-running")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let fixture = fixture.with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let mut client = fixture.connect();
    for (name, access) in [
        ("unshared", None),
        ("read", Some(ShareAccess::read())),
        ("drive", Some(drive_access())),
    ] {
        if let Some(access) = access {
            fixture.grant_to(ShareGrantTransition::Grant(access)).await;
        }
        client.send(
            "peek",
            "conversation.peek",
            json!({"conversationId": fixture.id.to_string()}),
        );
        let (peek, before) = client.reply("peek").await;
        assert!(before.is_empty());
        if name == "unshared" {
            assert_eq!(peek["error"]["code"], "conversation_not_found", "{peek}");
        } else {
            assert_eq!(peek["ok"], true, "{name}: {peek}");
            assert_eq!(peek["payload"]["executionId"], "drive-running");
        }
        let execution = if name == "drive" {
            peek["payload"]["executionId"].clone()
        } else {
            json!("drive-running")
        };
        client.send(
            "stop",
            "conversation.stop",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": format!("stop-{name}"),
                "executionId": execution,
            }),
        );
        let (stop, before) = client.reply("stop").await;
        assert!(before.is_empty());
        match name {
            "unshared" => assert_eq!(stop["error"]["code"], "conversation_not_found", "{stop}"),
            "read" => assert_eq!(stop["error"]["code"], "forbidden", "{stop}"),
            "drive" => {
                assert_eq!(stop["ok"], true, "{stop}");
                assert_eq!(stop["payload"]["outcome"], "cancelled", "{stop}");
            }
            _ => unreachable!(),
        }
        assert_eq!(
            fixture.provider.cancel_calls.load(Ordering::SeqCst),
            usize::from(name == "drive")
        );
    }
    assert_eq!(
        fixture.provider.cancel_turns.lock().unwrap().as_slice(),
        ["drive-running"]
    );
    client.send(
        "admin",
        "conversation.create",
        json!({
            "conversationId": Uuid::new_v4().to_string(), "requestId": "paired-admin",
        }),
    );
    let (admin, _) = client.reply("admin").await;
    assert_eq!(admin["error"]["code"], "forbidden", "{admin}");
    fixture.grant_to(ShareGrantTransition::Revoke).await;
    client.send(
        "revoked",
        "conversation.peek",
        json!({"conversationId": fixture.id.to_string()}),
    );
    let (revoked, _) = client.reply("revoked").await;
    assert_eq!(
        revoked["error"]["code"], "conversation_not_found",
        "{revoked}"
    );
    client.close().await;
    drop(release);
}

/// E21: the paired-read exception does not change an unpaired surface's grant.
#[tokio::test]
async fn unpaired_read_credential_keeps_manifest_peek_admission() {
    let fixture = SubscriptionFixture::new().await;
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    let mut client = fixture.connect();
    client.send(
        "peek",
        "conversation.peek",
        json!({"conversationId": fixture.id.to_string()}),
    );
    let (peek, _) = client.reply("peek").await;
    assert_eq!(peek["error"]["code"], "forbidden", "{peek}");
    client.close().await;
}

/// E14: transient SDK worker contention releases capacity and rechecks admission.
#[tokio::test(start_paused = true)]
async fn event_reader_capacity_retries_are_bounded_cancelable_and_reauthorized() {
    use crate::product::subscription::{read_event, Target};
    use nessa_sdk::application::agent_execution::sessions::{
        CommittedEventReader, EventFuture, EventRead, EventReadError, EventReadLimits,
    };
    struct Busy {
        reads: Arc<std::sync::atomic::AtomicUsize>,
        succeeds: bool,
    }
    impl CommittedEventReader for Busy {
        fn read(&mut self, _: EventReadLimits) -> EventFuture<'_, EventRead> {
            let count = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
            let capacity = count == 1 || !self.succeeds;
            Box::pin(async move {
                if capacity {
                    Err(EventReadError::Capacity)
                } else {
                    Ok(EventRead::Preparing)
                }
            })
        }
    }
    let fixture = SubscriptionFixture::new().await;
    let target = Target::Events {
        conversation: fixture.id.clone(),
        after: None,
        replay: false,
        kinds: Vec::new(),
    };
    let limits = EventReadLimits::new(16, 256 * 1024 * 1024).unwrap();
    let mut reader = Busy {
        reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        succeeds: true,
    };
    assert!(matches!(
        read_event(
            &fixture.state,
            &fixture.service,
            &fixture.session,
            &target,
            "retry",
            &mut reader,
            limits
        )
        .await,
        Ok(EventRead::Preparing)
    ));
    assert_eq!(reader.reads.load(Ordering::SeqCst), 2);
    let mut reader = Busy {
        reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        succeeds: false,
    };
    let started = tokio::time::Instant::now();
    assert!(
        matches!(read_event(&fixture.state, &fixture.service, &fixture.session, &target, "bounded", &mut reader, limits).await, Err(code) if code == "subscription_capacity")
    );
    assert_eq!(reader.reads.load(Ordering::SeqCst), 7);
    assert!(started.elapsed() <= Duration::from_secs(4));
    let mut reader = Busy {
        reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        succeeds: true,
    };
    let observed = reader.reads.clone();
    {
        let read = read_event(
            &fixture.state,
            &fixture.service,
            &fixture.session,
            &target,
            "revoked",
            &mut reader,
            limits,
        );
        tokio::pin!(read);
        // Admission can suspend before the reader runs. Keep this task runnable
        // until read() reports Capacity and the future parks in its backoff.
        loop {
            assert!(futures_util::poll!(&mut read).is_pending());
            if observed.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            fixture.state.requests.available_permits(),
            fixture.state.limits.requests()
        );
        grants(&fixture.authority, &["server.read"]);
        assert!(matches!(read.await, Err(code) if code == "forbidden"));
    }
    assert_eq!(reader.reads.load(Ordering::SeqCst), 1);
    grants(&fixture.authority, &["conversation.write", "server.read"]);
    let mut reader = Busy {
        reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        succeeds: true,
    };
    let observed = reader.reads.clone();
    {
        let read = read_event(
            &fixture.state,
            &fixture.service,
            &fixture.session,
            &target,
            "cancel",
            &mut reader,
            limits,
        );
        tokio::pin!(read);
        // Admission can suspend before the reader runs. Keep this task runnable
        // until read() reports Capacity and the future parks in its backoff.
        loop {
            assert!(futures_util::poll!(&mut read).is_pending());
            if observed.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(reader.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.state.requests.available_permits(),
        fixture.state.limits.requests()
    );
}
