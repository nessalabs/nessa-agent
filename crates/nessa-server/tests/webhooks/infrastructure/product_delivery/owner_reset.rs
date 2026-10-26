//! Actual owner product commands, canonical SDK replacement and signed loopback.
use super::*;
struct Gate {
    transport: Arc<Transport>,
    open: AtomicBool,
}
#[async_trait]
impl WebhookHttp for Gate {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        if !self.open.load(Ordering::SeqCst) {
            return Err(PrepareFailure::Unavailable);
        }
        self.transport.prepare(destination).await
    }
}
pub(super) async fn replace(storage: &RecordStorage, conversation: &ConversationId) {
    let session = SessionId::new(conversation.to_string()).unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
    writer.erase().await.unwrap();
    writer
        .save_changes(
            writer.load().await.unwrap().binding().clone(),
            vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                id: session,
                provider: ProviderIdentity::new("synthetic", "model", "workspace").unwrap(),
                context: ProviderContext::Absent,
            }])
            .unwrap()],
        )
        .await
        .unwrap();
}
async fn frozen(f: &Harness, client: &mut Client, credential: &str) -> Value {
    tokio::time::timeout(WAIT, async {
        loop {
            let result = client
                .call(
                    "conversation.webhook.cursor",
                    json!({"conversationId":f.a.to_string(),"credentialId":credential}),
                )
                .await;
            assert_eq!(result["ok"], true, "{result}");
            if result["payload"]["resetRequired"] == true {
                return result["payload"].clone();
            }
            f.runtime.wake_delivery();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_owner_reset_selects_canonical_incarnation_replays_and_delivers_only_granted_chat() {
    let f = Harness::new().await;
    f.clock.0.store(2_345, Ordering::SeqCst);
    let mut recipient = Recipient::new(false);
    let gate = Arc::new(Gate {
        transport: recipient.transport.clone(),
        open: AtomicBool::new(false),
    });
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    let mut unprivileged = health_reader(&f, &mut client).await;
    let storage = mount(&f, gate.clone()).await;
    two_fresh_cycles(&f).await;
    assert!(recipient.requests.try_recv().is_err());
    replace(storage.as_ref(), &f.a).await;
    f.runtime.wake_delivery();
    let before = frozen(&f, &mut client, &credential).await;
    let params = json!({"conversationId":f.a.to_string(),"credentialId":credential,"requestId":before["requestId"]});
    let denied = unprivileged
        .call("conversation.webhook.reset", params.clone())
        .await;
    assert_eq!(denied["ok"], false, "{denied}");
    let result = client
        .call("conversation.webhook.reset", params.clone())
        .await;
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["payload"]["position"], "0");
    assert_ne!(
        result["payload"]["scope"]["incarnation"],
        before["scope"]["incarnation"]
    );
    assert!(recipient.requests.try_recv().is_err());
    let replay = client
        .call("conversation.webhook.reset", params.clone())
        .await;
    assert_eq!(replay["payload"], result["payload"]);
    let wrong=client.call("conversation.webhook.reset",json!({"conversationId":f.b.to_string(),"credentialId":credential,"requestId":before["requestId"]})).await;
    assert_eq!(wrong["ok"], false, "{wrong}");
    gate.open.store(true, Ordering::SeqCst);
    f.runtime.wake_delivery();
    let first = checked_request(&recipient.next().await, &f.a);
    let second = checked_request(&recipient.next().await, &f.a);
    assert_ne!(first.0, second.0);
    for body in [&first.3, &second.3] {
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            body["incarnation"],
            result["payload"]["scope"]["incarnation"]
        );
    }
    two_fresh_cycles(&f).await;
    let unshared=client.call("conversation.unshare",json!({"requestId":"reset-unshare","conversationId":f.a.to_string(),"credentialId":credential})).await;
    assert_eq!(unshared["ok"], true, "{unshared}");
    f.clock.0.store(4000, Ordering::SeqCst);
    two_fresh_cycles(&f).await;
    assert!(recipient.requests.try_recv().is_err());
    let revoked = client
        .call(
            "credential.revoke",
            json!({"requestId":"reset-revoke","credentialId":credential}),
        )
        .await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    let refused = client.call("conversation.webhook.reset", params).await;
    assert_eq!(refused["ok"], false, "{refused}");
    two_fresh_cycles(&f).await;
    assert!(recipient.requests.try_recv().is_err());
    drop(unprivileged);
    drop(client);
    drop(storage);
    f.close().await.unwrap();
}

struct PausedOwnerHead {
    inner: Arc<dyn PrincipalRecordReadSource>,
    entered: Notify,
    release: Notify,
    armed: AtomicBool,
}
impl PrincipalRecordReadSource for PausedOwnerHead {
    fn read_principal<'a>(
        &'a self,
        selected: PrincipalRecordScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            let owner = selected.principal().as_str() == "owner";
            let response = self
                .inner
                .read_principal(selected, operation, lease)
                .await?;
            if owner && self.armed.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(response)
        })
    }
}
async fn mount_paused(
    f: &Harness,
    http: Arc<dyn WebhookHttp>,
) -> (Arc<RecordStorage>, Arc<PausedOwnerHead>) {
    let storage = canonical_storage(f).await;
    let source = Arc::new(PausedOwnerHead {
        inner: Arc::new(NessaRecordReadSource::new(
            storage.clone(),
            Id::new("synthetic-source").unwrap(),
            tokio::runtime::Handle::current(),
        )),
        entered: Notify::new(),
        release: Notify::new(),
        armed: AtomicBool::new(true),
    });
    f.runtime
        .start_delivery(DeliveryDependencies {
            metadata: f.grants.clone(),
            source: source.clone(),
            policy: Arc::new(CedarConversationAccessPolicy::new().unwrap()),
            capacity: f.state.record_reads.clone(),
            gateway: f.state.gateway.clone(),
            http,
        })
        .unwrap();
    (storage, source)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_reset_commits_observed_j_after_k_replaces_it_then_fresh_read_refreezes_without_k_post(
) {
    let f = Harness::new().await;
    let recipient = Recipient::new(false);
    let gate = Arc::new(Gate {
        transport: recipient.transport.clone(),
        open: AtomicBool::new(false),
    });
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    let (storage, source) = mount_paused(&f, gate).await;
    two_fresh_cycles(&f).await;
    replace(storage.as_ref(), &f.a).await;
    let before = frozen(&f, &mut client, &credential).await;
    let params = json!({"conversationId":f.a.to_string(),"credentialId":credential,"requestId":before["requestId"]});
    let original = params.clone();
    let call = tokio::spawn(async move {
        let result = client.call("conversation.webhook.reset", params).await;
        (client, result)
    });
    tokio::time::timeout(WAIT, source.entered.notified())
        .await
        .unwrap();
    // This real SDK write must complete while the completed J read is held.
    // There is deliberately no canonical writer fence across native reset.
    let replaced = tokio::time::timeout(WAIT, replace(storage.as_ref(), &f.a)).await;
    source.release.notify_one();
    let (mut client, result) = tokio::time::timeout(WAIT, call).await.unwrap().unwrap();
    assert!(
        replaced.is_ok(),
        "physical replacement must not wait on the owner acknowledgement"
    );
    assert_eq!(result["ok"], true, "{result}");
    let after = frozen(&f, &mut client, &credential).await;
    assert_ne!(after["requestId"], before["requestId"]);
    assert_eq!(after["scope"], result["payload"]["scope"]);
    assert_eq!(after["position"], "0");
    let replay = client.call("conversation.webhook.reset", original).await;
    assert_eq!(replay["ok"], true, "{replay}");
    assert_eq!(replay["payload"], result["payload"]);
    let still = client
        .call(
            "conversation.webhook.cursor",
            json!({"conversationId":f.a.to_string(),"credentialId":credential}),
        )
        .await;
    assert_eq!(still["payload"]["requestId"], after["requestId"]);
    assert_eq!(still["payload"]["resetRequired"], true);
    let c = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let queued: i64 = c
        .query_row(
            "SELECT count(*) FROM deliveries WHERE phase IN ('queued','sending','retry')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        queued, 0,
        "K must not be queued or admitted by the reset for J"
    );
    drop(c);
    assert!(recipient.requests.is_empty());
    drop(client);
    drop(source);
    drop(storage);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_reset_refuses_target_expiry_after_head_and_joins_before_queued_revoke() {
    for expiry in [false, true] {
        let f = Harness::new().await;
        let recipient = Recipient::new(false);
        let gate = Arc::new(Gate {
            transport: recipient.transport.clone(),
            open: AtomicBool::new(false),
        });
        let (mut client, _) = Client::open(f.address, &f.owner).await;
        let credential = issue_share(&f, &mut client).await;
        let (storage, source) = mount_paused(&f, gate.clone()).await;
        two_fresh_cycles(&f).await;
        replace(storage.as_ref(), &f.a).await;
        let before = frozen(&f, &mut client, &credential).await;
        let params = json!({"conversationId":f.a.to_string(),"credentialId":credential,"requestId":before["requestId"]});
        let replay = params.clone();
        let call = tokio::spawn(async move {
            let response = client.call("conversation.webhook.reset", params).await;
            (client, response)
        });
        tokio::time::timeout(WAIT, source.entered.notified())
            .await
            .unwrap();
        if expiry {
            f.clock.0.store(100000, Ordering::SeqCst);
        }
        let mut revoke = f.runtime.credentials().revoke(RevokeCredentialRequest {
            request_id: format!("reset-head-revoke-{expiry}"),
            issuer_principal_id: "owner".into(),
            credential_id: credential.clone(),
            revoked_at: 100,
        });
        let pending = revoke.as_mut().now_or_never().is_none();
        source.release.notify_one();
        // Poll both: a waiting writer must not prevent the already-held reset
        // from completing by causing it to acquire another fair Auth read hold.
        let (called, revoked) = tokio::join!(
            tokio::time::timeout(WAIT, call),
            tokio::time::timeout(WAIT, revoke)
        );
        let (mut client, result) = called.unwrap().unwrap();
        assert!(
            pending,
            "the actual owner Auth hold keeps canonical mutation pending"
        );
        assert!(revoked.unwrap().is_ok());
        assert_eq!(result["ok"], !expiry, "{result}");
        gate.open.store(true, Ordering::SeqCst);
        two_fresh_cycles(&f).await;
        let refused = client.call("conversation.webhook.reset", replay).await;
        assert_eq!(
            refused["ok"], false,
            "replay must freshly refuse the revoked target: {refused}"
        );
        assert!(recipient.requests.is_empty());
        drop(client);
        drop(source);
        drop(storage);
        f.close().await.unwrap();
    }
}

struct ReplaceBeforePage {
    inner: Arc<dyn PrincipalRecordReadSource>,
    storage: Arc<RecordStorage>,
    conversation: ConversationId,
    armed: AtomicBool,
    entered: Notify,
    identity_changed: AtomicBool,
}
impl PrincipalRecordReadSource for ReplaceBeforePage {
    fn read_principal<'a>(
        &'a self,
        selected: PrincipalRecordScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            let changing = matches!(operation, RecordReadOperation::Page(_))
                && selected.conversation() == &self.conversation
                && self.armed.swap(false, Ordering::SeqCst);
            if changing {
                replace(self.storage.as_ref(), &self.conversation).await;
            }
            let result = self.inner.read_principal(selected, operation, lease).await;
            if changing {
                self.identity_changed.store(
                    matches!(result, Err(RecordReadError::IdentityChanged)),
                    Ordering::SeqCst,
                );
                self.entered.notify_one();
            }
            result
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_head_page_replacement_pauses_then_freezes_and_requires_owner_reset_before_new_post()
{
    let f = Harness::new().await;
    let mut recipient = Recipient::new(false);
    let gate = Arc::new(Gate {
        transport: recipient.transport.clone(),
        open: AtomicBool::new(false),
    });
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    let storage = canonical_storage(&f).await;
    let source = Arc::new(ReplaceBeforePage {
        inner: Arc::new(NessaRecordReadSource::new(
            storage.clone(),
            Id::new("synthetic-source").unwrap(),
            tokio::runtime::Handle::current(),
        )),
        storage: storage.clone(),
        conversation: f.a.clone(),
        armed: AtomicBool::new(false),
        entered: Notify::new(),
        identity_changed: AtomicBool::new(false),
    });
    f.runtime
        .start_delivery(DeliveryDependencies {
            metadata: f.grants.clone(),
            source: source.clone(),
            policy: Arc::new(CedarConversationAccessPolicy::new().unwrap()),
            capacity: f.state.record_reads.clone(),
            gateway: f.state.gateway.clone(),
            http: gate.clone(),
        })
        .unwrap();
    two_fresh_cycles(&f).await;
    let before = client
        .call(
            "conversation.webhook.cursor",
            json!({"conversationId":f.a.to_string(),"credentialId":credential}),
        )
        .await;
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(before["payload"]["position"], "2");
    let writer = storage
        .open(SessionId::new(f.a.to_string()).unwrap())
        .await
        .unwrap();
    writer
        .save_changes(
            writer.load().await.unwrap().binding().clone(),
            vec![
                SessionSaveUnit::new(vec![SessionChange::ProviderIdentityChanged {
                    before: ProviderIdentity::new("synthetic", "model", "workspace").unwrap(),
                    after: ProviderIdentity::new("synthetic", "model", "workspace-after").unwrap(),
                }])
                .unwrap(),
            ],
        )
        .await
        .unwrap();
    drop(writer);
    source.armed.store(true, Ordering::SeqCst);
    f.runtime.wake_delivery();
    tokio::time::timeout(WAIT, source.entered.notified())
        .await
        .unwrap();
    assert!(
        source.identity_changed.load(Ordering::SeqCst),
        "actual SDK Page must refuse the Head's replaced incarnation"
    );
    let frozen = frozen(&f, &mut client, &credential).await;
    assert_eq!(frozen["scope"], before["payload"]["scope"]);
    assert_eq!(frozen["position"], "2");
    assert!(recipient.requests.is_empty());
    let listed = client.call("credential.list", json!({})).await;
    assert_eq!(
        listed["ok"], true,
        "a physical identity race must not seal the shared command owner: {listed}"
    );
    let c = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let rows: i64 = c
        .query_row("SELECT count(*) FROM deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        rows, 2,
        "the replaced Page cannot advance or queue its third record"
    );
    drop(c);
    let reset=client.call("conversation.webhook.reset",json!({"conversationId":f.a.to_string(),"credentialId":credential,"requestId":frozen["requestId"]})).await;
    assert_eq!(reset["ok"], true, "{reset}");
    gate.open.store(true, Ordering::SeqCst);
    f.runtime.wake_delivery();
    for _ in 0..2 {
        let request = checked_request(&recipient.next().await, &f.a);
        let body: Value = serde_json::from_slice(&request.3).unwrap();
        assert_eq!(
            body["incarnation"],
            reset["payload"]["scope"]["incarnation"]
        );
    }
    drop(client);
    drop(source);
    drop(storage);
    f.close().await.unwrap();
}
