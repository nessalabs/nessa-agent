//! Memory-adapter ownership observed through public storage leases and receipts.

use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            ProviderContext, SessionChange, SessionLoad, SessionLoadState, SessionSaveUnit,
            SessionSnapshot, SessionStorage, StorageError,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, SessionId},
    infrastructure::session_storage::InMemoryStorage,
};

fn opening(id: &SessionId, name: &str) -> (SessionSaveUnit, SessionSnapshot) {
    let provider = ProviderIdentity::new(name, "model", "workspace").unwrap();
    let change = SessionChange::Opened {
        id: id.clone(),
        provider: provider.clone(),
        context: ProviderContext::Absent,
    };
    let snapshot = SessionSnapshot {
        id: id.clone(),
        provider,
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
        lease: None,
        artifacts: Vec::new(),
        notes: Vec::new(),
    };
    (SessionSaveUnit::new(vec![change]).unwrap(), snapshot)
}

fn context(prior: &SessionSnapshot, name: &str) -> (SessionSaveUnit, SessionSnapshot) {
    let context = ProviderContext::Recorded(ExecutionSessionId::new(name).unwrap());
    let change = SessionChange::ProviderContext {
        before: prior.provider_context.clone(),
        after: context.clone(),
    };
    let snapshot = SessionSnapshot {
        provider_context: context,
        ..prior.clone()
    };
    (SessionSaveUnit::new(vec![change]).unwrap(), snapshot)
}

#[tokio::test]
async fn actual_memory_binding_prefix_receipt_and_reset_keep_original_ownership() {
    let storage = InMemoryStorage::new();
    let id = SessionId::new("memory-owner").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let other = InMemoryStorage::new();
    let foreign = other.open(id.clone()).await.unwrap().load().await.unwrap();
    let (first, snapshot) = opening(&id, "provider");
    assert!(matches!(
        lease
            .save_changes(foreign.binding().clone(), vec![first.clone()])
            .await,
        Err(StorageError::IdentityMismatch)
    ));
    assert!(lease.load().await.unwrap().snapshot().is_none());
    let (wrong_session, _) = opening(&SessionId::new("another").unwrap(), "provider");
    assert!(matches!(
        lease
            .save_changes(original.clone(), vec![wrong_session])
            .await,
        Err(StorageError::IdentityMismatch)
    ));
    assert!(lease.load().await.unwrap().snapshot().is_none());
    let receipt = lease
        .save_changes(original.clone(), vec![first.clone()])
        .await
        .unwrap();
    assert_eq!(
        lease
            .save_changes(original.clone(), vec![first.clone()])
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
    let (changed_first, _) = opening(&id, "changed-provider");
    assert!(matches!(
        lease
            .save_changes(original.clone(), vec![changed_first])
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
    let (suffix, extended) = context(&snapshot, "extended-context");
    let repartitioned = SessionSaveUnit::new(
        first
            .changes()
            .iter()
            .chain(suffix.changes())
            .cloned()
            .collect(),
    )
    .unwrap();
    assert!(matches!(
        lease
            .save_changes(original.clone(), vec![repartitioned])
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
    let extension = lease
        .save_changes(original.clone(), vec![first.clone(), suffix])
        .await
        .unwrap();
    assert_eq!(extension.units(), 2);
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
    let (later, _) = context(&extended, "later-context");
    assert!(matches!(
        lease
            .save_changes(receipt.next().clone(), vec![later.clone()])
            .await,
        Err(StorageError::IdentityMismatch)
    ));
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
    let next_receipt = lease
        .save_changes(extension.next().clone(), vec![later.clone()])
        .await
        .unwrap();
    assert_eq!(
        lease
            .save_changes(extension.next().clone(), vec![later])
            .await
            .unwrap(),
        next_receipt
    );
    lease.erase().await.unwrap();
    let reset = lease.load().await.unwrap();
    assert!(reset.snapshot().is_none());
    assert_ne!(reset.binding().backend(), original.backend());
    let result = lease.save_changes(original, vec![first.clone()]).await;
    assert!(
        lease.load().await.unwrap().snapshot().is_none(),
        "old retry cannot replace reset state"
    );
    assert!(matches!(result, Err(StorageError::IdentityMismatch)));
    lease
        .save_changes(reset.binding().clone(), vec![first])
        .await
        .unwrap();
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
}

#[tokio::test]
async fn actual_memory_plan_preflight_refuses_before_replacing_published_evidence() {
    let storage = InMemoryStorage::new();
    let id = SessionId::new("memory-preflight").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let (first, snapshot) = opening(&id, "provider");
    let (duplicate, _) = opening(&id, "provider");
    let result = lease
        .save_changes(original.clone(), vec![first.clone(), duplicate])
        .await;
    assert!(
        lease.load().await.unwrap().snapshot().is_none(),
        "invalid later unit cannot publish valid prefix"
    );
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    let receipt = lease.save_changes(original, vec![first]).await.unwrap();
    let result = lease.save_changes(receipt.next().clone(), Vec::new()).await;
    assert_eq!(
        lease.load().await.unwrap().binding(),
        receipt.next(),
        "empty plan cannot advance revision before a refused receipt"
    );
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    let (suffix, extended) = context(&snapshot, "valid-context");
    lease
        .save_changes(receipt.next().clone(), vec![suffix])
        .await
        .unwrap();
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
}

#[tokio::test]
async fn reloaded_snapshot_refuses_a_save_binding_from_another_conversation() {
    for populated in [false, true] {
        let storage = InMemoryStorage::new();
        let a = SessionId::new("conversation-a").unwrap();
        let b = SessionId::new("conversation-b").unwrap();
        let lease_a = storage.open(a.clone()).await.unwrap();
        let lease_b = storage.open(b.clone()).await.unwrap();
        if populated {
            for (id, lease) in [(&a, &lease_a), (&b, &lease_b)] {
                let (unit, _) = opening(id, "provider");
                lease
                    .save_changes(lease.load().await.unwrap().binding().clone(), vec![unit])
                    .await
                    .unwrap();
            }
        }
        let own = lease_a.load().await.unwrap();
        let foreign = lease_b.load().await.unwrap();
        let (saved, binding) = own.clone().into_published(&a).unwrap();
        assert_eq!(saved.as_ref(), own.snapshot());
        assert_eq!(&binding, own.binding());
        let mixed = SessionLoad::new(
            own.snapshot().cloned(),
            foreign.binding().clone(),
            SessionLoadState::Published,
        );
        assert!(matches!(
            mixed.into_published(&a),
            Err(StorageError::IdentityMismatch)
        ));
        assert_eq!(lease_a.load().await.unwrap().snapshot(), own.snapshot());
        assert_eq!(lease_b.load().await.unwrap().snapshot(), foreign.snapshot());
    }
}
