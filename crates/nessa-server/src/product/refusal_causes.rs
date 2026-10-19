//! Every cause behind a refused conversation command, as static names for
//! the gateway log.
//!
//! A wire code keeps one cause and drops the rest: a close whose agent and
//! upload release both failed answers with the agent's code, and admission
//! evidence that lost audit and storage answers `audit_unavailable`. The
//! log is where the others stay, so this names each variant and walks into
//! every nested failure and typed reason. Each match names every variant,
//! so a new one is named here too. Free text is never written — transport
//! and protocol text, provider diagnostics, hook messages, paths, policy and
//! agent ids — only names, typed reasons and numbers.

use crate::conversation::application::{
    ConversationError, DeletionFailures, McpAppError, StopFailure,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    hooks::{HookError, HookFailure},
    sessions::StorageError,
};
use std::fmt::Write as _;

/// `close-incomplete(agent: closed, release: attachment-release(storage-io))`.
pub(super) fn refusal_causes(error: &ConversationError) -> String {
    let mut out = String::new();
    conversation(&mut out, error);
    out
}

/// A name, then the nested causes that failed as `(label: cause, ...)`;
/// just the name when none did.
struct Group<'a> {
    out: &'a mut String,
    open: bool,
}

impl<'a> Group<'a> {
    fn new(out: &'a mut String, name: &str) -> Self {
        out.push_str(name);
        Self { out, open: false }
    }

    /// Where the next cause is written, after its separator and label.
    fn cause(&mut self, label: &str) -> &mut String {
        self.out.push_str(if self.open { ", " } else { "(" });
        self.open = true;
        if !label.is_empty() {
            self.out.push_str(label);
            self.out.push_str(": ");
        }
        self.out
    }

    fn end(self) {
        if self.open {
            self.out.push(')');
        }
    }
}

fn conversation(out: &mut String, error: &ConversationError) {
    use ConversationError as E;
    let name = match error {
        E::InvalidInput => "invalid-input",
        E::ImagesUnsupported => "images-unsupported",
        E::AttachmentNotFound => "attachment-not-found",
        E::AttachmentRelease(inner) => {
            let mut group = Group::new(out, "attachment-release");
            conversation(group.cause(""), inner);
            return group.end();
        }
        E::AttachmentCleanup {
            storage_failures,
            audit_failures,
        } => {
            let mut group = Group::new(out, "attachment-cleanup");
            if *storage_failures > 0 {
                group.cause("").push_str("storage");
            }
            if *audit_failures > 0 {
                group.cause("").push_str("audit");
            }
            return group.end();
        }
        E::CloseIncomplete { agent, release } => {
            let mut group = Group::new(out, "close-incomplete");
            conversation(group.cause("agent"), agent);
            conversation(group.cause("release"), release);
            return group.end();
        }
        E::ChildCreationRefused(inner) => {
            let mut group = Group::new(out, "child-creation-refused");
            conversation(group.cause(""), inner);
            return group.end();
        }
        E::NotFound => "not-found",
        E::Deleted => "deleted",
        E::ShareTargetNotPaired => "share-target-not-paired",
        E::CapabilitiesRefused { .. } => "capabilities-refused",
        E::ShareGrantChanged => "share-grant-changed",
        E::IdempotencyConflict => "idempotency-conflict",
        E::MessageQuotaExceeded => "message-quota-exceeded",
        E::DrivePolicyUnenforceable => "drive-policy-unenforceable",
        E::Revoked => "revoked",
        E::RevokedAfterSave => "revoked-after-save",
        E::UnavailableAfterSave => "unavailable-after-save",
        E::DeletionIncomplete(failures) => return deletion(out, failures),
        E::AgentNotConfigured => "agent-not-configured",
        E::AgentUnsupported => "agent-unsupported",
        E::LeaseRefused(refusal) => return typed(out, "lease-refused", refusal),
        E::LeaseUnreadable => "lease-unreadable",
        E::EnvironmentNotConfigured => "environment-not-configured",
        E::LinkedFileUnreachable => "linked-file-unreachable",
        E::PlacementUnreadable => "placement-unreadable",
        E::ModelUnavailable => "model-unavailable",
        E::ApprovalModeUnavailable => "approval-mode-unavailable",
        E::RequestConflict => "request-conflict",
        E::TurnRunning => "turn-running",
        E::ApprovalModeNotApplied => "approval-mode-not-applied",
        E::ApprovalModeUncertain => "approval-mode-uncertain",
        E::ModelChangeNotApplied => "model-change-not-applied",
        E::ModelChangeUncertain => "model-change-uncertain",
        E::Capacity => "capacity",
        E::Unavailable => "unavailable",
        E::Metadata => "metadata",
        E::CatalogueIdentityChanged => "catalogue-identity-changed",
        E::CatalogueInvalidRequest => "catalogue-invalid-request",
        E::Audit => "audit",
        E::AdmissionEvidence { audit, storage } => {
            let mut group = Group::new(out, "admission-evidence");
            if let Some(error) = audit {
                agent(group.cause("audit"), error);
            }
            if let Some(error) = storage {
                storage_cause(group.cause("storage"), error);
            }
            return group.end();
        }
        E::Retirement(failures) => {
            let mut group = Group::new(out, "retirement");
            for (_, error) in failures {
                agent(group.cause(""), error);
            }
            return group.end();
        }
        E::RetirementAdmission { cleanup_error } => {
            let mut group = Group::new(out, "retirement-admission");
            if let Some(inner) = cleanup_error {
                conversation(group.cause("cleanup"), inner);
            }
            return group.end();
        }
        E::Agent(error) => return agent(out, error),
        E::PermissionAnswer { error, .. } => {
            let mut group = Group::new(out, "permission-answer");
            agent(group.cause(""), error);
            return group.end();
        }
        E::Storage(error) => return storage_cause(out, error),
        E::McpApp(error) => {
            let mut group = Group::new(out, "mcp-app");
            group.cause("").push_str(error.code().as_str());
            // The server's JSON-RPC code, never its message.
            if let McpAppError::Remote(Some((code, _))) = error {
                let _ = write!(group.cause("remote"), "{code}");
            }
            return group.end();
        }
    };
    out.push_str(name);
}

