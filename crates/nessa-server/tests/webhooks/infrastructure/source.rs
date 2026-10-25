//! Source-only integration: actual Auth/Cedar, grants, registration and SDK reads.
//! No sender or HTTP effect is mounted; this is not recipient delivery E2E.
use super::*;
use crate::conversation::application::{
    PrincipalRecordReadSource, PrincipalRecordScope, RecordReadError, RecordReadFuture,
    RecordReadLease, RecordReadOperation, RecordReadResponse, RecordReadValue,
};
use crate::conversation::infrastructure::{NessaRecordReadSource, TestReadGate};
use crate::webhooks::application::{
    MacKey, ReadWebhookRecords, RegisterWebhook, Registration, RegistrationConfig,
    RegistrationIntent, WebhookReadFailure,
};
use crate::webhooks::domain::Destination;
use crate::webhooks::infrastructure::SqlOutbox;
use nessa_auth::application::credential_admin::CredentialAdmin;
use nessa_protocol::product::generated::{
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
};
use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{ProviderContext, SessionChange, SessionSaveUnit, SessionStorage},
    },
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use nessa_sync::replication::domain::{Id, Limits, PageRequest, Scope};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

async fn registered(f: &Fixture) -> (SqlOutbox, Registration, PathBuf) {
    let private = f._root.path().join("private");
    let path = private.join("outbox.db");
    let store = SqlOutbox::open(
        &path,
        private.join("webhook-audit"),
        Arc::new(Time(AtomicU64::new(2_000))),
    )
    .unwrap();
    let intent = RegistrationIntent {
        id: Uuid::new_v4(),
        observed_at_ms: 1_000,
        issue: IssueCredentialRequest {
            purpose: CredentialIssuePurpose::Webhook,
            request_id: "issue-synthetic-hook".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "hook-key".into(),
            principal: PrincipalInputDto {
                id: "hook".into(),
                kind: PrincipalKindDto::Integration,
            },
            membership: MembershipInputDto {
                id: "hook-member".into(),
                principal_id: "hook".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Member,
                state: MembershipStateDto::Active,
            },
            audience_id: "gateway".into(),
            issued_at: 1,
            expires_at: Some(100),
            grants: vec![CredentialGrantDto {
                action: "conversation.read".into(),
                resource: ResourceDto {
                    organization_id: "org".into(),
                    id: "gateway".into(),
                },
            }],
        },
        config: RegistrationConfig {
            destination: Destination::parse("https://recipient.test/synthetic").unwrap(),
            key: Arc::new(MacKey::new([7; 32])),
        },
    };
    RegisterWebhook {
        credentials: &f.credentials,
        admission: &f.credentials,
        intents: &store,
        stops: &store,
        clock: &f.clock,
    }
    .execute(intent)
    .await
    .unwrap();
    let registration = store
        .registration(f.target.credential.clone())
        .await
        .unwrap()
        .unwrap();
    (store, registration, path)
}
async fn physical(f: &Fixture) -> Arc<RecordStorage> {
    let storage = Arc::new(RecordStorage::new(f._root.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let session = SessionId::new(f.a.to_string()).unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
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
    drop(writer);
    storage
}
fn saved(path: &Path) -> Vec<(String, i64)> {
    let c = nessa_local_database::rusqlite::Connection::open(path).unwrap();
    ["cursors", "deliveries", "audit"]
        .into_iter()
        .map(|table| {
            (
                table.into(),
                c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap(),
            )
        })
        .collect()
}
fn request(scope: Scope, head: u64) -> RecordReadOperation {
    RecordReadOperation::Page(PageRequest {
        scope,
        after: 0,
        target: head,
        max_records: MAX_RECORD_PAGE_RECORDS,
        max_payload_bytes: MAX_RECORD_PAGE_PAYLOAD_BYTES,
        max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    })
}
struct NeverRead(AtomicUsize);
impl PrincipalRecordReadSource for NeverRead {
    fn read_principal<'a>(
        &'a self,
        _: PrincipalRecordScope,
        _: RecordReadOperation,
        _: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { panic!("refused authority must never reach a physical source") })
    }
}

