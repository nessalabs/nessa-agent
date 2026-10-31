//! `VoiceCalls` over a memory secret, a scripted provider and a recording
//! audit: rows K1–K9 of `docs/design/voice-chat.md`.
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::secrets::{SecretStore, SecretUnavailable};
use crate::voice::application::{
    AuditUnavailable, MintFailure, RealtimeProvider, VoiceAudit, VoiceCaller, VoiceCalls,
    VoiceError, VoiceEvent, VoiceRecord,
};
use crate::voice::domain::{CallGrant, CallTransport, VoiceKey};

#[derive(Default)]
struct MemorySecret {
    value: Mutex<Option<Vec<u8>>>,
    locked: Mutex<bool>,
    writes_fail: Mutex<bool>,
    deletes_fail: Mutex<bool>,
}

impl SecretStore for MemorySecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        if *self.locked.lock().unwrap() {
            return Err(SecretUnavailable::Locked);
        }
        Ok(self.value.lock().unwrap().clone().map(Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        if *self.writes_fail.lock().unwrap() {
            return Err(SecretUnavailable::Unavailable);
        }
        *self.value.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        if *self.deletes_fail.lock().unwrap() {
            return Err(SecretUnavailable::Unavailable);
        }
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}

struct Scripted {
    answer: Mutex<Result<CallGrant, MintFailure>>,
    keys_seen: Mutex<Vec<String>>,
}

impl Scripted {
    fn answering(answer: Result<CallGrant, MintFailure>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            keys_seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl RealtimeProvider for Scripted {
    async fn mint(&self, key: &VoiceKey) -> Result<CallGrant, MintFailure> {
        self.keys_seen.lock().unwrap().push(key.expose().to_owned());
        self.answer.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Recording {
    records: Mutex<Vec<VoiceRecord>>,
    refuses: Mutex<bool>,
}

impl VoiceAudit for Recording {
    fn record(&self, record: &VoiceRecord) -> Result<(), AuditUnavailable> {
        if *self.refuses.lock().unwrap() {
            return Err(AuditUnavailable);
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
}

impl Recording {
    fn events(&self) -> Vec<VoiceEvent> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.event.clone())
            .collect()
    }
}

fn grant() -> CallGrant {
    CallGrant {
        transport: CallTransport::WebRtc,
        call_url: "https://api.openai.com/v1/realtime/calls".into(),
        client_secret: "ek_1".into(),
        expires_at_ms: 60_000,
        model: "gpt-realtime".into(),
    }
}

fn caller() -> VoiceCaller {
    VoiceCaller {
        organization_id: "org".into(),
        principal_id: "owner".into(),
        credential_id: "panel".into(),
    }
}

struct Fixture {
    secret: Arc<MemorySecret>,
    provider: Arc<Scripted>,
    audit: Arc<Recording>,
    calls: VoiceCalls,
}

fn fixture(answer: Result<CallGrant, MintFailure>) -> Fixture {
    let secret = Arc::new(MemorySecret::default());
    let provider = Scripted::answering(answer);
    let audit = Arc::new(Recording::default());
    let calls = VoiceCalls::new(secret.clone(), provider.clone(), audit.clone());
    Fixture {
        secret,
        provider,
        audit,
        calls,
    }
}

fn key(text: &str) -> VoiceKey {
    VoiceKey::parse(text.as_bytes()).unwrap()
}

const CONVERSATION: &str = "00000000-0000-4000-8000-000000000001";

#[tokio::test]
async fn k1_a_locked_vault_refuses_every_method() {
    let f = fixture(Ok(grant()));
    *f.secret.locked.lock().unwrap() = true;
    assert_eq!(f.calls.status(), Err(VoiceError::VaultLocked));
    assert_eq!(
        f.calls.save_key(caller(), key("sk-1")).await,
        Err(VoiceError::VaultLocked)
    );
    assert_eq!(
        f.calls.forget_key(caller()).await,
        Err(VoiceError::VaultLocked)
    );
    assert_eq!(
        f.calls.start_call(caller(), CONVERSATION.into()).await,
        Err(VoiceError::VaultLocked)
    );
    assert!(f.audit.events().is_empty());
}

#[tokio::test]
async fn k3_a_save_is_recorded_first_and_says_whether_it_replaced_one() {
    let f = fixture(Ok(grant()));
    assert!(!f.calls.status().unwrap().key_saved);
    assert!(
        f.calls
            .save_key(caller(), key("sk-1"))
            .await
            .unwrap()
            .key_saved
    );
    assert!(
        f.calls
            .save_key(caller(), key("sk-2"))
            .await
            .unwrap()
            .key_saved
    );
    assert_eq!(
        f.audit.events(),
        [
            VoiceEvent::KeySaveRequested { replacing: false },
            VoiceEvent::KeySaved,
            VoiceEvent::KeySaveRequested { replacing: true },
            VoiceEvent::KeySaved,
        ]
    );
    assert_eq!(
        f.secret.value.lock().unwrap().as_deref(),
        Some(&b"sk-2"[..])
    );
    // The record names who asked.
    assert_eq!(f.audit.records.lock().unwrap()[0].caller, caller());
}

#[tokio::test]
async fn k3_a_save_that_cannot_be_recorded_changes_nothing() {
    let f = fixture(Ok(grant()));
    *f.audit.refuses.lock().unwrap() = true;
    assert_eq!(
        f.calls.save_key(caller(), key("sk-1")).await,
        Err(VoiceError::AuditUnavailable)
    );
    assert!(f.secret.value.lock().unwrap().is_none());
}

#[tokio::test]
async fn k3_a_failed_write_is_recorded_and_leaves_the_old_key() {
    let f = fixture(Ok(grant()));
    f.calls.save_key(caller(), key("sk-1")).await.unwrap();
    *f.secret.writes_fail.lock().unwrap() = true;
    assert_eq!(
        f.calls.save_key(caller(), key("sk-2")).await,
        Err(VoiceError::StorageUnavailable)
    );
    assert_eq!(f.audit.events().last(), Some(&VoiceEvent::KeySaveFailed));
    assert_eq!(
        f.secret.value.lock().unwrap().as_deref(),
        Some(&b"sk-1"[..])
    );
}

#[tokio::test]
async fn k4_k5_forget_deletes_a_saved_key_and_is_quiet_without_one() {
    let f = fixture(Ok(grant()));
    assert!(!f.calls.forget_key(caller()).await.unwrap().key_saved);
    assert!(f.audit.events().is_empty());
    f.calls.save_key(caller(), key("sk-1")).await.unwrap();
    assert!(!f.calls.forget_key(caller()).await.unwrap().key_saved);
    assert_eq!(
        f.audit.events()[2..],
        [VoiceEvent::KeyForgetRequested, VoiceEvent::KeyForgotten]
    );
    assert!(f.secret.value.lock().unwrap().is_none());
}

#[tokio::test]
async fn k4_a_failed_delete_is_recorded_and_the_key_stays() {
    let f = fixture(Ok(grant()));
    f.calls.save_key(caller(), key("sk-1")).await.unwrap();
    *f.secret.deletes_fail.lock().unwrap() = true;
    assert_eq!(
        f.calls.forget_key(caller()).await,
        Err(VoiceError::StorageUnavailable)
    );
    assert_eq!(f.audit.events().last(), Some(&VoiceEvent::KeyForgetFailed));
    assert!(f.calls.status().unwrap().key_saved);
}

#[tokio::test]
async fn k6_no_key_starts_no_call() {
    let f = fixture(Ok(grant()));
    assert_eq!(
        f.calls.start_call(caller(), CONVERSATION.into()).await,
        Err(VoiceError::KeyMissing)
    );
    assert!(f.provider.keys_seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn k6_a_malformed_entry_is_no_key_and_is_never_sent() {
    let f = fixture(Ok(grant()));
    *f.secret.value.lock().unwrap() = Some(b"not a key".to_vec());
    assert!(!f.calls.status().unwrap().key_saved);
    assert_eq!(
        f.calls.start_call(caller(), CONVERSATION.into()).await,
        Err(VoiceError::KeyMissing)
    );
    assert!(f.provider.keys_seen.lock().unwrap().is_empty());
    // It can still be deleted.
    f.calls.forget_key(caller()).await.unwrap();
    assert!(f.secret.value.lock().unwrap().is_none());
}

#[tokio::test]
async fn k7_a_call_is_recorded_then_minted_with_the_saved_key() {
    let f = fixture(Ok(grant()));
    f.calls.save_key(caller(), key("sk-1")).await.unwrap();
    assert_eq!(
        f.calls.start_call(caller(), CONVERSATION.into()).await,
        Ok(grant())
    );
    assert_eq!(*f.provider.keys_seen.lock().unwrap(), ["sk-1"]);
    assert_eq!(
        f.audit.events()[2..],
        [
            VoiceEvent::CallRequested {
                conversation_id: CONVERSATION.into()
            },
            VoiceEvent::CallMinted {
                conversation_id: CONVERSATION.into()
            },
        ]
    );
    // Request and outcome are one operation.
    let records = f.audit.records.lock().unwrap();
    assert_eq!(records[2].operation_id, records[3].operation_id);
}

/// A provider that answers only once let go.
struct Gated {
    gate: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl RealtimeProvider for Gated {
    async fn mint(&self, _key: &VoiceKey) -> Result<CallGrant, MintFailure> {
        self.gate.notified().await;
        Ok(grant())
    }
}

#[tokio::test]
async fn k7_a_caller_gone_mid_mint_still_leaves_the_outcome_on_record() {
    let secret = Arc::new(MemorySecret::default());
    let provider = Arc::new(Gated {
        gate: tokio::sync::Notify::new(),
    });
    let audit = Arc::new(Recording::default());
    let calls = VoiceCalls::new(secret, provider.clone(), audit.clone());
    calls.save_key(caller(), key("sk-1")).await.unwrap();
    // The caller gives up (its socket closed) while the provider is asked.
    let gave_up = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        calls.start_call(caller(), CONVERSATION.into()),
    )
    .await;
    assert!(gave_up.is_err());
    provider.gate.notify_one();
    for _ in 0..100 {
        if audit.events().len() == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        audit.events().last(),
        Some(&VoiceEvent::CallMinted {
            conversation_id: CONVERSATION.into()
        })
    );
}

#[tokio::test]
async fn k7_a_call_that_cannot_be_recorded_is_not_minted() {
    let f = fixture(Ok(grant()));
    f.calls.save_key(caller(), key("sk-1")).await.unwrap();
    *f.audit.refuses.lock().unwrap() = true;
    assert_eq!(
        f.calls.start_call(caller(), CONVERSATION.into()).await,
        Err(VoiceError::AuditUnavailable)
    );
    assert!(f.provider.keys_seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn k8_k9_a_refused_mint_is_recorded_with_its_reason() {
    for (failure, error, reason) in [
        (
            MintFailure::Rejected,
            VoiceError::KeyRejected,
            MintFailure::Rejected,
        ),
        (
            MintFailure::Unreachable,
            VoiceError::Unreachable,
            MintFailure::Unreachable,
        ),
    ] {
        let f = fixture(Err(failure));
        f.calls.save_key(caller(), key("sk-1")).await.unwrap();
        assert_eq!(
            f.calls.start_call(caller(), CONVERSATION.into()).await,
            Err(error)
        );
        let records = f.audit.records.lock().unwrap().clone();
        let [.., requested, refused] = records.as_slice() else {
            panic!("expected a request and a refusal");
        };
        assert_eq!(requested.operation_id, refused.operation_id);
        assert_eq!(
            refused.event,
            VoiceEvent::CallRefused {
                conversation_id: CONVERSATION.into(),
                reason,
            }
        );
    }
}

#[tokio::test]
async fn k8_a_refusal_that_cannot_be_recorded_is_still_answered() {
    let f = fixture(Err(MintFailure::Rejected));
    f.calls.save_key(caller(), key("sk-1")).await.unwrap();
    // The request is kept; the refusal's record is the one that fails.
    struct RefuseSecond(Mutex<usize>);
    impl VoiceAudit for RefuseSecond {
        fn record(&self, _: &VoiceRecord) -> Result<(), AuditUnavailable> {
            let mut seen = self.0.lock().unwrap();
            *seen += 1;
            if *seen == 2 {
                Err(AuditUnavailable)
            } else {
                Ok(())
            }
        }
    }
    let calls = VoiceCalls::new(
        f.secret.clone(),
        f.provider.clone(),
        Arc::new(RefuseSecond(Mutex::new(0))),
    );
    assert_eq!(
        calls.start_call(caller(), CONVERSATION.into()).await,
        Err(VoiceError::KeyRejected)
    );
}
