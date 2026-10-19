use super::*;
use crate::conversation::infrastructure::NessaRecordWatches;
use nessa_protocol::conversation::read_scope::ReceiverReadScope;
use tempfile::TempDir;

/// A product route state for host cleanup tests: one conversation granted to
/// a paired receiver, its record storage holding one opening save, and the passive-read and change
/// sources composition binds. Nothing is admitted on it, so a host cleanup
/// test decides what is in flight.
pub(crate) struct HostProductFixture {
    state: ProductRouteState,
    storage: Arc<RecordStorage>,
    id: ConversationId,
    _directory: TempDir,
}

impl HostProductFixture {
    pub(crate) async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let metadata = Arc::new(
            LocalConversationStore::open(&private.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
        );
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        crate::conversation_test_support::grant_as(
            metadata.as_ref(),
            &id,
            "receiver",
            crate::conversation::domain::ShareAccess::read(),
        )
        .await;
        let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
        storage.initialize().await.unwrap();
        // One durable opening save, so a record head read finds the history.
        let session = SessionId::new(id.to_string()).unwrap();
        let writer = storage.open(session.clone()).await.unwrap();
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let loaded = writer.load().await.unwrap();
        writer
            .save_changes(
                loaded.binding().clone(),
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session,
                    provider,
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
        let (state, _) = fixture(MembershipRole::Member);
        let state = state
            .with_passive_read(
                Arc::new(RecordBinding),
                metadata.clone(),
                metadata.clone(),
                crate::conversation_test_support::access_policy(),
            )
            .with_change_watches(Arc::new(NessaRecordWatches::new(storage.clone())), metadata)
            .with_record_source(Arc::new(RecordAdmissionSpy(AtomicU64::new(0))));
        Self {
            state,
            storage,
            id,
            _directory: directory,
        }
    }

    pub(crate) fn state(&self) -> ProductRouteState {
        self.state.clone()
    }

    pub(crate) fn storage(&self) -> Arc<RecordStorage> {
        self.storage.clone()
    }

    pub(crate) fn scope(&self) -> ReceiverReadScope {
        ReceiverReadScope {
            receiver_id: "receiver".into(),
            organization_id: OrganizationId::new("organization").unwrap(),
            owner_id: PrincipalId::new("principal").unwrap(),
            conversation_id: self.id.clone(),
            access_epoch: 3,
        }
    }
}