#[tokio::test]
async fn granted_principal_reads_actual_canonical_frames_without_receiver_or_cursor_effects() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let storage = physical(&f).await;
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("source").unwrap(),
        Handle::current(),
    );
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let before = saved(&path);
    let head = read
        .execute(&registration, &f.a, f.epoch, RecordReadOperation::Head)
        .await
        .unwrap();
    let RecordReadValue::Head(head_value) = head.value().unwrap() else {
        panic!("head")
    };
    assert_eq!(head_value.head, 2);
    assert_eq!(
        head_value.scope.receiver().as_str(),
        registration.id.to_string()
    );
    let operation = request(head_value.scope.clone(), head_value.head);
    drop(head);
    let page = read
        .execute(&registration, &f.a, f.epoch, operation)
        .await
        .unwrap();
    let RecordReadValue::Page(value) = page.value().unwrap() else {
        panic!("page")
    };
    assert_eq!(value.records.len(), 2);
    assert_eq!(value.records[0].position, 1);
    assert_eq!(value.records[1].position, 2);
    assert!(value.records.iter().all(|r| r.scope == value.request.scope));
    assert_eq!(
        saved(&path),
        before,
        "source-only reads cannot acknowledge a cursor or enqueue/send"
    );
    f.clock.0.store(100_000, Ordering::SeqCst);
    assert!(matches!(
        page.value(),
        Err(WebhookReadFailure::Admission(AdmissionFailure::Refused))
    ));
    drop(page);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn refused_principals_touch_no_source_and_advance_no_cursor() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let source = NeverRead(AtomicUsize::new(0));
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let before = saved(&path);
    for field in [
        "other-chat",
        "credential",
        "principal",
        "tenant",
        "owner",
        "epoch",
        "expired",
    ] {
        let mut selected = registration.clone();
        let mut conversation = f.a.clone();
        let mut epoch = f.epoch;
        match field {
            "other-chat" => conversation = f.b.clone(),
            "credential" => selected.target.credential = CredentialId::new("owner-key").unwrap(),
            "principal" => selected.target.principal = PrincipalId::new("owner").unwrap(),
            "tenant" => selected.target.organization = OrganizationId::new("other").unwrap(),
            "owner" => selected.target.owner = PrincipalId::new("other").unwrap(),
            "epoch" => epoch += 1,
            "expired" => f.clock.0.store(100_000, Ordering::SeqCst),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                read.execute(&selected, &conversation, epoch, RecordReadOperation::Head,)
                    .await,
                Err(WebhookReadFailure::Admission(AdmissionFailure::Refused))
            ),
            "{field}"
        );
        f.clock.0.store(2_000, Ordering::SeqCst);
    }
    f.change(ShareGrantTransition::Revoke, "source-unshare")
        .await;
    assert!(read
        .execute(&registration, &f.a, f.epoch, RecordReadOperation::Head,)
        .await
        .is_err());
    let next = f
        .change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            "source-regrant",
        )
        .await
        .unwrap();
    assert!(next > f.epoch);
    assert!(read
        .execute(&registration, &f.a, f.epoch, RecordReadOperation::Head,)
        .await
        .is_err());
    f.credentials
        .revoke_sync(RevokeCredentialRequest {
            request_id: "source-revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "hook-key".into(),
            revoked_at: 3,
        })
        .unwrap();
    assert!(read
        .execute(&registration, &f.a, next, RecordReadOperation::Head,)
        .await
        .is_err());
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&path), before);
}