fn deletion(out: &mut String, failures: &DeletionFailures) {
    let DeletionFailures {
        stop,
        history,
        history_leased_elsewhere,
        provider,
        no_agent_slot,
        audit,
        attachments,
        summary,
        tombstone,
        interrupted,
        another_attempt,
    } = failures;
    let mut group = Group::new(out, "deletion-incomplete");
    match stop {
        Some(StopFailure::OverBudget) => group.cause("stop").push_str("over-budget"),
        Some(StopFailure::Failed(error)) => agent(group.cause("stop"), error),
        None => {}
    }
    for (label, failure) in [
        ("history", history),
        ("provider", provider),
        ("audit", audit),
        ("attachments", attachments),
        ("summary", summary),
        ("tombstone", tombstone),
    ] {
        if let Some(error) = failure {
            conversation(group.cause(label), error);
        }
    }
    for (name, set) in [
        ("history-leased-elsewhere", history_leased_elsewhere),
        ("no-agent-slot", no_agent_slot),
        ("interrupted", interrupted),
        ("another-attempt", another_attempt),
    ] {
        if *set {
            group.cause("").push_str(name);
        }
    }
    group.end();
}

/// The failure a nested `Result` carries, if it failed.
fn failed<T>(result: &Result<T, AgentError>) -> Option<&AgentError> {
    result.as_ref().err()
}

