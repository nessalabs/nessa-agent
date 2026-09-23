pub(super) use crate::application::agent_execution::agents::*;
pub(super) use crate::application::agent_execution::executions::ExecutionUpdate;
pub(super) use crate::application::agent_execution::executions::*;
pub(super) use crate::application::agent_execution::permissions::*;
pub(super) use crate::application::agent_execution::providers::*;
pub(super) use crate::application::agent_execution::sessions::SessionManager;
pub(super) use crate::application::dto::{ImageInputLimitsDto, ModalitiesDto, ModelMetadataDto};
pub(super) use crate::domain::agent_execution::{
    executions::*, permissions::*, prompts::*, sessions::ExecutionFinish,
};
pub(super) use crate::domain::common::value_objects::TokenLimits;
pub(super) use crate::domain::model_metadata::entities::ModelMetadata;
pub(super) use crate::infrastructure::acp::sessions::AcpConfig;
pub(super) use crate::infrastructure::claude_acp::sessions::ClaudeAcpProvider;
pub(super) use crate::infrastructure::codex_acp::sessions::CodexAcpProvider;
pub(super) use crate::infrastructure::opencode_acp::sessions::OpencodeAcpProvider;
pub(super) use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
pub(super) use tempfile::TempDir;
pub(super) use tokio::time::timeout;

struct AcceptingLifecycleAudit;
impl ExecutionAudit for AcceptingLifecycleAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub(super) async fn attached_agent(
    provider: Arc<dyn AgentProvider>,
    manager: SessionManager,
) -> Result<Agent, AgentError> {
    let agent = Agent::prepare(provider, manager, Arc::new(AcceptingLifecycleAudit))
        .await
        .map_err(|error| error.cause().clone())?;
    let authorization =
        agent.authorize_attachment(AttachmentRequest::CallerRequested(close_action()))?;
    agent.start_attachment(authorization)?.wait().await?;
    Ok(agent)
}

pub(super) async fn attach_agent(
    agent: &Agent,
    request: AttachmentRequest,
) -> Result<(), AgentError> {
    let authorization = agent.authorize_attachment(request)?;
    agent.start_attachment(authorization)?.wait().await
}

#[derive(Default)]
pub(super) struct RecordingAudit {
    pub(super) records: Mutex<Vec<PermissionCancellation>>,
    pub(super) closures: Mutex<Vec<SessionClosureRecord>>,
    pub(super) finishes: Mutex<Vec<ExecutionFinish>>,
    pub(super) answers: Mutex<Vec<PermissionAnswerRecord>>,
    pub(super) reorders: Mutex<Vec<QueueOrderRecord>>,
    pub(super) declines: Mutex<Vec<ReviewDeclineRecord>>,
    pub(super) reject: bool,
    pub(super) stall: bool,
}
impl ExecutionAudit for RecordingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if self.stall {
                return std::future::pending().await;
            }
            if self.reject {
                return Err(AgentError::Transport("fixture audit rejected".into()));
            }
            match record {
                ExecutionAuditRecord::Finished(record) => {
                    self.finishes.lock().unwrap().push(record)
                }
                ExecutionAuditRecord::SessionClosed(record) => {
                    self.closures.lock().unwrap().push(record)
                }
                ExecutionAuditRecord::Cancelled(record) => {
                    self.records.lock().unwrap().push(record)
                }
                ExecutionAuditRecord::Answered(record) => self.answers.lock().unwrap().push(record),
                ExecutionAuditRecord::QueueReordered(record) => {
                    self.reorders.lock().unwrap().push(record)
                }
                ExecutionAuditRecord::Attachment(_)
                | ExecutionAuditRecord::QueueAdmitted(_)
                | ExecutionAuditRecord::QueueSettled(_)
                | ExecutionAuditRecord::SteeringAcknowledged(_) => {}
                ExecutionAuditRecord::ReviewDeclined(record) => {
                    self.declines.lock().unwrap().push(record)
                }
            }
            Ok(())
        })
    }
}
pub(super) fn close_action() -> ActionContext {
    ActionContext::new("fixture-host", "fixture-tests", "close-session").unwrap()
}
pub(super) fn test_acp_binding_with_audit(
    mode: &str,
    capacity: usize,
    audit: Arc<RecordingAudit>,
) -> (TempDir, ClaudeAcpProvider) {
    let (root, config, model) = test_acp_configuration(mode, capacity);
    (
        root,
        ClaudeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit).unwrap(),
    )
}

