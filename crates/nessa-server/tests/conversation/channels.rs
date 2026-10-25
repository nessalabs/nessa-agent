//! A channel is made once per request id, in a folder the person chose or
//! one made for it; a conversation created in it starts with its defaults
//! and its agent is built in its folder; nobody else can use it.
use super::*;
use crate::conversation::application::{
    ConversationAgent, ConversationAgentFuture, ConversationAgentSource, ConversationAgents,
    ConversationDependencies, ConversationLimits, ProviderSessionErasers, RequestedConversation, SubmissionMode, SubmittedMessage,
};
use crate::conversation::domain::{
    ChannelDefaults, ChannelName, ChannelNotes, ChannelRefusal,
};
use crate::conversation::infrastructure::{LocalChannelStore, LocalConversationStore};
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
        assert!(place.instructions.contains("Its folder is"), "{}", place.instructions);
        self.record(agent, model, mode, Some(place))
    }
}

/// Folders as a fixed table says: `/missing` is not there, `/file` is a
/// file, `/refused` is refused, and a made folder is under `/data/channels`.
struct TableFolders;

impl ChannelFolders for TableFolders {
    fn accept_folder(&self, chosen: &ChannelPath) -> Result<ChannelPath, FolderRefusal> {
        match chosen.as_str() {
            "/missing" => Err(FolderRefusal::NotFound),
            "/file" => Err(FolderRefusal::NotADirectory),
            "/refused" => Err(FolderRefusal::Refused),
            // A link, kept as where it leads.
            "/link" => Ok(ChannelPath::new(PathBuf::from("/work/repo")).unwrap()),
            _ => Ok(chosen.clone()),
        }
    }
    fn accept_resource(&self, path: &ChannelPath) -> Result<ChannelPath, FolderRefusal> {
        match path.as_str() {
            "/missing" => Err(FolderRefusal::NotFound),
            _ => Ok(path.clone()),
        }
    }
    fn make(&self, id: &ChannelId) -> Result<ChannelPath, FolderRefusal> {
        Ok(ChannelPath::new(PathBuf::from(format!("/data/channels/{id}"))).unwrap())
    }
}

struct Fixture {
    service: ConversationService,
    source: Arc<RecordingSource>,
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
        name: ChannelName::new("design-review").unwrap(),
        folder: folder.map(path),
        defaults: ChannelDefaults {
            agent: AgentId::Codex,
            model: ConversationModelId::new(CODEX_MODEL).unwrap(),
            mode: ConversationApprovalMode::Auto,
        },
        notes: ChannelNotes::new("Check every export.").unwrap(),
        resources: ChannelResources::new(vec![path("/work/brand-kit")]).unwrap(),
    }
}

impl Fixture {
    fn new() -> Self {
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
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                clock: Arc::new(TestClock),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits::default(),
            Some("/work/default".into()),
        )
        .unwrap();
        service.bind_commands(storage);
        service.bind_channels(
            Arc::new(LocalChannelStore::open(&root.join("channels.sqlite3")).unwrap()),
            Arc::new(TableFolders),
        );
        Self {
            service,
            source,
            _directory: directory,
        }
    }

    async fn channel(&self, request_id: &str, folder: Option<&str>) -> Channel {
        self.service
            .create_channel(caller("person", request_id), request_id.into(), request(folder))
            .await
            .unwrap()
    }

    fn asked(&self) -> Vec<Asked> {
        std::mem::take(&mut self.source.asked.lock().unwrap())
    }
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
        format!("/data/channels/{}", made.id())
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
    assert_eq!(listing.default_folder.as_deref(), Some("/work/default"));
    assert_eq!(listing.channels, vec![made]);
}

#[tokio::test]
async fn a_chosen_folder_is_kept_resolved_and_a_refused_one_says_why() {
    let fixture = Fixture::new();
    let linked = fixture.channel("request-1", Some("/link")).await;
    assert!(!linked.folder().made);
    assert_eq!(linked.folder().path.as_str(), "/work/repo");
    // A retry naming the same place another way is the same request.
    assert_eq!(fixture.channel("request-1", Some("/work/repo")).await, linked);
    // Two channels may share a folder.
    let shared = fixture.channel("request-2", Some("/work/repo")).await;
    assert_ne!(shared.id(), linked.id());
    for (folder, refusal) in [
        ("/missing", FolderRefusal::NotFound),
        ("/file", FolderRefusal::NotADirectory),
        ("/refused", FolderRefusal::Refused),
    ] {
        assert_eq!(
            fixture
                .service
                .create_channel(caller("person", folder), folder.into(), request(Some(folder)))
                .await,
            Err(ChannelError::Folder(refusal)),
            "{folder}"
        );
    }
    let mut missing = request(None);
    missing.resources = ChannelResources::new(vec![path("/missing")]).unwrap();
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
    let channel = fixture.channel("request-1", Some("/work/repo")).await;
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
                Some(PathBuf::from("/work/repo"))
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
                Some(PathBuf::from("/work/repo"))
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