fn agent(out: &mut String, error: &AgentError) {
    use AgentError as A;
    let name = match error {
        A::AttachmentUnavailable(phase) => return typed(out, "attachment", phase),
        A::AttachmentAuthorizationStale => "attachment-authorization-stale",
        A::OutputRetentionLimit => "output-retention-limit",
        A::DiagnosticLimit => "diagnostic-limit",
        A::Configuration(_) => "configuration",
        A::SubmissionConflict => "submission-conflict",
        A::SubmissionUnresolved => "submission-unresolved",
        A::ExecutionObservation {
            error,
            execution_result,
        } => {
            let mut group = Group::new(out, "execution-observation");
            agent(group.cause(""), error);
            if let Some(execution) = execution_result.as_deref().and_then(failed) {
                agent(group.cause("execution"), execution);
            }
            return group.end();
        }
        A::MultipleOperationFailures {
            first_error,
            subsequent_error,
        } => {
            let mut group = Group::new(out, "multiple-operation-failures");
            agent(group.cause("first"), first_error);
            agent(group.cause("then"), subsequent_error);
            return group.end();
        }
        A::OperationAndCleanupFailure {
            operation_error,
            cleanup_error,
        } => {
            let mut group = Group::new(out, "operation-and-cleanup");
            agent(group.cause("operation"), operation_error);
            agent(group.cause("cleanup"), cleanup_error);
            return group.end();
        }
        A::Scheduling(error) => return typed(out, "scheduling", error),
        A::StorageDuringClose {
            error,
            cleanup_result,
        } => {
            let then = failed(cleanup_result);
            return storage_then(out, "storage-during-close", error, "cleanup", then);
        }
        A::Storage(error) => return storage_cause(out, error),
        A::StorageInitialization {
            error,
            cleanup_result,
        } => {
            let then = failed(cleanup_result);
            return storage_then(out, "storage-initialization", error, "cleanup", then);
        }
        A::StorageAfterExecution {
            error,
            execution_result,
        } => {
            let then = failed(execution_result);
            return storage_then(out, "storage-after-execution", error, "execution", then);
        }
        A::Unsupported(_) => "unsupported",
        A::InvalidInput(_) => "invalid-input",
        A::RefusedAfterSave => "refused-after-save",
        A::AfterSaveUnavailable => "after-save-unavailable",
        A::UnknownApp(error) => return typed(out, "unknown-app", error),
        A::UserImage(error) => return typed(out, "user-image", error),
        A::ImageInputRefused(refusal) => return typed(out, "image-input-refused", refusal),
        A::MessageTooLarge {
            encoded_bytes,
            max_bytes,
        } => {
            let mut group = Group::new(out, "message-too-large");
            let _ = write!(group.cause("bytes"), "{encoded_bytes}");
            let _ = write!(group.cause("max"), "{max_bytes}");
            return group.end();
        }
        A::Busy => "busy",
        A::ApprovalModeChanged => "approval-mode-changed",
        A::Closed => "closed",
        A::StalePermission => "stale-permission",
        A::Protocol(_) => "protocol",
        A::AuthenticationRequired { .. } => "authentication-required",
        A::Provider { code, .. } => {
            let mut group = Group::new(out, "provider");
            let _ = write!(group.cause("code"), "{code}");
            return group.end();
        }
        A::Transport(_) => "socket",
        A::Deadline => "deadline",
        A::StartupDeadline(step) => {
            let mut group = Group::new(out, "startup-deadline");
            group.cause("").push_str(step.as_str());
            return group.end();
        }
        A::Backpressure => "backpressure",
        A::CleanupUncertain => "cleanup-uncertain",
        A::AuditFailure => "audit",
        A::AuditAndCleanupFailure => "audit-and-cleanup",
        A::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error,
            cleanup_error,
        } => {
            let mut group = Group::new(out, "permission-answer-delivery-and-audit");
            agent(group.cause("delivery"), delivery_error);
            if let Some(cleanup) = cleanup_error {
                agent(group.cause("cleanup"), cleanup);
            }
            return group.end();
        }
        A::BeforeInvocationHook(failure) => {
            let mut group = Group::new(out, "before-invocation-hook");
            hook(group.cause(""), failure);
            return group.end();
        }
        A::AfterInvocationHooks {
            failures,
            execution_result,
        } => {
            let mut group = Group::new(out, "after-invocation-hooks");
            for failure in failures {
                hook(group.cause(""), failure);
            }
            if let Some(execution) = failed(execution_result) {
                agent(group.cause("execution"), execution);
            }
            return group.end();
        }
    };
    out.push_str(name);
}

/// A cause whose payload is a typed reason with no free text — fieldless
/// enums and sizes — written as its `Debug` form.
fn typed(out: &mut String, name: &str, reason: &impl std::fmt::Debug) {
    let mut group = Group::new(out, name);
    let _ = write!(group.cause(""), "{reason:?}");
    group.end();
}

/// A hook by its registration index, never its failure text.
fn hook(out: &mut String, failure: &HookFailure) {
    let kind = match failure.error {
        HookError::Failed(_) => "failed",
        HookError::Panicked => "panicked",
    };
    let _ = write!(out, "hook-{}-{kind}", failure.index);
}

/// A storage failure, then what the operation it interrupted ended in.
fn storage_then(
    out: &mut String,
    name: &str,
    error: &StorageError,
    label: &str,
    then: Option<&AgentError>,
) {
    let mut group = Group::new(out, name);
    storage_cause(group.cause(""), error);
    if let Some(then) = then {
        agent(group.cause(label), then);
    }
    group.end();
}

