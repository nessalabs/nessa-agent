//! Requested canonical diagnostics remain distinct from native progress/effects.
use super::*;
use std::sync::atomic::AtomicU8;

struct WaitingPreparation {
    entered: Notify,
    release: Notify,
    released: AtomicBool,
    calls: AtomicU64,
}
impl WaitingPreparation {
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.release.notify_waiters();
    }
}
#[async_trait]
impl WebhookHttp for WaitingPreparation {
    async fn prepare(&self, _: &Destination) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        loop {
            let notified = self.release.notified();
            if self.released.load(Ordering::SeqCst) {
                break;
            }
            notified.await;
        }
        Err(PrepareFailure::Unavailable)
    }
}
struct ReleasePreparation(Arc<WaitingPreparation>);
impl Drop for ReleasePreparation {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct OwnerProbe {
    inner: Arc<dyn PrincipalRecordReadSource>,
    storage: Arc<RecordStorage>,
    calls: AtomicU64,
    mode: AtomicU8,
    entered: Notify,
    release: Notify,
    released: AtomicBool,
}
impl PrincipalRecordReadSource for OwnerProbe {
    fn read_principal<'a>(
        &'a self,
        selected: PrincipalRecordScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            let owner = selected.principal().as_str() == "owner";
            let mode = if owner {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.mode.load(Ordering::SeqCst)
            } else {
                0
            };
            if mode == 1 {
                return Err(RecordReadError::TemporarilyUnavailable);
            }
            if mode == 3 {
                super::owner_reset::replace(self.storage.as_ref(), selected.conversation()).await;
            }
            let mut response = self
                .inner
                .read_principal(selected, operation, lease)
                .await?;
            if mode == 4 {
                self.entered.notify_one();
                loop {
                    let notified = self.release.notified();
                    if self.released.load(Ordering::SeqCst) {
                        break;
                    }
                    notified.await;
                }
            }
            if mode == 2 {
                if let crate::conversation::application::RecordReadValue::Head(head) =
                    &mut response.value
                {
                    head.scope = nessa_sync::replication::domain::Scope::new(
                        head.scope.receiver().clone(),
                        head.scope.origin().clone(),
                        Id::new(Uuid::new_v4().to_string()).unwrap(),
                        head.scope.incarnation().clone(),
                        head.scope.schema().clone(),
                        head.scope.access_epoch().clone(),
                    );
                }
            }
            Ok(response)
        })
    }
}
fn native_fingerprint(f: &Harness) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let path = f._root.path().join("private");
    let mut audits: Vec<_> = std::fs::read_dir(path.join("audit"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_str().unwrap().to_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect();
    audits.sort();
    (std::fs::read(path.join("outbox.db")).unwrap(), audits)
}
async fn start(f: &Harness) -> (String, Client, Client, Arc<OwnerProbe>, ReleasePreparation) {
    let (mut owner, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(f, &mut owner).await;
    let unprivileged = health_reader(f, &mut owner).await;
    let storage = canonical_storage(f).await;
    let probe = Arc::new(OwnerProbe {
        inner: Arc::new(NessaRecordReadSource::new(
            storage.clone(),
            Id::new("synthetic-source").unwrap(),
            tokio::runtime::Handle::current(),
        )),
        storage,
        calls: AtomicU64::new(0),
        mode: AtomicU8::new(0),
        entered: Notify::new(),
        release: Notify::new(),
        released: AtomicBool::new(false),
    });
    let preparation = Arc::new(WaitingPreparation {
        entered: Notify::new(),
        release: Notify::new(),
        released: AtomicBool::new(false),
        calls: AtomicU64::new(0),
    });
    let release = ReleasePreparation(preparation.clone());
    f.runtime
        .start_delivery(DeliveryDependencies {
            metadata: f.grants.clone(),
            source: probe.clone(),
            policy: Arc::new(CedarConversationAccessPolicy::new().unwrap()),
            capacity: f.state.record_reads.clone(),
            gateway: f.state.gateway.clone(),
            http: preparation.clone(),
        })
        .unwrap();
    tokio::time::timeout(WAIT, preparation.entered.notified())
        .await
        .unwrap();
    (credential, owner, unprivileged, probe, release)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_and_false_cursor_skip_head_admission_and_capacity_while_requested_head_refuses_exhaustion(
) {
    let f = Harness::new().await;
    // Native registration observes milliseconds; canonical Auth issues seconds.
    f.clock.0.store(2_345, Ordering::SeqCst);
    let (credential, mut owner, unprivileged, probe, release) = start(&f).await;
    assert_eq!(
        f.runtime
            .credentials()
            .read(&CredentialId::new(&credential).unwrap())
            .await
            .unwrap()
            .credential
            .issued_at(),
        2
    );
    assert_eq!(
        f.store
            .reset_snapshot(CredentialId::new(&credential).unwrap(), f.a.clone())
            .await
            .unwrap()
            .issued_at_ms,
        2_345,
        "the exact native registration generation keeps its millisecond observation"
    );
    let permits = f.state.record_reads.available_permits();
    assert!(permits > 0);
    let all = f
        .state
        .record_reads
        .clone()
        .try_acquire_many_owned(permits as u32)
        .unwrap();
    let before = native_fingerprint(&f);
    let omitted = owner
        .call(
            "conversation.webhook.cursor",
            json!({"conversationId":f.a.to_string(),"credentialId":credential}),
        )
        .await;
    let explicit=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":false})).await;
    let refused=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    let untouched = before == native_fingerprint(&f);
    let calls = probe.calls.load(Ordering::SeqCst);
    let transport = release.0.calls.load(Ordering::SeqCst);
    drop(all);
    release.0.release();
    drop(owner);
    drop(unprivileged);
    f.close().await.unwrap();
    assert_eq!(omitted["ok"], true, "{omitted}");
    assert_eq!(explicit["payload"], omitted["payload"]);
    assert_eq!(omitted["payload"].as_object().unwrap().len(), 4);
    assert!(omitted["payload"].get("observedSourceHead").is_none());
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "temporarily_unavailable");
    assert_eq!(
        calls, 0,
        "omitted/false and exhausted requests perform no physical Head I/O"
    );
    assert_eq!(
        transport, 1,
        "query does not prepare or dispatch another transport"
    );
    assert!(
        untouched,
        "query does not mutate/publish/reconcile native state or audits"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_diagnostic_refuses_foreign_and_unauthorized_inputs_before_head_then_reports_read_failures_explicitly(
) {
    let f = Harness::new().await;
    let (credential, mut owner, mut unprivileged, probe, release) = start(&f).await;
    let before = native_fingerprint(&f);
    let denied=unprivileged.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    let foreign=owner.call("conversation.webhook.cursor",json!({"conversationId":f.b.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    let wrong_target=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":"foreign-target","includeSourceHead":true})).await;
    let early = probe.calls.load(Ordering::SeqCst);
    probe.mode.store(1, Ordering::SeqCst);
    let unavailable=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    probe.mode.store(2, Ordering::SeqCst);
    let malformed=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    let untouched = before == native_fingerprint(&f);
    let transport = release.0.calls.load(Ordering::SeqCst);
    release.0.release();
    drop(owner);
    drop(unprivileged);
    f.close().await.unwrap();
    for result in [denied, foreign, wrong_target, unavailable, malformed] {
        assert_eq!(result["ok"], false, "{result}");
        assert!(result.get("payload").is_none());
    }
    assert_eq!(
        early, 0,
        "target and current owner admission precede physical Head"
    );
    assert_eq!(transport, 1);
    assert!(
        untouched,
        "refused diagnostic does not create native/audit effects"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_replacement_after_native_snapshot_returns_distinct_observed_head_without_freezing_or_advancing(
) {
    let f = Harness::new().await;
    let (credential, mut owner, unprivileged, probe, release) = start(&f).await;
    let before = native_fingerprint(&f);
    let native = owner
        .call(
            "conversation.webhook.cursor",
            json!({"conversationId":f.a.to_string(),"credentialId":credential}),
        )
        .await;
    probe.mode.store(3, Ordering::SeqCst);
    let diagnostic=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    let after=owner.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":false})).await;
    let untouched = before == native_fingerprint(&f);
    let calls = probe.calls.load(Ordering::SeqCst);
    let transport = release.0.calls.load(Ordering::SeqCst);
    release.0.release();
    drop(owner);
    drop(unprivileged);
    f.close().await.unwrap();
    assert_eq!(native["ok"], true, "{native}");
    assert_eq!(diagnostic["ok"], true, "{diagnostic}");
    assert_eq!(after["payload"], native["payload"]);
    assert_eq!(diagnostic["payload"]["scope"], native["payload"]["scope"]);
    assert_eq!(
        diagnostic["payload"]["position"],
        native["payload"]["position"]
    );
    assert_eq!(diagnostic["payload"]["resetRequired"], false);
    assert_ne!(
        diagnostic["payload"]["observedSourceHead"]["scope"]["incarnation"],
        native["payload"]["scope"]["incarnation"]
    );
    assert_eq!(calls, 1);
    assert_eq!(transport, 1);
    assert!(
        untouched,
        "separate Head observation is not a native freeze/advance or publication"
    );
}

struct ReleaseHead(Arc<OwnerProbe>);
impl Drop for ReleaseHead {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
        self.0.release.notify_waiters();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_head_rechecks_target_expiry_after_physical_observation() {
    let f = Harness::new().await;
    let (credential, mut owner, unprivileged, probe, release) = start(&f).await;
    probe.mode.store(4, Ordering::SeqCst);
    let head = ReleaseHead(probe.clone());
    let before = native_fingerprint(&f);
    let conversation = f.a.to_string();
    let call = tokio::spawn(async move {
        let value=owner.call("conversation.webhook.cursor",json!({"conversationId":conversation,"credentialId":credential,"includeSourceHead":true})).await;
        (owner, value)
    });
    tokio::time::timeout(WAIT, probe.entered.notified())
        .await
        .unwrap();
    f.clock.0.store(100000, Ordering::SeqCst);
    drop(head);
    let (owner, value) = tokio::time::timeout(WAIT, call).await.unwrap().unwrap();
    let untouched = before == native_fingerprint(&f);
    release.0.release();
    drop(owner);
    drop(unprivileged);
    f.close().await.unwrap();
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["error"]["code"], "unauthorized");
    assert!(untouched);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_head_deadline_returns_explicit_refusal_but_shutdown_retains_physical_ownership()
{
    use crate::product::passive_read::deadlines::{PASSIVE_READ_TIMEOUT, RECORD_SEND_TIMEOUT};
    let f = Harness::new().await;
    let (credential, mut owner, unprivileged, probe, release) = start(&f).await;
    probe.mode.store(4, Ordering::SeqCst);
    let head = ReleaseHead(probe.clone());
    let before = native_fingerprint(&f);
    owner.socket.send(Message::Text(json!({"type":"req","id":"diagnostic-timeout","method":"conversation.webhook.cursor","params":{"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true}}).to_string().into())).await.unwrap();
    tokio::time::timeout(WAIT, probe.entered.notified())
        .await
        .unwrap();
    // The receive bound is the published source plus response-delivery phases,
    // rather than Client::call's shorter generic ten-second fixture deadline.
    let value = tokio::time::timeout(PASSIVE_READ_TIMEOUT + RECORD_SEND_TIMEOUT, async {
        loop {
            if let Message::Text(text) = owner.socket.next().await.unwrap().unwrap() {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["id"] == "diagnostic-timeout" {
                    break value;
                }
            }
        }
    })
    .await
    .unwrap();
    let mut shutdown = Box::pin(f.runtime.shutdown());
    let retained = shutdown.as_mut().now_or_never().is_none();
    let untouched = before == native_fingerprint(&f);
    let capacity_held = f.state.record_reads.available_permits();
    drop(head);
    release.0.release();
    tokio::time::timeout(WAIT, shutdown).await.unwrap().unwrap();
    let capacity_after = f.state.record_reads.available_permits();
    drop(owner);
    drop(unprivileged);
    f.close().await.unwrap();
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["error"]["code"], "read_timeout");
    assert!(retained, "deadline must not abandon owned physical work");
    assert!(
        capacity_after > capacity_held,
        "reader permit must survive the deadline until physical completion"
    );
    assert!(untouched);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_checkpoint_needs_no_canonical_read_grant_but_requested_head_does() {
    let f = Harness::with_owner_read(
        None,
        None,
        vec![ConfiguredWebhook {
            principal: PrincipalId::new("hook").unwrap(),
            registration: RegistrationConfig {
                destination: Destination::parse("https://recipient.test/synthetic").unwrap(),
                key: Arc::new(MacKey::new([7; 32])),
            },
        }],
        false,
    )
    .await;
    let (credential, mut manager, unprivileged, probe, release) = start(&f).await;
    let before = native_fingerprint(&f);
    let omitted = manager
        .call(
            "conversation.webhook.cursor",
            json!({"conversationId":f.a.to_string(),"credentialId":credential}),
        )
        .await;
    let explicit=manager.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":false})).await;
    let requested=manager.call("conversation.webhook.cursor",json!({"conversationId":f.a.to_string(),"credentialId":credential,"includeSourceHead":true})).await;
    let untouched = before == native_fingerprint(&f);
    let calls = probe.calls.load(Ordering::SeqCst);
    release.0.release();
    drop(manager);
    drop(unprivileged);
    f.close().await.unwrap();
    assert_eq!(omitted["ok"], true, "{omitted}");
    assert_eq!(explicit["payload"], omitted["payload"]);
    assert_eq!(requested["ok"], false, "{requested}");
    assert_eq!(requested["error"]["code"], "forbidden");
    assert_eq!(calls, 0);
    assert!(untouched);
}
