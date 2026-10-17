//! Shared real SDK writer/private cache fixtures for record and catalogue evidence.
use super::ReadOnlyCache;
use crate::read_only_sync::application::CachePolicy;
use nessa_auth::application::ports::Clock;
use nessa_sdk::{
    application::agent_execution::{
        permissions::ActionContext,
        providers::ProviderIdentity,
        sessions::{
            ArtifactName, ArtifactRecord, CurrentLease, LeaseRecord, PublishedFile, SessionChange,
            SessionSaveUnit, SessionSnapshot, SessionStorage,
        },
    },
    domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseGrants, LeaseId, LeaseRevision, LeaseTerms,
        LeaseWork, SandboxProfile,
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    domain::common::value_objects::{MediaType, Sha256Digest},
    infrastructure::session_storage::{
        physical_record_schema, RecordReadStatus, RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    },
};
use nessa_sync::replication::{
    domain::{validate_page, Checkpoint, CommitPlan, Id, Limits, Page, PageRequest, Record, Scope},
    infrastructure::MAX_PAGE_PAYLOAD,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) struct FixedClock;
impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        123_000
    }
}

pub(super) fn id(text: &str) -> Id {
    Id::new(text).unwrap()
}
pub(super) fn scope() -> Scope {
    Scope::new(
        id("receiver"),
        id("origin"),
        id("00000000-0000-0000-0000-000000000001"),
        id("incarnation"),
        physical_record_schema(),
        id("epoch"),
    )
}
pub(super) fn policy() -> CachePolicy {
    CachePolicy::new(
        32 * 1024 * 1024,
        8 * 1024 * 1024,
        Limits::new(16, MAX_PAGE_PAYLOAD, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES).unwrap(),
    )
    .unwrap()
}
pub(super) fn cache(path: &Path) -> ReadOnlyCache {
    ReadOnlyCache::open(path, policy(), Arc::new(FixedClock)).unwrap()
}

pub(super) fn cache_path(root: &Path, name: &str) -> PathBuf {
    let private = root.join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    private.join(name)
}
pub(super) fn plan(scope: &Scope, after: u64, records: Vec<Record>) -> CommitPlan {
    let target = records.last().unwrap().position;
    let limits = policy().suffix_page();
    let request = PageRequest {
        scope: scope.clone(),
        after,
        target,
        max_records: limits.max_records(),
        max_payload_bytes: limits.max_payload_bytes(),
        max_record_bytes: limits.max_record_bytes(),
    };
    validate_page(
        &Checkpoint::new(scope.clone(), after),
        &request,
        Page {
            request: request.clone(),
            records,
        },
        limits,
    )
    .unwrap()
}

/// The real record writer owns encoding and the real source owns frame reads.
pub(super) async fn source_records(root: &Path) -> (Scope, Vec<Record>) {
    record_fixture(root, false, 400).await
}

pub(super) async fn source_records_with_history(root: &Path) -> (Scope, Vec<Record>) {
    record_fixture(root, true, 400).await
}

pub(super) async fn source_long_pending_records(root: &Path) -> (Scope, Vec<Record>) {
    record_fixture(root, true, 10_000).await
}

