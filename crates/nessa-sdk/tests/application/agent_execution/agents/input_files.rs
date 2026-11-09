//! Original uploaded input is correlated before native dispatch and stopped by WorkPermit.
use super::*;
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use nessa_sdk::domain::common::value_objects::Sha256Digest;

struct FileProvider {
    inner: Arc<TestProvider>,
    resolver: Arc<dyn nessa_sdk::application::agent_execution::providers::InputFileResolver>,
}
struct WaitingFiles(tokio::sync::Notify);
impl nessa_sdk::application::agent_execution::providers::InputFileResolver for WaitingFiles {
    fn resolve(
        &self,
        _: ExecutionId,
        _: Vec<nessa_sdk::domain::agent_execution::prompts::UploadedFileReference>,
    ) -> nessa_sdk::application::agent_execution::providers::InputFileFuture<'_> {
        Box::pin(async move {
            self.0.notify_one();
            std::future::pending().await
        })
    }
}
impl AgentProvider for FileProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let mut opened = self.inner.open(request).await?;
            opened.session = opened.session.with_input_files(self.resolver.clone());
            Ok(opened)
        })
    }
}
#[tokio::test]
async fn a_targeted_stop_during_file_preparation_is_undispatched_cancelled_and_does_not_close_the_session(
) {
    use nessa_sdk::domain::{
        agent_execution::prompts::UploadedFileReference, common::value_objects::MediaType,
    };
    for queued in [false, true] {
        let storage = MemoryStorage::default();
        let inner = TestProvider::new();
        let resolver = Arc::new(WaitingFiles(tokio::sync::Notify::new()));
        let provider = Arc::new(FileProvider {
            inner: inner.clone(),
            resolver: resolver.clone(),
        });
        let agent = attached_agent(provider, storage.manager().await)
            .await
            .unwrap();
        let input = ExecutionRequest {
            user_message: UserMessage::new(
                None,
                vec![],
                vec![],
                vec![UploadedFileReference::new(
                    Sha256Digest::from_bytes([1; 32]),
                    MediaType::parse("text/plain").unwrap(),
                    4,
                    "notes.txt".into(),
                )
                .unwrap()],
            )
            .unwrap(),
            ..request("preparing-file")
        };
        let running = {
            let agent = agent.clone();
            tokio::spawn(async move {
                if queued {
                    agent
                        .enqueue(input, Submitter::owner(actor()))
                        .await
                        .unwrap()
                        .wait()
                        .await
                } else {
                    agent.invoke(input, Submitter::owner(actor())).await
                }
            })
        };
        tokio::time::timeout(Duration::from_secs(5), resolver.0.notified())
            .await
            .unwrap();
        agent
            .cancel_turn(ExecutionId::new("preparing-file").unwrap())
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap(),
            Ok(ExecutionOutcome::Cancelled)
        );
        let saved = storage.snapshot();
        let record = &saved.invocations[0];
        assert!(record.provider_report.is_none());
        assert!(record.events.is_empty());
        assert!(record.local_cancellation.is_none());
        assert_eq!(record.result, Some(Ok(ExecutionOutcome::Cancelled)));
        assert_eq!(inner.calls.executions.load(Ordering::SeqCst), 0);
        assert_eq!(
            agent
                .invoke(request("after-file-stop"), Submitter::owner(actor()))
                .await
                .unwrap(),
            ExecutionOutcome::Completed
        );
        agent.close(actor()).await.unwrap();
    }
}

struct PreparedFiles(Vec<nessa_sdk::application::agent_execution::providers::PreparedInputFile>);
impl nessa_sdk::application::agent_execution::providers::InputFileResolver for PreparedFiles {
    fn resolve(
        &self,
        _: ExecutionId,
        _: Vec<nessa_sdk::domain::agent_execution::prompts::UploadedFileReference>,
    ) -> nessa_sdk::application::agent_execution::providers::InputFileFuture<'_> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}
#[tokio::test]
async fn prepared_input_must_match_the_original_reference_before_dispatch() {
    use nessa_sdk::{
        application::agent_execution::providers::{PreparedInputFile, UserFileError},
        domain::{
            agent_execution::prompts::{LinkedFile, UploadedFileReference},
            common::value_objects::MediaType,
        },
    };
    let reference = UploadedFileReference::new(
        Sha256Digest::from_bytes([2; 32]),
        MediaType::parse("text/plain").unwrap(),
        4,
        "notes.txt".into(),
    )
    .unwrap();
    for changed in [false, true] {
        let storage = MemoryStorage::default();
        let inner = TestProvider::new();
        let prepared = if changed {
            vec![PreparedInputFile::new(
                UploadedFileReference::new(
                    reference.digest(),
                    reference.media_type().clone(),
                    reference.size(),
                    "different.txt".into(),
                )
                .unwrap(),
                LinkedFile::new("/workspace/file.txt".into()).unwrap(),
            )]
        } else {
            vec![]
        };
        let provider = Arc::new(FileProvider {
            inner: inner.clone(),
            resolver: Arc::new(PreparedFiles(prepared)),
        });
        let agent = attached_agent(provider, storage.manager().await)
            .await
            .unwrap();
        let input = ExecutionRequest {
            user_message: UserMessage::new(None, vec![], vec![], vec![reference.clone()]).unwrap(),
            ..request("correlated-file")
        };
        assert_eq!(
            agent.invoke(input, Submitter::owner(actor())).await,
            Err(AgentError::UserFile(UserFileError::Mismatch))
        );
        assert_eq!(inner.calls.executions.load(Ordering::SeqCst), 0);
        assert_eq!(
            storage.snapshot().invocations[0]
                .request
                .user_message
                .uploads(),
            std::slice::from_ref(&reference)
        );
        agent.close(actor()).await.unwrap();
    }
}
