//! Agent participation: attachment close stays recoverable; owned close and disposal seal.
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use nessa_sdk::application::agent_execution::{
    agents::OwnedLifetime, permissions::ActionContext, providers::SessionCloseRequest,
};

use super::{actor, agent, AgentError};

struct RecordingGate {
    scope: Arc<Mutex<()>>,
    sealed: Arc<AtomicBool>,
    disposals: AtomicUsize,
    host_seals: AtomicUsize,
    notes: Mutex<Vec<(bool, bool)>>,
}

impl RecordingGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            scope: Arc::new(Mutex::new(())),
            sealed: Arc::new(AtomicBool::new(false)),
            disposals: AtomicUsize::new(0),
            host_seals: AtomicUsize::new(0),
            notes: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl OwnedLifetime for RecordingGate {
    fn admission_scope(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.scope)
    }
    fn seal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.sealed)
    }
    fn seal_for_disposal(&self) {
        self.disposals.fetch_add(1, Ordering::SeqCst);
        self.sealed.store(true, Ordering::Release);
    }
    async fn seal_for_host(&self, _actor: &ActionContext) -> Result<(), AgentError> {
        self.host_seals.fetch_add(1, Ordering::SeqCst);
        self.sealed.store(true, Ordering::Release);
        Ok(())
    }
    async fn note_attachment(&self, released: bool, evidence_acknowledged: bool) {
        self.notes
            .lock()
            .expect("notes")
            .push((released, evidence_acknowledged));
    }
    async fn join_descendants(&self) -> Result<(), AgentError> {
        Ok(())
    }
}

#[tokio::test]
async fn c15_attachment_close_does_not_seal_or_block_reopen() {
    let agent = agent().await;
    let gate = RecordingGate::new();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    assert!(agent.install_owned_lifetime(gate.clone()).is_err());
    agent.close(actor()).await.unwrap();
    assert!(!gate.sealed.load(Ordering::Acquire));
    assert_eq!(gate.disposals.load(Ordering::SeqCst), 0);
    assert_eq!(gate.host_seals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn c16_owned_close_seals_and_refuses_a_later_attachment() {
    let agent = agent().await;
    let gate = RecordingGate::new();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    agent.end_owned_lifetime(actor()).await.unwrap();
    assert!(gate.sealed.load(Ordering::Acquire));
    assert_eq!(gate.host_seals.load(Ordering::SeqCst), 1);
    assert!(matches!(
        agent.authorize_attachment(
            nessa_sdk::application::agent_execution::agents::AttachmentRequest::CallerRequested(
                actor()
            )
        ),
        Err(AgentError::Closed)
    ));
}

#[tokio::test]
async fn c18_disposal_seals_without_replacing_the_earlier_attachment_cause() {
    let agent = agent().await;
    let gate = RecordingGate::new();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    let first = agent
        .inner
        .lifecycle
        .start_stop(SessionCloseRequest::ExecutionFailed);
    let joined = agent
        .inner
        .lifecycle
        .start_stop(SessionCloseRequest::SessionHandlesDropped);
    assert!(gate.sealed.load(Ordering::Acquire));
    assert_eq!(gate.disposals.load(Ordering::SeqCst), 1);
    assert_eq!(first.request, SessionCloseRequest::ExecutionFailed);
    assert_eq!(joined.request, SessionCloseRequest::ExecutionFailed);
}