async fn record_fixture(root: &Path, separate_open: bool, count: usize) -> (Scope, Vec<Record>) {
    let storage = RecordStorage::new(root.join("source")).unwrap();
    storage.initialize().await.unwrap();
    let session = SessionId::new("00000000-0000-0000-0000-000000000001").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
    let mut context = ProviderContext::Absent;
    let mut changes = vec![SessionChange::Opened {
        id: session.clone(),
        provider: provider.clone(),
        context: context.clone(),
    }];
    if separate_open {
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                SessionSnapshot {
                    model_change: None,
                    current_configuration: None,
                    id: session.clone(),
                    provider: provider.clone(),
                    provider_context: context.clone(),
                    invocations: vec![],
                    queue_history: vec![],
                    lease: None,
                    artifacts: Vec::new(),
                    notes: Vec::new(),
                },
                vec![SessionSaveUnit::new(std::mem::take(&mut changes)).unwrap()],
            )
            .await
            .unwrap();
    }
    // One valid atomic decision group spans several physical pieces.
    for index in 0..count {
        let next = ProviderContext::Recorded(
            ExecutionSessionId::new(format!("{index:04}{}", "x".repeat(240))).unwrap(),
        );
        changes.push(SessionChange::ProviderContext {
            before: context,
            after: next.clone(),
        });
        context = next;
    }
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            SessionSnapshot {
                model_change: None,
                current_configuration: None,
                id: session.clone(),
                provider,
                provider_context: context,
                invocations: vec![],
                queue_history: vec![],
                lease: None,
                artifacts: Vec::new(),
                notes: Vec::new(),
            },
            vec![SessionSaveUnit::new(changes).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    read_back(storage, session).await
}

/// The digest of the one artifact `source_records_with_artifact` records.
pub(super) const ARTIFACT_DIGEST: &str =
    "sha256:00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff";

/// A conversation whose agent published one file under its lease, recorded
/// as the gateway records it: opened, the lease issued, the artifact kept.
pub(super) async fn source_records_with_artifact(root: &Path) -> (Scope, Vec<Record>) {
    let storage = RecordStorage::new(root.join("source")).unwrap();
    storage.initialize().await.unwrap();
    let session = SessionId::new("00000000-0000-0000-0000-000000000001").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
    let actor = ActionContext::new("person", "desktop", "send").unwrap();
    let issued = LeaseRecord::Issued {
        lease: LeaseId::new("lease-1").unwrap(),
        revision: LeaseRevision::FIRST,
        terms: LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        },
        actor: actor.clone(),
    };
    let artifact = ArtifactRecord {
        lease: LeaseId::new("lease-1").unwrap(),
        turn: None,
        name: ArtifactName::new("report.txt").unwrap(),
        file: PublishedFile::new(
            Sha256Digest::parse(ARTIFACT_DIGEST).unwrap(),
            MediaType::parse("text/plain").unwrap(),
            12,
        )
        .unwrap(),
        actor,
    };
    let changes = vec![
        SessionChange::Opened {
            id: session.clone(),
            provider: provider.clone(),
            context: ProviderContext::Absent,
        },
        SessionChange::Lease(issued.clone()),
        SessionChange::Artifact(artifact.clone()),
    ];
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            SessionSnapshot {
                model_change: None,
                current_configuration: None,
                id: session.clone(),
                provider,
                provider_context: ProviderContext::Absent,
                invocations: vec![],
                queue_history: vec![],
                lease: Some(CurrentLease::apply(None, &issued).unwrap()),
                artifacts: vec![artifact],
                notes: Vec::new(),
            },
            vec![SessionSaveUnit::new(changes).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    read_back(storage, session).await
}

/// Every record `session` has, read from the real source as the gateway
/// serves them.
async fn read_back(storage: RecordStorage, session: SessionId) -> (Scope, Vec<Record>) {
    let mut source = storage
        .record_source(&session, id("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(id("receiver"), id("epoch"));
    let returned = tokio::task::spawn_blocking(move || {
        let target = loop {
            if let RecordReadStatus::Ready(head) = source.bounded_head(&scope).unwrap() {
                break head;
            }
        };
        let mut records = vec![];
        let mut after = 0;
        while after < target {
            let limits = policy().suffix_page();
            let request = PageRequest {
                scope: scope.clone(),
                after,
                target,
                max_records: limits.max_records(),
                max_payload_bytes: limits.max_payload_bytes(),
                max_record_bytes: limits.max_record_bytes(),
            };
            let RecordReadStatus::Ready(page) = source.bounded_page(&request).unwrap() else {
                panic!("head validated the captured prefix");
            };
            after = page.records.last().unwrap().position;
            records.extend(page.records);
        }
        drop(source);
        (scope, records)
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
    returned
}