/// Which storage failure `conversation_storage_unavailable` stood for: `busy`
/// is a lease or owner someone else holds, `io` the store refusing or failing
/// the work. A shutdown keeps both of its failures, bounded by the storage
/// diagnostic tree limits.
fn storage_cause(out: &mut String, error: &StorageError) {
    let name = match error {
        StorageError::Closed => "storage-closed",
        StorageError::ReadCapacity => "storage-read-capacity",
        StorageError::ReadWorkerPanicked => "storage-read-worker",
        StorageError::DiagnosticLimit => "storage-diagnostic-limit",
        StorageError::ShutdownFailures(failure) => {
            let mut group = Group::new(out, "storage-shutdown");
            storage_cause(group.cause("read"), failure.read());
            storage_cause(group.cause("runtime"), failure.runtime());
            return group.end();
        }
        StorageError::Busy => "storage-busy",
        StorageError::Io(_) => "storage-io",
        StorageError::Corrupt(_) => "storage-corrupt",
        StorageError::Store(_) => "storage-store",
        StorageError::AnotherVersion { found } => {
            let mut group = Group::new(out, "storage-another-version");
            if let Some(found) = found {
                let _ = write!(group.cause("found"), "{found}");
            }
            return group.end();
        }
        StorageError::IdentityMismatch => "storage-identity",
        StorageError::ChangesRequired => "storage-changes-required",
        StorageError::Unresolved => "storage-unresolved",
        StorageError::TooLarge => "storage-too-large",
        StorageError::CommittedReadUnavailable => "storage-committed-read",
    };
    out.push_str(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::application::agent_execution::sessions::StorageShutdownFailure;

    #[test]
    fn a_model_refusal_is_distinct_from_an_uncertain_change() {
        assert_eq!(
            refusal_causes(&ConversationError::ModelChangeNotApplied),
            "model-change-not-applied"
        );
        assert_eq!(
            refusal_causes(&ConversationError::ModelChangeUncertain),
            "model-change-uncertain"
        );
    }

    #[test]
    fn a_close_keeps_the_release_failure_its_code_drops() {
        let error = ConversationError::CloseIncomplete {
            agent: Box::new(ConversationError::Agent(AgentError::Closed)),
            release: Box::new(ConversationError::AttachmentRelease(Box::new(
                ConversationError::Storage(StorageError::Busy),
            ))),
        };
        assert_eq!(
            refusal_causes(&error),
            "close-incomplete(agent: closed, release: attachment-release(storage-busy))"
        );
    }

    #[test]
    fn admission_evidence_keeps_both_channels() {
        let error = ConversationError::AdmissionEvidence {
            audit: Some(AgentError::AuditFailure),
            storage: Some(StorageError::Busy),
        };
        assert_eq!(
            refusal_causes(&error),
            "admission-evidence(audit: audit, storage: storage-busy)"
        );
    }

    #[test]
    fn a_child_creation_refusal_keeps_nested_static_causes() {
        let error = ConversationError::ChildCreationRefused(Box::new(
            ConversationError::ChildCreationRefused(Box::new(ConversationError::Storage(
                StorageError::Busy,
            ))),
        ));
        assert_eq!(
            refusal_causes(&error),
            "child-creation-refused(child-creation-refused(storage-busy))"
        );
        let error = ConversationError::ChildCreationRefused(Box::new(ConversationError::Agent(
            AgentError::Transport("/home/someone/private-socket".into()),
        )));
        assert_eq!(refusal_causes(&error), "child-creation-refused(socket)");
    }

    #[test]
    fn attachment_cleanup_keeps_both_channels() {
        let error = ConversationError::AttachmentCleanup {
            storage_failures: 1,
            audit_failures: 2,
        };
        assert_eq!(refusal_causes(&error), "attachment-cleanup(storage, audit)");
    }

    #[test]
    fn a_shutdown_keeps_both_of_its_failures() {
        let failure = StorageShutdownFailure::new(StorageError::Busy, StorageError::Closed)
            .expect("a small shutdown failure is within the diagnostic limits");
        let error = ConversationError::Storage(StorageError::ShutdownFailures(Box::new(failure)));
        assert_eq!(
            refusal_causes(&error),
            "storage-shutdown(read: storage-busy, runtime: storage-closed)"
        );
    }

    #[test]
    fn a_typed_reason_is_kept_and_hook_text_is_not() {
        let error = ConversationError::Agent(AgentError::AfterInvocationHooks {
            failures: vec![
                HookFailure {
                    index: 0,
                    error: HookError::Failed("/home/someone/secret".into()),
                },
                HookFailure {
                    index: 2,
                    error: HookError::Panicked,
                },
            ],
            execution_result: Box::new(Err(AgentError::Busy)),
        });
        assert_eq!(
            refusal_causes(&error),
            "after-invocation-hooks(hook-0-failed, hook-2-panicked, execution: busy)"
        );
        let lease = ConversationError::LeaseRefused(
            nessa_sdk::domain::agent_execution::leases::LeaseRefusal::EnvironmentBusy,
        );
        assert_eq!(refusal_causes(&lease), "lease-refused(EnvironmentBusy)");
    }

    #[test]
    fn a_leaf_is_its_name_and_never_its_text() {
        let error = ConversationError::Agent(AgentError::Transport("/home/someone/socket".into()));
        assert_eq!(refusal_causes(&error), "socket");
    }
}