pub(super) fn test_acp_configuration(
    mode: &str,
    capacity: usize,
) -> (TempDir, AcpConfig, ModelMetadata) {
    let root = tempfile::tempdir().unwrap();
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "exact-fixture-model".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: true,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let config = AcpConfig {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/infrastructure/acp/contracts/fixtures/claude_acp_test_handler.py")
                .into_os_string(),
            mode.into(),
        ],
        environment: BTreeMap::new(),
        credential_environment: BTreeMap::new(),
        workspace: root.path().to_path_buf(),
        tools_enabled: true,
        mcp_servers: Vec::new(),
        permissions: PermissionOfferPolicy::once_only(),
        launch_timeout: Duration::from_secs(10),
        startup_timeout: Duration::from_secs(10),
        execution_timeout: Some(Duration::from_millis(700)),
        shutdown_grace: Duration::from_millis(100),
        kill_timeout: Duration::from_secs(2),
        event_capacity: capacity,
        max_frame_bytes: 8192,
        max_incoming_frame_bytes: 8192,
        images: None,
    };
    (root, config, model)
}
pub(super) fn test_acp_binding(mode: &str, capacity: usize) -> (TempDir, ClaudeAcpProvider) {
    let (root, config, model) = test_acp_configuration(mode, capacity);
    (
        root,
        ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
        .unwrap(),
    )
}
/// The same fixture setup for Codex, which is launched and configured
/// differently enough that sharing one builder would hide the difference.
pub(super) fn codex_configuration(
    mode: &str,
    capacity: usize,
) -> (TempDir, AcpConfig, ModelMetadata) {
    let (root, mut config, _) = test_acp_configuration(mode, capacity);
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/codex_acp_test_handler.py")
            .into_os_string(),
        mode.into(),
    ];
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "openai".into(),
        model_id: "exact-fixture-model".into(),
        display_name: "Fixture".into(),
        input: text,
        output: text,
        tool_use: true,
        reasoning: true,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
        image_input: None,
    })
    .unwrap();
    (root, config, model)
}

pub(super) fn test_codex_binding(mode: &str, capacity: usize) -> (TempDir, CodexAcpProvider) {
    let (root, config, model) = codex_configuration(mode, capacity);
    (
        root,
        CodexAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
        .unwrap(),
    )
}

/// The same fixture setup for Opencode, whose session is configured partly over
/// the protocol (the mode) and partly at launch (the permission policy), and
/// whose models are served by its own gateway.
pub(super) fn opencode_configuration(
    mode: &str,
    capacity: usize,
) -> (TempDir, AcpConfig, ModelMetadata) {
    let (root, mut config, _) = test_acp_configuration(mode, capacity);
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/opencode_acp_test_handler.py")
            .into_os_string(),
        mode.into(),
    ];
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "opencode".into(),
        model_id: "exact-fixture-model".into(),
        display_name: "Fixture".into(),
        input: text,
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
        image_input: None,
    })
    .unwrap();
    (root, config, model)
}

pub(super) fn test_opencode_binding(mode: &str, capacity: usize) -> (TempDir, OpencodeAcpProvider) {
    test_opencode_binding_with_audit(mode, capacity, Arc::new(RecordingAudit::default()))
}

/// The same binding with the audit sink kept, for the tests that assert on
/// what was recorded rather than on what the session did.
///
/// A binding that builds its own sink and drops the handle can only be checked
/// through its effects, which is how a decision written against no record, or
/// against the wrong one, stays invisible.
pub(super) fn test_opencode_binding_with_audit(
    mode: &str,
    capacity: usize,
    audit: Arc<RecordingAudit>,
) -> (TempDir, OpencodeAcpProvider) {
    let (root, config, model) = opencode_configuration(mode, capacity);
    (
        root,
        OpencodeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit)
            .unwrap(),
    )
}

/// The same binding on a model that does take images.
///
/// The fixture model is text-only, so what a binding declares about images is
/// invisible through it: the effective capability would come out text either
/// way, and a binding that passed Opencode's advertised image prompt through
/// would look the same as one that did not. This is the model that tells them
/// apart.
pub(super) fn test_opencode_binding_on_a_model_that_takes_images(
    mode: &str,
    capacity: usize,
) -> (TempDir, OpencodeAcpProvider) {
    test_opencode_binding_on_a_model_that_takes_images_from(mode, capacity, None)
}

