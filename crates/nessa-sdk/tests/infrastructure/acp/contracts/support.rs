pub(super) use crate::application::agent_execution::agents::*;
pub(super) use crate::application::agent_execution::executions::ExecutionUpdate;
pub(super) use crate::application::agent_execution::executions::*;
pub(super) use crate::application::agent_execution::permissions::*;
pub(super) use crate::application::agent_execution::providers::*;
pub(super) use crate::application::dto::{ModalitiesDto, ModelMetadataDto};
pub(super) use crate::domain::agent_execution::{
    executions::*, permissions::*, prompts::*, sessions::ExecutionFinish,
};
pub(super) use crate::domain::common::value_objects::TokenLimits;
pub(super) use crate::domain::model_metadata::entities::ModelMetadata;
pub(super) use crate::infrastructure::acp::sessions::AcpConfig;
pub(super) use crate::infrastructure::claude_acp::sessions::ClaudeAcpProvider;
pub(super) use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
pub(super) use tempfile::TempDir;
pub(super) use tokio::time::timeout;

#[derive(Default)]
pub(super) struct RecordingAudit {
    pub(super) records: Mutex<Vec<PermissionCancellation>>,
    pub(super) closures: Mutex<Vec<SessionClosureRecord>>,
    pub(super) finishes: Mutex<Vec<ExecutionFinish>>,
    pub(super) answers: Mutex<Vec<PermissionAnswerRecord>>,
    pub(super) reorders: Mutex<Vec<QueueOrderRecord>>,
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
