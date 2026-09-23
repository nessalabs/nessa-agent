//! The record on disk is the only evidence a warm-up ever happened, so what it
//! says about cause, initiator and failure is the behaviour, not a detail of
//! the writer.
use super::{DurableWarmUpAudit, ProviderFailure, WarmUpAudit, WarmUpAuditRecord};
use crate::agent_warm_up::domain::{RuntimeFingerprint, WarmUpCause, WarmUpState};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep},
    permissions::ActionContext,
};
use std::path::Path;

fn runtime() -> RuntimeFingerprint {
    RuntimeFingerprint::new("claude-acp", "claude-sonnet-5", "sha256:aa").unwrap()
}

fn initiator() -> ActionContext {
    ActionContext::new("gateway", "runtime_warm_up", "correlation").unwrap()
}

fn record(failure: Option<ProviderFailure>) -> WarmUpAuditRecord {
    WarmUpAuditRecord {
        runtime: runtime(),
        before: WarmUpState::Cold,
        after: if failure.is_some() {
            WarmUpState::Cold
        } else {
            WarmUpState::Warmed
        },
        cause: WarmUpCause::AutomaticPreparation,
        initiator: initiator(),
        session_id: failure.is_none().then(|| "provider-session".to_owned()),
        failure,
        correlation_id: "correlation".into(),
        requested_at_ms: 1_000,
        observed_at_ms: 2_000,
    }
}

/// The single record in `directory`, parsed.
fn written(directory: &Path) -> serde_json::Value {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "one attempt, one record");
    serde_json::from_slice(&std::fs::read(entries.pop().unwrap()).unwrap()).unwrap()
}

#[tokio::test]
async fn a_completed_warm_up_names_an_automatic_cause_and_the_gateway_as_initiator() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("warm-up");
    let audit = DurableWarmUpAudit::new(directory.clone()).unwrap();
    audit.record(record(None)).await.unwrap();

    let value = written(&directory);
    assert_eq!(value["kind"], "agent_runtime_warm_up");
    // Nobody asked for this, and the record says so rather than naming a person.
    assert_eq!(value["cause"], "automatic_runtime_warm_up");
    assert_eq!(value["initiator"]["principalId"], "gateway");
    assert_eq!(value["initiator"]["surfaceId"], "runtime_warm_up");
    assert_eq!(value["initiator"]["requestId"], "correlation");
    assert_eq!(value["target"]["provider"], "claude-acp");
    assert_eq!(value["target"]["model"], "claude-sonnet-5");
    assert_eq!(value["target"]["configuration"], "sha256:aa");
    assert_eq!(value["target"]["sessionId"], "provider-session");
    assert_eq!(value["transition"]["before"], "cold");
    assert_eq!(value["transition"]["after"], "warmed");
    assert!(value["failure"].is_null());
    assert_eq!(value["requestedAtMs"], 1_000);
    assert_eq!(value["observedAtMs"], 2_000);
}

/// The step that ran out of budget is the reason a reader opens one of these,
/// so it is a field they can branch on rather than a rendered Rust enum.
#[tokio::test]
async fn a_failed_warm_up_records_the_startup_step_as_data() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("warm-up");
    let audit = DurableWarmUpAudit::new(directory.clone()).unwrap();
    audit
        .record(record(Some(ProviderFailure {
            error: AgentError::StartupDeadline(AgentStartupStep::new(
                AgentStartupPhase::Session,
                AgentStartupContext::Restored,
            )),
            cleanup_unconfirmed: true,
        })))
        .await
        .unwrap();

    let value = written(&directory);
    assert_eq!(value["transition"]["after"], "cold", "nothing was warmed");
    assert!(value["target"]["sessionId"].is_null());
    assert_eq!(value["failure"]["error"]["kind"], "startup_deadline");
    assert_eq!(value["failure"]["error"]["step"], "session_resume");
    assert_eq!(value["failure"]["error"]["context"], "restored");
    // A launch that may still be running is not a clean failure.
    assert_eq!(value["failure"]["cleanupUnconfirmed"], true);
}

#[tokio::test]
async fn every_failure_a_warm_up_can_reach_is_discriminated() {
    let root = tempfile::tempdir().unwrap();
    for (index, (error, kind)) in [
        (AgentError::Deadline, "deadline"),
        (AgentError::Closed, "closed"),
        (AgentError::CleanupUncertain, "cleanup_uncertain"),
        (AgentError::AuditFailure, "audit_failure"),
        (
            AgentError::Provider {
                code: -32000,
                diagnostic: None,
            },
            "provider",
        ),
        (AgentError::Transport("pipe closed".into()), "transport"),
        (AgentError::Protocol("bad frame".into()), "protocol"),
        (
            AgentError::Configuration("bad path".into()),
            "configuration",
        ),
        (AgentError::Unsupported("no resume".into()), "unsupported"),
        (
            AgentError::InvalidInput("bad actor".into()),
            "invalid_input",
        ),
        (AgentError::Backpressure, "other"),
    ]
    .into_iter()
    .enumerate()
    {
        let directory = root.path().join(format!("warm-up-{index}"));
        let audit = DurableWarmUpAudit::new(directory.clone()).unwrap();
        audit
            .record(record(Some(ProviderFailure {
                error,
                cleanup_unconfirmed: false,
            })))
            .await
            .unwrap();
        let value = written(&directory);
        assert_eq!(value["failure"]["error"]["kind"], kind);
        assert_eq!(value["failure"]["cleanupUnconfirmed"], false);
    }
}

/// A sink that cannot accept evidence has to say so: the service refuses to
/// record completion on an audit failure, so swallowing this would let an
/// unrecorded transition look like one that never happened.
#[tokio::test]
async fn a_directory_that_cannot_be_written_reports_an_audit_failure() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("warm-up");
    let audit = DurableWarmUpAudit::new(directory.clone()).unwrap();
    // Replace the directory with a file: the next write has nowhere to land.
    std::fs::remove_dir_all(&directory).unwrap();
    std::fs::write(&directory, b"not a directory").unwrap();
    assert!(matches!(
        audit.record(record(None)).await,
        Err(crate::agent_warm_up::application::WarmUpError::Audit(_))
    ));
}
