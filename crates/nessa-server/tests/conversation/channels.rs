//! A channel is made once per request id, in a folder the person chose or
//! one made for it; a conversation created in it starts with its defaults
//! and its agent is built in its folder; nobody else can use it.
use super::*;
use crate::conversation::application::{
    ConversationAgent, ConversationAgentFuture, ConversationAgentSource, ConversationAgents,
    ConversationDependencies, ConversationLimits, ProviderSessionErasers, RequestedConversation,
    SubmissionMode, SubmittedMessage,
};
use crate::conversation::domain::{
    ChannelDefaults, ChannelName, ChannelNotes, ChannelRefusal, ProviderSessionErasure,
};
use crate::conversation::infrastructure::{LocalChannelStore, LocalConversationStore};
#[cfg(unix)]
use crate::conversation_test_support::channel_lookup::{ChannelLookup, ChannelRead, Membership};
use crate::conversation_test_support::{absolute, unrooted};
use crate::conversation_test_support::{
    AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_sdk::application::agent_execution::providers::ApprovalMode;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::collections::HashSet;
use std::sync::Mutex;

const CODEX_MODEL: &str = "gpt-5.6-terra";

/// What each resolution asked for, and where.
type Asked = (AgentId, String, ConversationApprovalMode, Option<PathBuf>);

/// Answers every agent with one fake provider on the model asked for, and
/// records each ask, with the place a channel's agent was asked to work in.
struct RecordingSource {
    factory: Arc<ProviderFactory>,
    asked: Mutex<Vec<Asked>>,
}

impl RecordingSource {
    fn agent(&self, model: &str, mode: ConversationApprovalMode) -> ConversationAgent {
        let mode = match mode {
            ConversationApprovalMode::Ask => ApprovalMode::Ask,
            ConversationApprovalMode::Auto => ApprovalMode::Auto,
            ConversationApprovalMode::Full => ApprovalMode::Full,
        };
        ConversationAgent {
            workspace: None,
            provider: Arc::new(
                Provider::new(self.factory.clone())
                    .with_model(model)
                    .with_mode(mode),
            ),
            execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
            reserved_output_tokens: 4096,
            readiness: None,
            sandbox: nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
        }
    }

    fn record(
        &self,
        agent: AgentId,
        model: &str,
        mode: ConversationApprovalMode,
        place: Option<&AgentPlace>,
    ) -> ConversationAgentFuture<'_> {
        self.asked.lock().unwrap().push((
            agent,
            model.to_owned(),
            mode,
            place.map(|place| place.folder.clone()),
        ));
        let configured = self.agent(model, mode);
        Box::pin(async move { Ok(Some(configured)) })
    }
}

impl ConversationAgentSource for RecordingSource {
    fn resolve(&self, _agent: AgentId) -> ConversationAgentFuture<'_> {
        let configured = self.agent("claude-default", ConversationApprovalMode::Ask);
        Box::pin(async move { Ok(Some(configured)) })
    }
    fn resolve_for<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationAgentFuture<'a> {
        self.record(agent, model, mode, None)
    }
    fn resolve_in<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
        place: &'a AgentPlace,
    ) -> ConversationAgentFuture<'a> {
        assert!(
            place.instructions.contains("Its folder is"),
            "{}",
            place.instructions
        );
        self.record(agent, model, mode, Some(place))
    }
}

/// Folders as a fixed table says: `/missing` is not there, `/file` is a
/// file, `/refused` is refused, and a made folder is under `/data/channels`.
/// A path named in `gone` was removed since it was attached.
#[derive(Clone, Default)]
struct TableFolders {
    gone: Arc<Mutex<Vec<String>>>,
}

impl TableFolders {
    fn is_gone(&self, path: &ChannelPath) -> bool {
        self.gone
            .lock()
            .unwrap()
            .iter()
            .any(|gone| gone == path.as_str())
    }
}

