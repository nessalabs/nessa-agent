//! L13a: transcript settlement precedes the final list summary.
use super::*;
use crate::conversation::application::ConversationSummaries;

struct HeldReplySummary {
    store: Arc<LocalConversationStore>,
    reply: String,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl ConversationSummaries for HeldReplySummary {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<ConversationSummary>> {
        ConversationSummaries::load(self.store.as_ref(), id)
    }

    fn record(
        &self,
        id: &ConversationId,
        summary: ConversationSummary,
    ) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        Box::pin(async move {
            let gate = if summary
                .preview()
                .is_some_and(|preview| preview.as_str() == self.reply)
            {
                self.gate.lock().unwrap().take()
            } else {
                None
            };
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            self.store.record(&id, summary).await
        })
    }

    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.store.erase(id)
    }
}

#[tokio::test]
async fn an_idle_list_before_the_reply_summary_does_not_finish_the_stress_turn() {
    let execution = "held-reply";
    let reply = format!("Response: question {execution}");
    let (entered, summary_waiting) = oneshot::channel();
    let (publish, release) = oneshot::channel();
    let fixture = SubscriptionFixture::with_summaries(|metadata| {
        Arc::new(HeldReplySummary {
            store: metadata.clone(),
            reply: reply.clone(),
            gate: Mutex::new(Some((entered, release))),
        })
    })
    .await;
    fixture.turn("bootstrap").await;
    fixture.settled_read(1).await;
    timeout(EXPECTED, async {
        loop {
            if ConversationSummaries::load(fixture.metadata.as_ref(), &fixture.id)
                .await
                .unwrap()
                .is_some_and(|summary| {
                    summary
                        .preview()
                        .is_some_and(|preview| preview.as_str() == "Response: question bootstrap")
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let hidden = stored_row(&fixture, 2_000_000_000_000, false).await;
    let fixture = fixture.with_devices(this_device(true));
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    fixture
        .grant_to(ShareGrantTransition::Grant(ShareAccess::read()))
        .await;
    let expected = ExpectedCatalogue {
        main: fixture.id.to_string(),
        active: BTreeSet::from([fixture.id.to_string()]),
    };
    let mut client = fixture.connect();
    let (subscription, first) = open_list(&mut client).await;
    expected.frame(&first, &subscription);
    let (finish, executing) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(executing);
    let (owner, authority) = super::super::super::fixture(MembershipRole::Member);
    grants(&authority, &["conversation.write", "server.read"]);
    let owner = owner.with_conversations(fixture.service.clone());
    let owner_session = authenticate(&owner).await;
    let request = RequestFrame::new(
        "send-held-reply", "conversation.send", &json!({
            "conversationId": fixture.id.to_string(), "requestId": "send-held-reply",
            "executionId": execution, "text": format!("question {execution}"), "attachments": [], "files": [],
        }),
    ).unwrap();
    let OutgoingMessage::Response(answer) = dispatch(&owner, &owner_session, request).await else {
        panic!("a turn response")
    };
    assert_eq!(serde_json::to_value(answer).unwrap()["ok"], true);
    fixture.provider.execution_started.notified().await;
    for frame in client
        .until(&subscription, |frame| {
            frame["payload"]["list"]["conversations"][0]["running"] == true
        })
        .await
    {
        expected.frame(&frame, &subscription);
    }
    finish.send(()).unwrap();
    timeout(EXPECTED, summary_waiting).await.unwrap().unwrap();
    fixture.turn_settled(execution).await;
    let idle = client
        .until(&subscription, |frame| {
            frame["payload"]["list"]["conversations"][0]["running"] == false
        })
        .await;
    for frame in &idle {
        expected.frame(frame, &subscription);
    }
    let idle = idle.last().unwrap();
    assert_ne!(
        idle["payload"]["list"]["conversations"][0]["preview"],
        reply
    );
    assert!(
        !reply_settled(idle, &expected.main, &reply),
        "idle before final summary is premature"
    );
    publish.send(()).unwrap();
    let final_frames = client
        .until(&subscription, |frame| {
            reply_settled(frame, &expected.main, &reply)
        })
        .await;
    for frame in &final_frames {
        expected.frame(frame, &subscription);
    }
    let mut still_running = final_frames.last().unwrap().clone();
    still_running["payload"]["list"]["conversations"][0]["running"] = json!(true);
    assert!(
        !reply_settled(&still_running, &expected.main, &reply),
        "a final preview alone does not settle a running turn"
    );
    let before_hidden = head(&fixture).await;
    fixture
        .metadata
        .record(
            &hidden,
            ConversationSummary::new(None, None, 3_000_000_000_000, false).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        head(&fixture).await,
        before_hidden,
        "hidden summary cannot advance the paired head"
    );
    client.quiet(&subscription).await;
    client.close().await;
}
