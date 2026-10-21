//! Seeded paired catalogue histories, with the mock agent and actual socket,
//! service and metadata store (L13). Canonical browser/stress wiring remains a
//! separate gate owned by the shared verification harness.
use super::*;
use crate::conversation::application::ConversationCatalogue;
use std::collections::BTreeSet;

#[path = "settlement.rs"]
mod settlement;

fn reply_settled(frame: &Value, main: &str, reply: &str) -> bool {
    let row = frame["payload"]["list"]["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["conversationId"] == main)
        .unwrap();
    row["running"] == false && row["preview"] == reply
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Hidden,
    Replace,
    Revoke,
    Archive,
    Turn,
    Reconnect,
}

/// A replayable permutation; every cycle exercises every transition.
fn plan(seed: &mut u64) -> [Step; 6] {
    let mut steps = [
        Step::Hidden,
        Step::Replace,
        Step::Revoke,
        Step::Archive,
        Step::Turn,
        Step::Reconnect,
    ];
    for index in (1..steps.len()).rev() {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        steps.swap(index, ((*seed >> 32) as usize) % (index + 1));
    }
    steps
}

/// This fixture's main row is newer than the zero-time synthetic grant rows.
/// Their IDs define the remaining order independently of the listing query.
struct ExpectedCatalogue {
    main: String,
    active: BTreeSet<String>,
}

impl ExpectedCatalogue {
    fn check(&self, list: &Value) {
        let expected: Vec<_> = std::iter::once(self.main.as_str())
            .chain(
                self.active
                    .iter()
                    .map(String::as_str)
                    .filter(|id| *id != self.main),
            )
            .take(MAX_LISTED_CONVERSATIONS)
            .collect();
        let actual: Vec<_> = list["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["conversationId"].as_str().unwrap())
            .collect();
        // The wire budget may shorten the service's bounded list again. Its
        // rows must still be a nonempty prefix of the independent ordering.
        assert!(!actual.is_empty());
        assert!(actual.len() <= expected.len());
        assert_eq!(
            actual,
            expected[..actual.len()],
            "grant-scoped rows in summary order"
        );
        assert_eq!(list["complete"], actual.len() == self.active.len());
    }

    fn frame(&self, frame: &Value, subscription: &str) {
        assert_eq!(frame["event"], "conversation.listed", "{frame}");
        assert_eq!(frame["payload"]["subscriptionId"], subscription);
        self.check(&frame["payload"]["list"]);
    }
}

async fn grant(
    fixture: &SubscriptionFixture,
    conversation: &ConversationId,
    receiver: &str,
    held: bool,
) {
    let changed = fixture
        .metadata
        .change(ShareGrantChange {
            transition: if held {
                ShareGrantTransition::Grant(ShareAccess::read())
            } else {
                ShareGrantTransition::Revoke
            },
            conversation_id: conversation.clone(),
            principal_id: Some(PrincipalId::new("grantee-receiver").unwrap()),
            receiver_id: Some(receiver.into()),
            credential_id: CredentialId::new(if receiver == "receiver" {
                "credential"
            } else {
                receiver
            })
            .unwrap(),
            expected_policy_revision: None,
            initiator: caller(&fixture.session, Uuid::new_v4().to_string()),
            at_ms: 1,
        })
        .await
        .unwrap();
    assert!(changed.applied);
}

async fn head(fixture: &SubscriptionFixture) -> u64 {
    fixture
        .metadata
        .head(
            &OrganizationId::new("organization").unwrap(),
            &PrincipalId::new("principal").unwrap(),
            &crate::conversation::application::Reader::PairedDevice {
                receiver_id: "receiver".into(),
            },
        )
        .await
        .unwrap()
        .revision
}

async fn run(seed: u64, cycles: usize) {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn(&format!("seed-{seed}-bootstrap")).await;
    fixture.settled_read(1).await;
    let (owner, authority) = super::super::fixture(MembershipRole::Member);
    grants(&authority, &["conversation.write", "server.read"]);
    let owner = owner.with_conversations(fixture.service.clone());
    let owner_session = authenticate(&owner).await;

    let fixture = fixture.with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    fixture
        .grant_to(ShareGrantTransition::Grant(ShareAccess::read()))
        .await;
    let mut expected = ExpectedCatalogue {
        main: fixture.id.to_string(),
        active: BTreeSet::from([fixture.id.to_string()]),
    };
    for _ in 0..=MAX_LISTED_CONVERSATIONS {
        expected
            .active
            .insert(stored_row(&fixture, 0, true).await.to_string());
    }
    // More newer hidden rows than the bound: filtering after LIMIT cannot
    // construct the oracle's first page.
    let mut hidden = None;
    for _ in 0..=MAX_LISTED_CONVERSATIONS {
        hidden = Some(stored_row(&fixture, 2_000_000_000_000, false).await);
    }
    let hidden = hidden.unwrap();
    let omitted: Vec<_> = expected
        .active
        .iter()
        .filter(|id| **id != expected.main)
        .rev()
        .take(2)
        .map(|id| ConversationId::new(id).unwrap())
        .collect();
    let (moving, archived) = (&omitted[0], &omitted[1]);
    let watches = fixture.storage.committed_watches();
    let mut client = fixture.connect();
    let (mut subscription, first) = open_list(&mut client).await;
    expected.frame(&first, &subscription);

    let mut random = seed;
    for cycle in 0..cycles {
        for (index, step) in plan(&mut random).into_iter().enumerate() {
            eprintln!("paired catalogue seed={seed} cycle={cycle} step={index} {step:?}");
            match step {
                Step::Hidden => {
                    fixture
                        .metadata
                        .record(
                            &hidden,
                            ConversationSummary::new(
                                None,
                                None,
                                3_000_000_000_000 + cycle as u64,
                                false,
                            )
                            .unwrap(),
                        )
                        .await
                        .unwrap();
                    client.quiet(&subscription).await;
                }
                Step::Replace => {
                    grant(&fixture, moving, "receiver-two", true).await;
                    expected.active.remove(&moving.to_string());
                    expected.frame(&client.next().await, &subscription);
                    let removed_at = head(&fixture).await;
                    grant(&fixture, moving, "receiver-three", true).await;
                    assert_eq!(head(&fixture).await, removed_at, "R1 cannot follow R2→R3");
                    client.quiet(&subscription).await;
                    grant(&fixture, moving, "receiver", true).await;
                    expected.active.insert(moving.to_string());
                    expected.frame(&client.next().await, &subscription);
                }
                Step::Revoke => {
                    grant(&fixture, moving, "receiver", false).await;
                    expected.active.remove(&moving.to_string());
                    expected.frame(&client.next().await, &subscription);
                    grant(&fixture, moving, "receiver", true).await;
                    expected.active.insert(moving.to_string());
                    expected.frame(&client.next().await, &subscription);
                }
                Step::Archive => {
                    for is_archived in [true, false] {
                        fixture
                            .metadata
                            .record(
                                archived,
                                ConversationSummary::new(None, None, 0, is_archived).unwrap(),
                            )
                            .await
                            .unwrap();
                        if is_archived {
                            expected.active.remove(&archived.to_string());
                        } else {
                            expected.active.insert(archived.to_string());
                        }
                        expected.frame(&client.next().await, &subscription);
                    }
                }
                Step::Turn => {
                    let execution = format!("seed-{seed}-cycle-{cycle}");
                    let (release, gate) = tokio::sync::oneshot::channel();
                    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
                    let request = RequestFrame::new(
                        &format!("send-{execution}"),
                        "conversation.send",
                        &json!({
                            "conversationId": fixture.id.to_string(),
                            "requestId": format!("send-{execution}"),
                            "executionId": execution,
                            "text": format!("question {execution}"),
                            "attachments": [], "files": [],
                        }),
                    )
                    .unwrap();
                    let OutgoingMessage::Response(answer) =
                        dispatch(&owner, &owner_session, request).await
                    else {
                        panic!("a turn response")
                    };
                    assert_eq!(serde_json::to_value(answer).unwrap()["ok"], true);
                    fixture.provider.execution_started.notified().await;
                    let running = |frame: &Value| {
                        frame["payload"]["list"]["conversations"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|row| row["conversationId"] == expected.main)
                            .unwrap()["running"]
                            .as_bool()
                            .unwrap()
                    };
                    for frame in client.until(&subscription, running).await {
                        expected.frame(&frame, &subscription);
                    }
                    release.send(()).unwrap();
                    fixture.turn_settled(&execution).await;
                    let reply = format!("Response: question {execution}");
                    for frame in client
                        .until(&subscription, |frame| {
                            reply_settled(frame, &expected.main, &reply)
                        })
                        .await
                    {
                        expected.frame(&frame, &subscription);
                    }
                }
                Step::Reconnect => {
                    client.close().await;
                    wait_for_watches(&fixture, watches).await;
                    client = fixture.connect();
                    let (id, frame) = open_list(&mut client).await;
                    subscription = id;
                    expected.frame(&frame, &subscription);
                }
            }
            let list = fixture.call("conversation.list", json!({})).await;
            assert_eq!(list["ok"], true, "{list}");
            expected.check(&list["payload"]);
        }
        let mut actual = BTreeSet::new();
        let mut cursor = None;
        loop {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({"cursor": cursor}));
            let observed = fixture.call("conversation.observe", params).await;
            assert_eq!(observed["ok"], true, "{observed}");
            let page = &observed["payload"];
            for row in page["conversations"].as_array().unwrap() {
                let id = row["conversationId"].as_str().unwrap().to_owned();
                assert!(expected.active.contains(&id), "no hidden row in a page");
                assert!(actual.insert(id), "a granted row appears only once");
            }
            if page["complete"] == true {
                assert!(page.get("cursor").is_none(), "{page}");
                break;
            }
            let next = page.get("cursor").expect("a cursor for an incomplete page");
            assert_ne!(Some(next), cursor.as_ref(), "the cursor advances");
            cursor = Some(next.clone());
        }
        assert_eq!(
            actual, expected.active,
            "the existing observation cursor scope"
        );
    }
    client.close().await;
    wait_for_watches(&fixture, watches).await;
    assert_eq!(
        fixture.state.requests.available_permits(),
        fixture.state.limits.requests()
    );
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        cycles + 1
    );
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn paired_catalogue_seeded_mock_agent_stress() {
    for seed in [1, 7, 41] {
        run(seed, 1).await;
    }
}

#[tokio::test]
async fn paired_catalogue_seeded_mock_agent_stress_under_load() {
    run(211, 8).await;
}