impl ChannelFolders for TableFolders {
    fn accept_folder(&self, chosen: &ChannelPath) -> Result<ChannelPath, FolderRefusal> {
        if self.is_gone(chosen) {
            return Err(FolderRefusal::NotFound);
        }
        match unrooted(chosen.as_str()) {
            "/missing" => Err(FolderRefusal::NotFound),
            "/file" => Err(FolderRefusal::NotADirectory),
            "/refused" => Err(FolderRefusal::Refused),
            // A link, kept as where it leads.
            "/link" => Ok(ChannelPath::new(PathBuf::from(absolute("/work/repo"))).unwrap()),
            _ => Ok(chosen.clone()),
        }
    }
    fn accept_resource(&self, path: &ChannelPath) -> Result<ChannelPath, FolderRefusal> {
        if self.is_gone(path) {
            return Err(FolderRefusal::NotFound);
        }
        match unrooted(path.as_str()) {
            "/missing" => Err(FolderRefusal::NotFound),
            _ => Ok(path.clone()),
        }
    }
    fn make(&self, id: &ChannelId) -> Result<ChannelPath, FolderRefusal> {
        Ok(ChannelPath::new(PathBuf::from(absolute(&format!("/data/channels/{id}")))).unwrap())
    }
}

struct Fixture {
    service: ConversationService,
    source: Arc<RecordingSource>,
    folders: TableFolders,
    _directory: tempfile::TempDir,
}

fn caller(who: &str, action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new(who).unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

fn new_id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}

fn path(text: &str) -> ChannelPath {
    ChannelPath::new(PathBuf::from(text)).unwrap()
}

fn request(folder: Option<&str>) -> ChannelRequest {
    ChannelRequest {
        execution: crate::conversation::domain::ChannelExecution::default(),
        name: ChannelName::new("design-review").unwrap(),
        folder: folder.map(path),
        defaults: ChannelDefaults {
            agent: AgentId::Codex,
            model: ConversationModelId::new(CODEX_MODEL).unwrap(),
            mode: ConversationApprovalMode::Auto,
        },
        notes: ChannelNotes::new("Check every export.").unwrap(),
        resources: ChannelResources::new(vec![path(&absolute("/work/brand-kit"))]).unwrap(),
    }
}

impl Fixture {
    fn new() -> Self {
        Self::with_erasers(ProviderSessionErasers::default())
    }

    fn with_erasers(provider_sessions: ProviderSessionErasers) -> Self {
        Self::with_channel_store(provider_sessions, None)
    }