/// The same, with the byte source composition would have supplied.
///
/// Image input needs three things to agree: the model's recorded limits, the
/// agent's advertised prompt capability, and somewhere to read the bytes from.
/// The binding owns only the last of those, so it is the one a test has to be
/// able to vary. The source is never read here — what is asserted is what the
/// session declares, not what it sends.
pub(super) fn test_opencode_binding_on_a_model_that_takes_images_from(
    mode: &str,
    capacity: usize,
    images: Option<Arc<dyn UserImageSource>>,
) -> (TempDir, OpencodeAcpProvider) {
    let (root, mut config, model) = opencode_configuration(mode, capacity);
    config.images = images;
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "opencode".into(),
        model_id: model.key().model_id().into(),
        display_name: "Fixture that takes images".into(),
        input: ModalitiesDto {
            text: true,
            image: true,
            audio: false,
        },
        // Recorded limits, not just the modality. A model whose limits are not
        // recorded is offered no images whatever its modality says, so without
        // these the assertion below would hold for a reason that has nothing to
        // do with the binding — which is the one thing it is there to test.
        image_input: Some(ImageInputLimitsDto {
            media_types: vec!["image/png".into(), "image/jpeg".into()],
            max_encoded_bytes: 5_000_000,
            max_edge_px: 8000,
            many_images_max_edge_px: 2000,
            native_long_edge_px: 1568,
        }),
        output: ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        },
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    (
        root,
        OpencodeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
        .unwrap(),
    )
}

/// A byte source that exists and is never asked. Declaring image input needs
/// composition to have supplied one; it does not need it to answer.
pub(super) struct UnreadImages;
impl UserImageSource for UnreadImages {
    fn read(&self, _: ImageReference) -> UserImageFuture<'_> {
        unreachable!("these tests assert on declared capability, never on a prompt")
    }
}

pub(super) fn prompt(text: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(text).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(text).unwrap()),
        estimated_input_tokens: 10,
        reserved_output_tokens: 100,
    }
}
pub(super) async fn start(
    opened: &OpenedProviderSession,
    text: &str,
) -> tokio::task::JoinHandle<Result<ExecutionOutcome, AgentError>> {
    let session = opened.session.clone();
    let input = prompt(text);
    tokio::spawn(async move { session.execute(input).await.into_result() })
}
pub(super) async fn next(opened: &mut OpenedProviderSession) -> ExecutionUpdate {
    timeout(Duration::from_secs(3), opened.events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_update()
}
pub(super) fn assert_gone(root: &TempDir, file: &str) {
    let pid: i32 = std::fs::read_to_string(root.path().join(file))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "fixture process {pid} survived"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

pub(super) fn attribution() -> ApprovalAttribution {
    ApprovalAttribution::new(
        ActionContext::new("nessa.binding-fixture", "nessa.cli", "fixture-answer").unwrap(),
        ApprovalBasis::Mode(ApprovalModeSnapshot::new("fixture-file-policy", "1").unwrap()),
    )
}

pub(super) async fn next_after_restore(opened: &mut OpenedProviderSession) -> ExecutionUpdate {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Some(event) = opened
                .events
                .next()
                .await
                .map_err(|failure| failure.into_error())
                .unwrap()
            {
                return event.into_update();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

// Native startup and reaping use real OS scheduling. Bound concurrent scenarios
// independently of libtest's CPU-based thread count; individual isolation tests
// still start multiple workers. No backend or client state is shared.
static PROCESS_TEST_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub(super) async fn process_test_slot() -> tokio::sync::SemaphorePermit<'static> {
    PROCESS_TEST_SLOTS.acquire().await.unwrap()
}

pub(super) async fn wait_for_file(root: &TempDir, file: &str) {
    timeout(Duration::from_secs(10), async {
        while !root.path().join(file).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture did not publish its startup marker");
}

pub(super) async fn wait_until_gone(root: &TempDir, file: &str) {
    let pid: i32 = std::fs::read_to_string(root.path().join(file))
        .unwrap()
        .parse()
        .unwrap();
    timeout(Duration::from_secs(5), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture process was not reaped");
    assert_gone(root, file);
}