#[tokio::test]
async fn cancelled_principal_read_retains_actual_fences_and_permit_until_physical_join() {
    for panic in [false, true] {
        let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
        let (_store, registration, _path) = registered(&f).await;
        let storage = physical(&f).await;
        let (source, gate) = NessaRecordReadSource::held_for_host_shutdown(
            storage.clone(),
            Id::new("source").unwrap(),
            Handle::current(),
            panic,
        );
        let _release = GateRelease(gate.clone());
        let capacity = Arc::new(Semaphore::new(1));
        let read = ReadWebhookRecords {
            admission: f.admission(),
            source: &source,
            capacity: &capacity,
            limits: read_limits(),
        };
        let mut pending =
            Box::pin(read.execute(&registration, &f.a, f.epoch, RecordReadOperation::Head));
        tokio::select! {
            entered=tokio::time::timeout(Duration::from_secs(3),gate.entered())=>entered.unwrap(),
            _=&mut pending=>panic!("physical read must stay held"),
        }
        let mut revoke = Box::pin(f.credentials.revoke(RevokeCredentialRequest {
            request_id: "source-held-revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "hook-key".into(),
            revoked_at: 3,
        }));
        let mut unshare = Box::pin(f.change(ShareGrantTransition::Revoke, "source-held-unshare"));
        assert!(matches!(
            revoke
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        assert!(matches!(
            unshare
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(pending);
        assert_eq!(capacity.available_permits(), 0);
        assert!(matches!(
            revoke
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        assert!(matches!(
            unshare
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        let mut shutdown = Box::pin(source.shutdown());
        assert!(matches!(
            shutdown
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        gate.release();
        assert_eq!(
            shutdown.await,
            if panic {
                Err(RecordReadError::WorkerPanicked)
            } else {
                Ok(())
            }
        );
        revoke.await.unwrap();
        unshare.await;
        assert_eq!(capacity.available_permits(), 1);
        storage.shutdown().await.unwrap();
    }
}

struct GateRelease(Arc<TestReadGate>);
impl Drop for GateRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test]
async fn expiry_during_a_real_physical_read_releases_the_fence_and_exposes_no_page() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let storage = physical(&f).await;
    let (source, gate) = NessaRecordReadSource::held_for_host_shutdown(
        storage.clone(),
        Id::new("source").unwrap(),
        Handle::current(),
        false,
    );
    let _release = GateRelease(gate.clone());
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let mut pending =
        Box::pin(read.execute(&registration, &f.a, f.epoch, RecordReadOperation::Head));
    tokio::select! { entered=tokio::time::timeout(Duration::from_secs(3),gate.entered())=>entered.unwrap(), _=&mut pending=>panic!("read must remain held") }
    f.clock.0.store(100_000, Ordering::SeqCst);
    gate.release();
    assert!(matches!(
        pending.await,
        Err(WebhookReadFailure::Admission(AdmissionFailure::Refused))
    ));
    assert_eq!(capacity.available_permits(), 1);
    assert_eq!(saved(&path), before);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}
fn selected_scope(registration: &Registration, conversation: &ConversationId, epoch: u64) -> Scope {
    Scope::new(
        Id::new(registration.id.to_string()).unwrap(),
        Id::new("source").unwrap(),
        Id::new(conversation.to_string()).unwrap(),
        Id::new("incarnation").unwrap(),
        Id::new("schema").unwrap(),
        Id::new(format!("epoch-{epoch}")).unwrap(),
    )
}
#[tokio::test]
async fn substituted_selectors_and_unbounded_page_requests_are_rejected_before_source_io() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let source = NeverRead(AtomicUsize::new(0));
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let mut another_registration = registration.clone();
    another_registration.id = Uuid::new_v4();
    let scopes = [
        selected_scope(&another_registration, &f.a, f.epoch),
        selected_scope(&registration, &f.b, f.epoch),
        selected_scope(&registration, &f.a, f.epoch + 1),
    ];
    for scope in scopes {
        assert!(read
            .execute(&registration, &f.a, f.epoch, request(scope, 2),)
            .await
            .is_err());
    }
    let RecordReadOperation::Page(mut page) =
        request(selected_scope(&registration, &f.a, f.epoch), 2)
    else {
        unreachable!()
    };
    page.max_records = usize::MAX;
    assert!(read
        .execute(
            &registration,
            &f.a,
            f.epoch,
            RecordReadOperation::Page(page),
        )
        .await
        .is_err());
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&path), before);
}
struct ChangedReply<'a> {
    source: &'a NessaRecordReadSource,
    mode: &'static str,
}
impl PrincipalRecordReadSource for ChangedReply<'_> {
    fn read_principal<'a>(
        &'a self,
        scope: PrincipalRecordScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            let mut response = self.source.read_principal(scope, operation, lease).await?;
            if let RecordReadValue::Head(head) = &mut response.value {
                let s = &head.scope;
                let wrong = Id::new("wrong-subject").unwrap();
                head.scope = Scope::new(
                    if self.mode == "head-subject" {
                        wrong.clone()
                    } else {
                        s.receiver().clone()
                    },
                    s.origin().clone(),
                    if self.mode == "head-chat" {
                        wrong.clone()
                    } else {
                        s.stream().clone()
                    },
                    s.incarnation().clone(),
                    s.schema().clone(),
                    if self.mode == "head-epoch" {
                        wrong
                    } else {
                        s.access_epoch().clone()
                    },
                );
                return Ok(response);
            }
            let RecordReadValue::Page(page) = &mut response.value else {
                panic!("page")
            };
            match self.mode {
                "request" => page.request.after += 1,
                "record-scope" => {
                    let s = &page.records[0].scope;
                    page.records[0].scope = Scope::new(
                        s.receiver().clone(),
                        s.origin().clone(),
                        s.stream().clone(),
                        Id::new("different-incarnation").unwrap(),
                        s.schema().clone(),
                        s.access_epoch().clone(),
                    );
                }
                "position" => page.records[0].position = 0,
                "payload" => {
                    page.records[0].payload = vec![0; MAX_PHYSICAL_RECORD_PAYLOAD_BYTES + 1]
                }
                _ => unreachable!(),
            }
            Ok(response)
        })
    }
}
#[tokio::test]
async fn contradictory_canonical_page_replies_cannot_escape_the_original_scope_or_bounds() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let storage = physical(&f).await;
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("source").unwrap(),
        Handle::current(),
    );
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let head = read
        .execute(&registration, &f.a, f.epoch, RecordReadOperation::Head)
        .await
        .unwrap();
    let RecordReadValue::Head(value) = head.value().unwrap() else {
        panic!("head")
    };
    let scope = value.scope.clone();
    let count = value.head;
    drop(head);
    for mode in ["request", "record-scope", "position", "payload"] {
        let changed = ChangedReply {
            source: &source,
            mode,
        };
        let read = ReadWebhookRecords {
            admission: f.admission(),
            source: &changed,
            capacity: &capacity,
            limits: read_limits(),
        };
        assert!(
            read.execute(&registration, &f.a, f.epoch, request(scope.clone(), count),)
                .await
                .is_err(),
            "{mode}"
        );
        assert_eq!(saved(&path), before);
    }
    let valid = read
        .execute(&registration, &f.a, f.epoch, request(scope, count))
        .await
        .unwrap();
    assert!(matches!(valid.value(), Ok(RecordReadValue::Page(_))));
    drop(valid);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_waiting_for_grant_keeps_auth_first_and_rechecks_expiry_before_io() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let source = NeverRead(AtomicUsize::new(0));
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let held = f
        .grants
        .hold_principal_revision(&f.a, &f.target.principal, f.epoch)
        .await
        .unwrap();
    let mut pending =
        Box::pin(read.execute(&registration, &f.a, f.epoch, RecordReadOperation::Head));
    assert!(matches!(
        pending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    let mut revoke = Box::pin(f.credentials.revoke(RevokeCredentialRequest {
        request_id: "read-waiting-grant-revoke".into(),
        issuer_principal_id: "owner".into(),
        credential_id: "hook-key".into(),
        revoked_at: 3,
    }));
    assert!(
        matches!(
            revoke
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ),
        "Auth is already held before waiting for the grant"
    );
    f.clock.0.store(100_000, Ordering::SeqCst);
    drop(held);
    assert!(matches!(
        pending.await,
        Err(WebhookReadFailure::Admission(AdmissionFailure::Refused))
    ));
    revoke.await.unwrap();
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&path), before);
}

