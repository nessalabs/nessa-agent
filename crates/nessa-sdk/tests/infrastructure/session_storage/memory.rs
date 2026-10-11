//! Memory-adapter ownership observed through public storage leases and receipts.

use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            ProviderContext, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
            StorageError,
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
            .save_changes(
                foreign.binding().clone(),
                snapshot.clone(),
                vec![first.clone()]
            )
            .await,
        Err(StorageError::IdentityMismatch)
    ));
    assert!(lease.load().await.unwrap().snapshot().is_none());
    let (wrong_session, wrong_snapshot) = opening(&SessionId::new("another").unwrap(), "provider");
    assert!(matches!(
        lease
            .save_changes(original.clone(), wrong_snapshot, vec![wrong_session])
            .await,
        Err(StorageError::IdentityMismatch)
    ));
    assert!(lease.load().await.unwrap().snapshot().is_none());
    let receipt = lease
        .save_changes(original.clone(), snapshot.clone(), vec![first.clone()])
        .await
        .unwrap();
    assert_eq!(
        lease
            .save_changes(original.clone(), snapshot.clone(), vec![first.clone()])
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
    let (changed_first, changed_snapshot) = opening(&id, "changed-provider");
    assert!(matches!(
        lease
            .save_changes(original.clone(), changed_snapshot, vec![changed_first])
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
            .save_changes(original.clone(), extended.clone(), vec![repartitioned])
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
    let extension = lease
        .save_changes(
            original.clone(),
            extended.clone(),
            vec![first.clone(), suffix],
        )
        .await
        .unwrap();
    assert_eq!(extension.units(), 2);
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
    let (later, latest) = context(&extended, "later-context");
    assert!(matches!(
        lease
            .save_changes(receipt.next().clone(), latest.clone(), vec![later.clone()])
            .await,
        Err(StorageError::IdentityMismatch)
    ));
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
    let next_receipt = lease
        .save_changes(
            extension.next().clone(),
            latest.clone(),
            vec![later.clone()],
        )
        .await
        .unwrap();
    assert_eq!(
        lease
            .save_changes(extension.next().clone(), latest, vec![later])
            .await
            .unwrap(),
        next_receipt
    );
    lease.erase().await.unwrap();
    let reset = lease.load().await.unwrap();
    assert!(reset.snapshot().is_none());
    assert_ne!(reset.binding().backend(), original.backend());
    let result = lease
        .save_changes(original, snapshot.clone(), vec![first.clone()])
        .await;
    assert!(
        lease.load().await.unwrap().snapshot().is_none(),
        "old retry cannot replace reset state"
    );
    assert!(matches!(result, Err(StorageError::IdentityMismatch)));
    lease
        .save_changes(reset.binding().clone(), snapshot.clone(), vec![first])
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
        .save_changes(
            original.clone(),
            snapshot.clone(),
            vec![first.clone(), duplicate],
        )
        .await;
    assert!(
        lease.load().await.unwrap().snapshot().is_none(),
        "invalid later unit cannot publish valid prefix"
    );
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    let (_, changed_candidate) = opening(&id, "changed-provider");
    let result = lease
        .save_changes(original.clone(), changed_candidate, vec![first.clone()])
        .await;
    assert!(
        lease.load().await.unwrap().snapshot().is_none(),
        "contradictory candidate cannot publish"
    );
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    let receipt = lease
        .save_changes(original, snapshot.clone(), vec![first])
        .await
        .unwrap();
    let result = lease
        .save_changes(receipt.next().clone(), snapshot.clone(), Vec::new())
        .await;
    assert_eq!(
        lease.load().await.unwrap().binding(),
        receipt.next(),
        "empty plan cannot advance revision before a refused receipt"
    );
    assert!(matches!(result, Err(StorageError::Corrupt(_))));
    let (suffix, extended) = context(&snapshot, "valid-context");
    lease
        .save_changes(receipt.next().clone(), extended.clone(), vec![suffix])
        .await
        .unwrap();
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
}
