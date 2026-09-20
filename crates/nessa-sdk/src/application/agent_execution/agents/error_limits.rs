//! Bound adapter-owned error trees before cloning or retaining them.
use super::AgentError;
use crate::application::agent_execution::hooks::{HookError, HookFailure};
use crate::application::agent_execution::sessions::StorageError;
use std::mem;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 32;

impl AgentError {
    /// Borrowed preflight includes spare allocations, nested errors and hook failures.
    pub(crate) fn validate_retained_size(&self) -> Result<(), StorageError> {
        let mut pending = vec![(self, 1usize)];
        let mut bytes = 0usize;
        let mut nodes = 0usize;
        while let Some((error, depth)) = pending.pop() {
            nodes = nodes.saturating_add(1);
            bytes = bytes.saturating_add(mem::size_of::<Self>());
            if nodes > MAX_NODES || depth > MAX_DEPTH {
                return Err(limit_error());
            }
            match error {
                Self::Configuration(text)
                | Self::Unsupported(text)
                | Self::InvalidInput(text)
                | Self::Protocol(text)
                | Self::Transport(text) => bytes = bytes.saturating_add(text.capacity()),
                Self::Storage(error)
                | Self::StorageDuringClose { error, .. }
                | Self::StorageInitialization { error, .. }
                | Self::StorageAfterExecution { error, .. } => {
                    bytes = bytes.saturating_add(storage_bytes(error));
                }
                Self::BeforeInvocationHook(failure) => {
                    bytes = bytes.saturating_add(hook_bytes(failure))
                }
                Self::AfterInvocationHooks { failures, .. } => {
                    nodes = nodes.saturating_add(failures.len());
                    if nodes > MAX_NODES {
                        return Err(limit_error());
                    }
                    bytes = bytes.saturating_add(
                        failures
                            .capacity()
                            .saturating_mul(mem::size_of::<HookFailure>()),
                    );
                    for failure in failures {
                        bytes = bytes.saturating_add(hook_bytes(failure));
                        if bytes > MAX_BYTES {
                            return Err(limit_error());
                        }
                    }
                }
                Self::OutputRetentionLimit
                | Self::DiagnosticLimit
                | Self::SubmissionConflict
                | Self::SubmissionUnresolved
                | Self::ExecutionObservation { .. }
                | Self::MultipleOperationFailures { .. }
                | Self::OperationAndCleanupFailure { .. }
                | Self::Scheduling(_)
                | Self::UserImage(_)
                | Self::Busy
                | Self::Closed
                | Self::StalePermission
                | Self::Provider { .. }
                | Self::Deadline
                | Self::Backpressure
                | Self::CleanupUncertain
                | Self::AuditFailure
                | Self::AuditAndCleanupFailure
                | Self::PermissionAnswerDeliveryAndAuditFailure { .. } => {}
            }
            if bytes > MAX_BYTES || nodes > MAX_NODES {
                return Err(limit_error());
            }
            children(error, |child| pending.push((child, depth + 1)));
        }
        Ok(())
    }

    /// Normalize at the adapter boundary, before any SDK clone or persistence.
    pub(crate) fn bounded(self) -> Self {
        if self.validate_retained_size().is_ok() {
            return self;
        }
        // Compact leaf diagnostics without expanding a bounded report. WorkStatus
        // and resource status are explicit facts outside this diagnostic tree.
        let self_error = match self {
            Self::Configuration(text) => return Self::Configuration(compact_diagnostic(text)),
            Self::Unsupported(text) => return Self::Unsupported(compact_diagnostic(text)),
            Self::InvalidInput(text) => return Self::InvalidInput(compact_diagnostic(text)),
            Self::Protocol(text) => return Self::Protocol(compact_diagnostic(text)),
            Self::Transport(text) => return Self::Transport(compact_diagnostic(text)),
            other => other,
        };
        self_error.discard_iteratively();
        Self::DiagnosticLimit
    }

    /// Dispose rejected external trees without recursively dropping their boxes.
    pub(crate) fn discard_iteratively(self) {
        let mut pending = vec![self];
        while let Some(error) = pending.pop() {
            match error {
                Self::ExecutionObservation {
                    error,
                    execution_result,
                } => {
                    pending.push(*error);
                    if let Some(result) = execution_result {
                        if let Err(error) = *result {
                            pending.push(error);
                        }
                    }
                }
                Self::MultipleOperationFailures {
                    first_error,
                    subsequent_error,
                } => {
                    pending.push(*first_error);
                    pending.push(*subsequent_error);
                }
                Self::OperationAndCleanupFailure {
                    operation_error,
                    cleanup_error,
                } => {
                    pending.push(*operation_error);
                    pending.push(*cleanup_error);
                }
                Self::StorageDuringClose { cleanup_result, .. }
                | Self::StorageInitialization { cleanup_result, .. } => {
                    if let Err(error) = *cleanup_result {
                        pending.push(error);
                    }
                }
                Self::StorageAfterExecution {
                    execution_result, ..
                }
                | Self::AfterInvocationHooks {
                    execution_result, ..
                } => {
                    if let Err(error) = *execution_result {
                        pending.push(error);
                    }
                }
                Self::PermissionAnswerDeliveryAndAuditFailure {
                    delivery_error,
                    cleanup_error,
                } => {
                    pending.push(*delivery_error);
                    if let Some(error) = cleanup_error {
                        pending.push(*error);
                    }
                }
                _ => {}
            }
        }
    }
}

fn limit_error() -> StorageError {
    StorageError::Corrupt("retained error exceeds 1 MiB, 128 nodes, or depth 32".into())
}
fn storage_bytes(error: &StorageError) -> usize {
    match error {
        StorageError::Io(text) | StorageError::Corrupt(text) => text.capacity(),
        _ => 0,
    }
}
fn hook_bytes(failure: &HookFailure) -> usize {
    match &failure.error {
        HookError::Failed(text) => text.capacity(),
        HookError::Panicked => 0,
    }
}
fn children<'a>(error: &'a AgentError, mut visit: impl FnMut(&'a AgentError)) {
    match error {
        AgentError::ExecutionObservation {
            error,
            execution_result,
        } => {
            visit(error);
            if let Some(result) = execution_result {
                if let Err(error) = result.as_ref() {
                    visit(error);
                }
            }
        }
        AgentError::MultipleOperationFailures {
            first_error,
            subsequent_error,
        } => {
            visit(first_error);
            visit(subsequent_error);
        }
        AgentError::OperationAndCleanupFailure {
            operation_error,
            cleanup_error,
        } => {
            visit(operation_error);
            visit(cleanup_error);
        }
        AgentError::StorageDuringClose { cleanup_result, .. }
        | AgentError::StorageInitialization { cleanup_result, .. } => {
            if let Err(error) = cleanup_result.as_ref() {
                visit(error);
            }
        }
        AgentError::StorageAfterExecution {
            execution_result, ..
        }
        | AgentError::AfterInvocationHooks {
            execution_result, ..
        } => {
            if let Err(error) = execution_result.as_ref() {
                visit(error);
            }
        }
        AgentError::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error,
            cleanup_error,
        } => {
            visit(delivery_error);
            if let Some(error) = cleanup_error {
                visit(error);
            }
        }
        _ => {}
    }
}
#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/agents/error_limits.rs"]
mod tests;

fn compact_diagnostic(mut text: String) -> String {
    const LIMIT: usize = 4096;
    const SUFFIX: &str = " [diagnostic truncated]";
    if text.len() > LIMIT {
        let mut end = LIMIT - SUFFIX.len();
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(SUFFIX);
    }
    text.into_boxed_str().into_string()
}