#[tokio::test]
async fn another_subject_chat_or_epoch_in_a_head_reply_is_not_an_authorized_source() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let storage = physical(&f).await;
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("source").unwrap(),
        Handle::current(),
    );
    let capacity = Arc::new(Semaphore::new(1));
    for mode in ["head-subject", "head-chat", "head-epoch"] {
        let changed = ChangedReply {
            source: &source,
            mode,
        };
        let read = ReadWebhookRecords {
            admission: f.admission(),
            source: &changed,
            capacity: &capacity,
            limits: read_limits(),
        };
        assert!(
            read.execute(&registration, &f.a, f.epoch, RecordReadOperation::Head)
                .await
                .is_err(),
            "{mode}"
        );
        assert_eq!(capacity.available_permits(), 1);
        assert_eq!(saved(&path), before);
    }
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn exhausted_shared_read_capacity_refuses_before_source_or_authority_waits() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let capacity = Arc::new(Semaphore::new(1));
    let permit = capacity.clone().try_acquire_owned().unwrap();
    let source = NeverRead(AtomicUsize::new(0));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let grant = f
        .grants
        .hold_principal_revision(&f.a, &f.target.principal, f.epoch)
        .await
        .unwrap();
    let mut pending =
        Box::pin(read.execute(&registration, &f.a, f.epoch, RecordReadOperation::Head));
    assert!(matches!(
        pending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(WebhookReadFailure::Source(
            RecordReadError::TemporarilyUnavailable
        )))
    ));
    drop(pending);
    drop(grant);
    drop(permit);
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&path), before);
}