    fn with_channel_store(
        provider_sessions: ProviderSessionErasers,
        channel_store: Option<Arc<dyn ChannelStore>>,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = &directory.path().join("gateway");
        nessa_local_storage::create_directory(root).unwrap();
        let storage = Arc::new(RecordStorage::new(root.join("records")).unwrap());
        let metadata = Arc::new(
            LocalConversationStore::open(&root.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
        );
        let source = Arc::new(RecordingSource {
            factory: Arc::new(ProviderFactory::default()),
            asked: Mutex::new(Vec::new()),
        });
        let agents = ConversationAgents::from_source(
            HashSet::from([AgentId::Claude, AgentId::Codex]),
            AgentId::Claude,
            source.clone(),
        )
        .unwrap();
        let service = ConversationService::new(
            ConversationDependencies {
                input_files: None,
                access_policy: crate::conversation_test_support::access_policy(),
                agents,
                storage: storage.clone(),
                metadata: metadata.clone(),
                mode_audit: Arc::new(AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: metadata.clone(),
                listing: metadata.clone(),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions,
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                clock: Arc::new(TestClock),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits::default(),
            Some(absolute("/work/default")),
        )
        .unwrap();
        service.bind_commands(storage);
        let folders = TableFolders::default();
        service.bind_channels(
            channel_store.unwrap_or_else(|| {
                Arc::new(LocalChannelStore::open(&root.join("channels.sqlite3")).unwrap())
            }),
            Arc::new(folders.clone()),
        );
        Self {
            service,
            source,
            folders,
            _directory: directory,
        }
    }

    async fn channel(&self, request_id: &str, folder: Option<&str>) -> Channel {
        self.service
            .create_channel(
                caller("person", request_id),
                request_id.into(),
                request(folder),
            )
            .await
            .unwrap()
    }

    fn asked(&self) -> Vec<Asked> {
        std::mem::take(&mut self.source.asked.lock().unwrap())
    }
}

#[cfg(unix)]
#[tokio::test]
async fn settings_authorization_refuses_another_channels_valid_owner_facts() {
    let real = Fixture::new();
    let found = real.channel("owned-channel", None).await;
    let id = found.id().clone();
    let other = ChannelId::from_random([99; 10]);
    assert_ne!(id, other);
    let fixture = Fixture::with_channel_store(
        ProviderSessionErasers::default(),
        Some(Arc::new(ChannelLookup {
            membership: Membership::Default,
            channel: ChannelRead::Found(Box::new(found)),
        })),
    );
    assert!(fixture
        .service
        .authorize_channel_settings(&caller("person", "settings"), &id)
        .await
        .is_ok());
    assert!(matches!(
        fixture
            .service
            .authorize_channel_settings(&caller("person", "settings"), &other)
            .await,
        Err(ConversationError::Unavailable)
    ));
}

#[test]
fn a_channel_id_is_the_same_for_a_retry_and_differs_by_owner() {
    let one = channel_id_for(&caller("person", "a"), "request-1");
    assert_eq!(one, channel_id_for(&caller("person", "b"), "request-1"));
    assert_ne!(one, channel_id_for(&caller("other", "a"), "request-1"));
    assert_ne!(one, channel_id_for(&caller("person", "a"), "request-2"));
    assert_eq!(ChannelId::new(one.as_str()), Ok(one));
    assert_eq!(ChannelId::new("ch_"), Err(ChannelRefusal::Id));
}

#[tokio::test]
async fn a_channel_with_no_folder_gets_one_made_and_a_retry_answers_it() {
    let fixture = Fixture::new();
    let made = fixture.channel("request-1", None).await;
    assert!(made.folder().made);
    assert_eq!(
        made.folder().path.as_str(),
        absolute(&format!("/data/channels/{}", made.id()))
    );
    assert_eq!(fixture.channel("request-1", None).await, made);
    let mut other = request(None);
    other.name = ChannelName::new("renamed").unwrap();
    assert_eq!(
        fixture
            .service
            .create_channel(caller("person", "again"), "request-1".into(), other)
            .await,
        Err(ChannelError::RequestConflict)
    );
    let listing = fixture
        .service
        .list_channels(&caller("person", "list"))
        .await
        .unwrap();
    assert_eq!(
        listing.default_folder.as_deref(),
        Some(absolute("/work/default").as_str())
    );
    assert_eq!(listing.channels, vec![made]);
}

#[tokio::test]
async fn a_chosen_folder_is_kept_resolved_and_a_refused_one_says_why() {
    let fixture = Fixture::new();
    let linked = fixture
        .channel("request-1", Some(absolute("/link").as_str()))
        .await;
    assert!(!linked.folder().made);
    assert_eq!(linked.folder().path.as_str(), absolute("/work/repo"));
    // A retry naming the same place another way is the same request.
    assert_eq!(
        fixture
            .channel("request-1", Some(absolute("/work/repo").as_str()))
            .await,
        linked
    );
    // Two channels may share a folder.
    let shared = fixture
        .channel("request-2", Some(absolute("/work/repo").as_str()))
        .await;
    assert_ne!(shared.id(), linked.id());
    for (folder, refusal) in [
        (absolute("/missing"), FolderRefusal::NotFound),
        (absolute("/file"), FolderRefusal::NotADirectory),
        (absolute("/refused"), FolderRefusal::Refused),
    ] {
        assert_eq!(
            fixture
                .service
                .create_channel(
                    caller("person", &folder),
                    folder.clone(),
                    request(Some(&folder))
                )
                .await,
            Err(ChannelError::Folder(refusal)),
            "{folder}"
        );
    }
    let mut missing = request(None);
    missing.resources = ChannelResources::new(vec![path(&absolute("/missing"))]).unwrap();
    assert_eq!(
        fixture
            .service
            .create_channel(caller("person", "r"), "resources".into(), missing)
            .await,
        Err(ChannelError::ResourceNotFound)
    );
}

#[tokio::test]
async fn a_channel_is_only_for_an_agent_this_gateway_has() {
    let fixture = Fixture::new();
    let mut opencode = request(None);
    opencode.defaults.agent = AgentId::Opencode;
    assert_eq!(
        fixture
            .service
            .create_channel(caller("person", "o"), "opencode".into(), opencode)
            .await,
        Err(ChannelError::InvalidInput)
    );
}

#[tokio::test]
async fn a_conversation_in_a_channel_starts_with_its_defaults_in_its_folder() {
    let fixture = Fixture::new();
    let channel = fixture
        .channel("request-1", Some(absolute("/work/repo").as_str()))
        .await;
    fixture.asked();
    let id = new_id();
    fixture
        .service
        .create(
            id.clone(),
            caller("person", "create"),
            RequestedConversation {
                channel: Some(channel.id().clone()),
                ..RequestedConversation::default()
            },
        )
        .await
        .unwrap();
    // Asked when it is created and again when it is opened: both times in
    // the channel's folder.
    let asked = fixture.asked();
    assert!(!asked.is_empty());
    for each in asked {
        assert_eq!(
            each,
            (
                AgentId::Codex,
                CODEX_MODEL.to_owned(),
                ConversationApprovalMode::Auto,
                Some(PathBuf::from(absolute("/work/repo")))
            )
        );
    }
    assert_eq!(
        fixture.service.channel_id_of(&id).await.unwrap(),
        Some(channel.id().clone())
    );
    // A conversation is listed once something was said in it.
    fixture
        .service
        .submit(
            id.clone(),
            caller("person", "say"),
            "say".to_owned(),
            SubmittedMessage {
                uploads: Vec::new(),
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let entry = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let list = fixture
                .service
                .list(caller("person", "list"), false)
                .await
                .unwrap();
            if let Some(entry) = list
                .conversations
                .into_iter()
                .find(|entry| entry.conversation_id == id.to_string())
            {
                return entry;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the conversation is listed");
    assert_eq!(entry.channel_id.as_deref(), Some(channel.id().as_str()));
    // What the caller names wins over the channel's defaults; another agent
    // does not take the channel's model, which is its own agent's.
    let other = new_id();
    fixture
        .service
        .create(
            other,
            caller("person", "create-2"),
            RequestedConversation {
                agent: Some(crate::conversation::application::RequestedAgent::Known(
                    AgentId::Claude,
                )),
                approval_mode: Some(ConversationApprovalMode::Ask),
                channel: Some(channel.id().clone()),
                ..RequestedConversation::default()
            },
        )
        .await
        .unwrap();
    let asked = fixture.asked();
    assert!(!asked.is_empty());
    for each in asked {
        assert_eq!(
            each,
            (
                AgentId::Claude,
                "claude-default".to_owned(),
                ConversationApprovalMode::Ask,
                Some(PathBuf::from(absolute("/work/repo")))
            )
        );
    }
}

#[tokio::test]
async fn a_conversation_in_no_channel_is_in_the_default_one() {
    let fixture = Fixture::new();
    let id = new_id();
    fixture
        .service
        .create(
            id.clone(),
            caller("person", "create"),
            RequestedConversation {
                channel: Some(ChannelId::default_channel()),
                ..RequestedConversation::default()
            },
        )
        .await
        .unwrap();
    assert!(fixture.asked().iter().all(|asked| asked.3.is_none()));
    assert_eq!(fixture.service.channel_id_of(&id).await.unwrap(), None);
}

#[tokio::test]
async fn nobody_else_can_start_a_conversation_in_a_channel() {
    let fixture = Fixture::new();
    let channel = fixture.channel("request-1", None).await;
    for (who, channel) in [
        ("other", channel.id().clone()),
        ("person", ChannelId::from_random([9; 10])),
    ] {
        let refused = fixture
            .service
            .create(
                new_id(),
                caller(who, "create"),
                RequestedConversation {
                    channel: Some(channel),
                    ..RequestedConversation::default()
                },
            )
            .await;
        assert!(
            matches!(refused, Err(ConversationError::ChannelNotFound)),
            "{who}: {refused:?}"
        );
    }
    // A channel's folder is on this machine: it runs nowhere else.
    assert!(matches!(
        fixture
            .service
            .create(
                new_id(),
                caller("person", "create"),
                RequestedConversation {
                    channel: Some(channel.id().clone()),
                    environment: Some("builder".into()),
                    ..RequestedConversation::default()
                },
            )
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(fixture
        .service
        .list_channels(&caller("other", "list"))
        .await
        .unwrap()
        .channels
        .is_empty());
}

#[tokio::test]
async fn a_creation_that_names_no_preset_is_judged_by_its_channels() {
    // What a check of the creation (an owner phone's) is told it starts in
    // is the channel's preset, the one the conversation then gets.
    let fixture = Fixture::new();
    let channel = fixture.channel("request-1", None).await;
    let asks = |who: &'static str, channel: Option<ChannelId>| {
        let service = &fixture.service;
        async move {
            service
                .creation_default_mode(&caller(who, "create"), channel.as_ref())
                .await
        }
    };
    assert_eq!(
        asks("person", Some(channel.id().clone())).await.unwrap(),
        ConversationApprovalMode::Auto
    );
    assert_eq!(
        asks("person", None).await.unwrap(),
        ConversationApprovalMode::Ask
    );
    assert_eq!(
        asks("person", Some(ChannelId::default_channel()))
            .await
            .unwrap(),
        ConversationApprovalMode::Ask
    );
    assert!(matches!(
        asks("other", Some(channel.id().clone())).await,
        Err(ConversationError::ChannelNotFound)
    ));
}

#[tokio::test]
async fn a_thread_an_agent_starts_keeps_its_conversations_preset_not_the_channels() {
    let fixture = Fixture::new();
    let channel = fixture
        .channel("request-1", Some(absolute("/work/repo").as_str()))
        .await;
    let id = new_id();
    fixture
        .service
        .create(
            id.clone(),
            caller("person", "create"),
            RequestedConversation {
                channel: Some(channel.id().clone()),
                ..RequestedConversation::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.service.thread_start(&id, false).await.unwrap(),
        (
            Some(channel.id().clone()),
            Some(ConversationApprovalMode::Auto)
        )
    );
    // The person turned it down to Ask: its threads start in Ask, though the
    // channel's default is Auto. The agent is built again in the folder.
    fixture.asked();
    fixture
        .service
        .set_approval_mode(
            id.clone(),
            caller("person", "ask"),
            ConversationApprovalMode::Ask,
        )
        .await
        .unwrap();
    assert!(fixture
        .asked()
        .iter()
        .all(|asked| asked.3.as_deref() == Some(std::path::Path::new(&absolute("/work/repo")))));
    assert_eq!(
        fixture.service.thread_start(&id, false).await.unwrap(),
        (
            Some(channel.id().clone()),
            Some(ConversationApprovalMode::Ask)
        )
    );
    // On another host it is in no channel, and names no preset.
    assert_eq!(
        fixture.service.thread_start(&id, true).await.unwrap(),
        (None, None)
    );
    let outside = new_id();
    fixture
        .service
        .create(
            outside.clone(),
            caller("person", "plain"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.service.thread_start(&outside, false).await.unwrap(),
        (None, None)
    );
}

#[tokio::test]
async fn a_retry_is_answered_with_the_kept_channel_after_what_it_named_is_gone() {
    // The answer to a create was lost, and before the retry the attached
    // folder and the chosen one were removed: the channel was still made,
    // and the retry is told so rather than asked to change the request.
    let fixture = Fixture::new();
    let made = fixture
        .channel("request-1", Some(absolute("/work/repo").as_str()))
        .await;
    fixture.folders.gone.lock().unwrap().extend([
        absolute("/work/brand-kit").to_owned(),
        absolute("/work/repo").to_owned(),
    ]);
    assert_eq!(
        fixture
            .channel("request-1", Some(absolute("/work/repo").as_str()))
            .await,
        made
    );
    // A new request naming them is refused.
    assert_eq!(
        fixture
            .service
            .create_channel(caller("person", "new"), "request-2".into(), request(None))
            .await,
        Err(ChannelError::ResourceNotFound)
    );
}

/// An agent's own store of sessions: says where each erasure was asked from,
/// and whether the conversation was still in its channel then.
struct PlaceEraser {
    channels: Mutex<Option<ConversationService>>,
    asked: Mutex<Vec<(Option<PathBuf>, bool)>>,
    conversation: Mutex<Option<ConversationId>>,
}

impl PlaceEraser {
    fn ask(&self, place: Option<PathBuf>) -> ConversationFuture<'_, ProviderSessionErasure> {
        Box::pin(async move {
            let service = self.channels.lock().unwrap().clone().unwrap();
            let id = self.conversation.lock().unwrap().clone().unwrap();
            let placed = service.channel_id_of(&id).await.unwrap().is_some();
            self.asked.lock().unwrap().push((place, placed));
            Ok(ProviderSessionErasure::Deleted)
        })
    }
}

impl crate::conversation::application::ProviderSessionEraser for PlaceEraser {
    fn erase(
        &self,
        _session: nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId,
    ) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.ask(None)
    }
    fn erase_in(
        &self,
        _session: nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId,
        place: AgentPlace,
    ) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.ask(Some(place.folder))
    }
    fn cleanup_outstanding(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn a_deleted_conversation_is_erased_from_its_channels_folder_then_let_go() {
    let eraser = Arc::new(PlaceEraser {
        channels: Mutex::new(None),
        asked: Mutex::new(Vec::new()),
        conversation: Mutex::new(None),
    });
    let mut erasers = ProviderSessionErasers::default();
    erasers.register(AgentId::Codex, eraser.clone());
    let fixture = Fixture::with_erasers(erasers);
    *eraser.channels.lock().unwrap() = Some(fixture.service.clone());
    let channel = fixture
        .channel("request-1", Some(absolute("/work/repo").as_str()))
        .await;
    let id = new_id();
    *eraser.conversation.lock().unwrap() = Some(id.clone());
    fixture
        .service
        .create(
            id.clone(),
            caller("person", "create"),
            RequestedConversation {
                channel: Some(channel.id().clone()),
                ..RequestedConversation::default()
            },
        )
        .await
        .unwrap();
    fixture
        .service
        .submit(
            id.clone(),
            caller("person", "say"),
            "say".to_owned(),
            SubmittedMessage {
                uploads: Vec::new(),
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let view = fixture
                .service
                .read(id.clone(), caller("person", "read"))
                .await
                .unwrap();
            if view.messages.iter().any(|message| {
                message.status
                    == nessa_protocol::conversation::view::ConversationMessageStatus::Completed
            }) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the turn completes");
    fixture
        .service
        .delete(id.clone(), caller("person", "delete"))
        .await
        .unwrap();
    // Asked from the channel's folder, while the conversation was still in it.
    assert_eq!(
        eraser.asked.lock().unwrap().clone(),
        vec![(Some(PathBuf::from(absolute("/work/repo"))), true)]
    );
    assert_eq!(fixture.service.channel_id_of(&id).await.unwrap(), None);
}

#[tokio::test]
async fn a_person_keeps_at_most_the_channels_the_rules_allow() {
    let fixture = Fixture::new();
    for n in 0..MAX_CHANNELS_PER_OWNER {
        fixture.channel(&format!("request-{n}"), None).await;
    }
    assert_eq!(
        fixture
            .service
            .create_channel(
                caller("person", "one-more"),
                "one-more".into(),
                request(None)
            )
            .await,
        Err(ChannelError::LimitReached)
    );
    // A retry of one already made is still answered, and somebody else's
    // channels are their own.
    fixture.channel("request-0", None).await;
    fixture
        .service
        .create_channel(caller("other", "first"), "first".into(), request(None))
        .await
        .unwrap();
}