fn read_limits() -> Limits {
    Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .unwrap()
}

#[tokio::test]
async fn a_retained_page_selector_cannot_cross_unshare_regrant_or_changed_physical_identity() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let storage = physical(&f).await;
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("source").unwrap(),
        Handle::current(),
    );
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let head = read
        .execute(&registration, &f.a, f.epoch, RecordReadOperation::Head)
        .await
        .unwrap();
    let RecordReadValue::Head(value) = head.value().unwrap() else {
        panic!("head")
    };
    let retained = value.scope.clone();
    let count = value.head;
    drop(head);
    for part in ["origin", "incarnation", "schema"] {
        let s = &retained;
        let wrong = Id::new("different-physical-identity").unwrap();
        let changed = Scope::new(
            s.receiver().clone(),
            if part == "origin" {
                wrong.clone()
            } else {
                s.origin().clone()
            },
            s.stream().clone(),
            if part == "incarnation" {
                wrong.clone()
            } else {
                s.incarnation().clone()
            },
            if part == "schema" {
                wrong
            } else {
                s.schema().clone()
            },
            s.access_epoch().clone(),
        );
        assert!(
            matches!(
                read.execute(&registration, &f.a, f.epoch, request(changed, count))
                    .await,
                Err(WebhookReadFailure::Source(RecordReadError::Admission(_)))
            ),
            "{part}"
        );
        assert_eq!(capacity.available_permits(), 1);
    }
    f.change(ShareGrantTransition::Revoke, "cursor-unshare")
        .await;
    let next = f
        .change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            "cursor-regrant",
        )
        .await
        .unwrap();
    let never = NeverRead(AtomicUsize::new(0));
    let denied = ReadWebhookRecords {
        admission: f.admission(),
        source: &never,
        capacity: &capacity,
        limits: read_limits(),
    };
    assert!(matches!(
        denied
            .execute(&registration, &f.a, next, request(retained, count))
            .await,
        Err(WebhookReadFailure::Source(RecordReadError::Admission(_)))
    ));
    assert_eq!(never.0.load(Ordering::SeqCst), 0);
    let head = read
        .execute(&registration, &f.a, next, RecordReadOperation::Head)
        .await
        .unwrap();
    let RecordReadValue::Head(value) = head.value().unwrap() else {
        panic!("head")
    };
    let fresh = value.scope.clone();
    drop(head);
    let page = read
        .execute(&registration, &f.a, next, request(fresh, count))
        .await
        .unwrap();
    assert!(matches!(page.value(), Ok(RecordReadValue::Page(_))));
    drop(page);
    assert_eq!(saved(&path), before);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn canonical_principal_page_uses_the_existing_durable_registration_scope_contract() {
    use crate::webhooks::application::{Outbox, QueueInitiator, QueuedRecord};
    use crate::webhooks::infrastructure::record_message;
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (store, registration, _path) = registered(&f).await;
    let storage = physical(&f).await;
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("source").unwrap(),
        Handle::current(),
    );
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let head = read
        .execute(&registration, &f.a, f.epoch, RecordReadOperation::Head)
        .await
        .unwrap();
    let RecordReadValue::Head(value) = head.value().unwrap() else {
        panic!("head")
    };
    let scope = value.scope.clone();
    let count = value.head;
    drop(head);
    let page = read
        .execute(&registration, &f.a, f.epoch, request(scope.clone(), count))
        .await
        .unwrap();
    let RecordReadValue::Page(value) = page.value().unwrap() else {
        panic!("page")
    };
    #[derive(serde::Deserialize)]
    struct WireResponse {
        payload: WirePage,
    }
    #[derive(serde::Deserialize)]
    struct WirePage {
        records: Vec<Box<serde_json::value::RawValue>>,
    }
    let encoded = nessa_protocol::product::record_read::encode_page(
        "synthetic",
        &value.request,
        nessa_sync::replication::domain::Page {
            request: value.request.clone(),
            records: value.records.clone(),
        },
    )
    .unwrap();
    let wire: WireResponse = serde_json::from_str(&encoded).unwrap();
    assert_eq!(wire.payload.records.len(), value.records.len());
    let mut ids = Vec::new();
    for (record, raw) in value.records.iter().zip(&wire.payload.records) {
        let message = record_message(
            Uuid::new_v4(),
            f.clock.unix_milliseconds(),
            &f.a.to_string(),
            scope.incarnation().as_str(),
            record.position,
            raw.get(),
        )
        .unwrap();
        let queued = QueuedRecord {
            registration: registration.id,
            conversation: f.a.clone(),
            scope: scope.clone(),
            expected_after: record.position - 1,
            position: record.position,
            epoch: f.epoch,
            body: message.body().to_vec(),
            id: message.id(),
            observed_at_ms: f.clock.unix_milliseconds(),
            initiator: QueueInitiator::Live,
        };
        let saved = store.queue(queued).await.expect(
            "actual canonical principal scope must be accepted by the current durable outbox",
        );
        ids.push(saved);
    }
    assert_eq!(ids.len(), 2);
    let due = store.due(f.clock.unix_milliseconds()).await.unwrap();
    assert_eq!(due.len(), 2);
    assert!(due
        .iter()
        .all(|r| r.scope == scope && r.registration.id == registration.id));
    assert!(due.iter().all(|r| ids.contains(&r.delivery.id())));
    #[derive(serde::Deserialize)]
    struct Envelope {
        record: Box<serde_json::value::RawValue>,
    }
    for delivery in &due {
        let envelope: Envelope = serde_json::from_slice(&delivery.body).unwrap();
        assert_eq!(
            envelope.record.get(),
            wire.payload.records[delivery.position as usize - 1].get()
        );
    }

    // This proves source/outbox compatibility and committed queueing only.
    // There is no HTTP adapter or supervised sender in this test.
    drop(page);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_before_the_grant_is_acquired_releases_auth_and_shared_capacity_without_io() {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    let (_store, registration, path) = registered(&f).await;
    let before = saved(&path);
    let source = NeverRead(AtomicUsize::new(0));
    let capacity = Arc::new(Semaphore::new(1));
    let read = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    };
    let grant = f
        .grants
        .hold_principal_revision(&f.a, &f.target.principal, f.epoch)
        .await
        .unwrap();
    let mut pending =
        Box::pin(read.execute(&registration, &f.a, f.epoch, RecordReadOperation::Head));
    assert!(matches!(
        pending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    let mut revoke = Box::pin(f.credentials.revoke(RevokeCredentialRequest {
        request_id: "cancel-source-before-grant".into(),
        issuer_principal_id: "owner".into(),
        credential_id: "hook-key".into(),
        revoked_at: 3,
    }));
    assert!(matches!(
        revoke
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert_eq!(capacity.available_permits(), 0);
    drop(pending);
    // The other caller still owns the grant. Revoke must no longer wait on
    // this cancelled source call's credential hold or try to acquire that grant.
    tokio::time::timeout(Duration::from_secs(3), revoke)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(capacity.available_permits(), 1);
    drop(grant);
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&path), before);
}
