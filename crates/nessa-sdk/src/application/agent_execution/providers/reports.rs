//! Provider observations keep resource ownership separate from diagnostic failures.
#![deny(missing_docs)]

use super::{CloseOutcome, FinalizedExecutionProjection, FinalizedExecutionSource};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionEvent;
use crate::application::agent_execution::permissions::PermissionSelectionState;
use crate::domain::agent_execution::executions::ExecutionOutcome;
use std::{future::Future, pin::Pin};

/// Observed physical cleanup. An error never implicitly establishes termination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceCleanup {
    /// The adapter confirmed release of its owned resources.
    Confirmed(CloseOutcome),
    /// Resources remain owned; retain the provider session and retry cleanup.
    Unconfirmed(AgentError),
}

/// Physical cleanup and acknowledgement of required audit evidence are independent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupReport {
    resources: ResourceCleanup,
    audit: Result<(), AgentError>,
    operation_failure: Option<Box<AgentError>>,
    completion_failure: Option<Box<AgentError>>,
}
impl CleanupReport {
    /// Record actual `resources` and independent `audit` acknowledgement.
    /// Diagnostic errors are bounded without changing either explicit status.
    pub fn new(resources: ResourceCleanup, audit: Result<(), AgentError>) -> Self {
        let resources = match resources {
            ResourceCleanup::Confirmed(outcome) => ResourceCleanup::Confirmed(outcome),
            ResourceCleanup::Unconfirmed(error) => ResourceCleanup::Unconfirmed(error.bounded()),
        };
        Self {
            resources,
            audit: audit.map_err(AgentError::bounded),
            operation_failure: None,
            completion_failure: None,
        }
    }
    /// Confirm cleanup with no outstanding audit failure.
    pub fn confirmed(outcome: CloseOutcome) -> Self {
        Self::new(ResourceCleanup::Confirmed(outcome), Ok(()))
    }
    /// Retain resource ownership after failed cleanup, without an audit failure.
    pub fn unconfirmed(error: AgentError) -> Self {
        Self::new(ResourceCleanup::Unconfirmed(error), Ok(()))
    }
    /// Physical cleanup observation, independent of audit delivery.
    pub fn resources(&self) -> &ResourceCleanup {
        &self.resources
    }
    /// Required audit acknowledgement. Failed physical retries do not erase it.
    pub fn audit(&self) -> &Result<(), AgentError> {
        &self.audit
    }
    /// Whether resource ownership can be released, including audit-only failure.
    pub fn is_confirmed(&self) -> bool {
        matches!(self.resources, ResourceCleanup::Confirmed(_))
    }
    /// Replace physical evidence after retry, retaining the original audit result.
    pub fn with_resources(self, resources: ResourceCleanup) -> Self {
        Self::new(resources, self.audit)
            .with_operation_failure(self.operation_failure.map(|error| *error))
            .with_completion_failure(self.completion_failure.map(|error| *error))
    }
    /// Replace audit acknowledgement while preserving physical cleanup and the
    /// initiating operation diagnostic.
    pub fn with_audit(self, audit: Result<(), AgentError>) -> Self {
        Self::new(self.resources, audit)
            .with_operation_failure(self.operation_failure.map(|error| *error))
            .with_completion_failure(self.completion_failure.map(|error| *error))
    }
    /// Preserve the initiating operation diagnostic alongside cleanup and audit.
    pub fn with_operation_failure(mut self, failure: Option<AgentError>) -> Self {
        self.operation_failure = failure.map(|error| Box::new(error.bounded()));
        self
    }
    /// Initiating operation failure, separate from this cleanup attempt.
    pub fn operation_failure(&self) -> Option<&AgentError> {
        self.operation_failure.as_deref()
    }
    /// Failure while supervising completion of this cleanup attempt.
    ///
    /// This is independent of physical release and audit acknowledgement. For
    /// example, a provider may return confirmed cleanup before destroying its
    /// cleanup future panics. The confirmed physical fact remains authoritative,
    /// while callers still observe the supervision failure.
    pub fn completion_failure(&self) -> Option<&AgentError> {
        self.completion_failure.as_deref()
    }
    /// Preserve a failure from supervising this cleanup attempt.
    pub fn with_completion_failure(mut self, failure: Option<AgentError>) -> Self {
        self.completion_failure = failure.map(|error| Box::new(error.bounded()));
        self
    }
    /// Return the result of this cleanup attempt.
    /// Confirmed resources and successful audit delivery return `Ok` when cleanup
    /// supervision also completed, even when `operation_failure` retains an earlier
    /// operation error. That field is historical evidence, not a failure of an
    /// otherwise successful cleanup. A `completion_failure` remains observable
    /// without changing the physical-release fact.
    /// If cleanup, supervision, or audit fails, the returned error includes it.
    /// Inspect [`Self::operation_failure`] before consuming the report when the
    /// original operation matters. Session lifecycle decisions use
    /// [`Self::resources`] and [`Self::audit`] directly.
    pub fn into_result(self) -> Result<CloseOutcome, AgentError> {
        let Self {
            resources,
            audit,
            operation_failure,
            completion_failure,
        } = self;
        let cleanup = match (resources, audit) {
            (ResourceCleanup::Confirmed(outcome), Ok(())) => match completion_failure {
                Some(error) => return Err(*error),
                None => return Ok(outcome),
            },
            (ResourceCleanup::Confirmed(_), Err(error))
            | (ResourceCleanup::Unconfirmed(error), Ok(())) => error,
            (ResourceCleanup::Unconfirmed(cleanup_error), Err(operation_error)) => {
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(operation_error),
                    cleanup_error: Box::new(cleanup_error),
                }
            }
        };
        let cleanup = match completion_failure {
            Some(completion_error) => AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(cleanup),
                cleanup_error: completion_error,
            },
            None => cleanup,
        };
        Err(match operation_failure {
            Some(operation_error) => AgentError::OperationAndCleanupFailure {
                operation_error,
                cleanup_error: Box::new(cleanup),
            },
            None => cleanup,
        })
    }
}

/// Cleanup always reports physical status, including supervisor failure.
pub type CleanupFuture<'a> = Pin<Box<dyn Future<Output = CleanupReport> + Send + 'a>>;

/// Provider session status reported by an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderSessionState {
    /// The operation did not revoke admission to this provider session.
    Usable,
    /// Stop admission and retain resources until explicit cleanup is confirmed.
    CleanupRequired,
    /// Cleanup was attempted; the report says whether it succeeded.
    CleanupReported(CleanupReport),
}

/// Failed provider operation with explicit provider session status; errors are diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderOperationFailure {
    error: AgentError,
    session_state: Box<ProviderSessionState>,
    permission_selection: Option<PermissionSelectionState>,
}
impl ProviderOperationFailure {
    /// Record `error` and the adapter-observed provider session status without inference.
    pub fn new(error: AgentError, session_state: ProviderSessionState) -> Self {
        Self {
            error: error.bounded(),
            session_state: Box::new(session_state),
            permission_selection: None,
        }
    }
    /// Record a permission-answer failure and the independently observed domain
    /// selection state. Diagnostics must not be used to reconstruct this fact.
    pub fn permission_answer(
        error: AgentError,
        session_state: ProviderSessionState,
        selection: PermissionSelectionState,
    ) -> Self {
        Self {
            error: error.bounded(),
            session_state: Box::new(session_state),
            permission_selection: Some(selection),
        }
    }
    /// Diagnostic cause, never a source of resource ownership decisions.
    pub fn error(&self) -> &AgentError {
        &self.error
    }
    /// Status for the work generation that admitted this operation.
    pub fn session_state(&self) -> &ProviderSessionState {
        &self.session_state
    }
    /// Known permission selection state when this failure belongs to an answer.
    pub fn permission_selection(&self) -> Option<PermissionSelectionState> {
        self.permission_selection
    }
    /// Separate diagnostic projection from explicit provider session evidence.
    pub fn into_parts(self) -> (AgentError, ProviderSessionState) {
        (self.error, *self.session_state)
    }
    /// Public consumer projection; does not convey resource authority.
    pub fn into_error(self) -> AgentError {
        self.error
    }
}

/// An operation result that never hides resource status inside a diagnostic tree.
pub type ProviderOperationResult<T> = Result<T, ProviderOperationFailure>;
/// Provider operation with an explicit failure and session status.
pub type ProviderOperationFuture<'a, T> =
    Pin<Box<dyn Future<Output = ProviderOperationResult<T>> + Send + 'a>>;

/// Who established the invocation result; neither variant confirms tool rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionReportSource {
    /// A provider response, or an attempt whose provider result remains unknown.
    Provider,
    /// The application stopped the invocation locally; there need not be a provider reply.
    LocalCancellation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FinalizedExecutionReport {
    source: FinalizedExecutionSource,
    physical: ResourceCleanup,
    completion_failure: Option<AgentError>,
    projection: FinalizedExecutionProjection,
    failure: Option<AgentError>,
    session_state: ProviderSessionState,
}
impl FinalizedExecutionReport {
    fn new(
        source: FinalizedExecutionSource,
        physical: ResourceCleanup,
        completion_failure: Option<AgentError>,
        projection: FinalizedExecutionProjection,
    ) -> Self {
        let physical = match physical {
            ResourceCleanup::Confirmed(outcome) => ResourceCleanup::Confirmed(outcome),
            ResourceCleanup::Unconfirmed(error) => ResourceCleanup::Unconfirmed(error.bounded()),
        };
        let completion_failure = completion_failure.map(AgentError::bounded);
        let failure = projection.consumer_failure();
        let session_state = ProviderSessionState::CleanupReported(
            CleanupReport::new(physical.clone(), projection.audit_result())
                .with_operation_failure(projection.operation_failure())
                .with_completion_failure(completion_failure.clone()),
        );
        Self {
            source,
            physical,
            completion_failure,
            projection,
            failure,
            session_state,
        }
    }
    fn physical_failure(&self) -> Option<AgentError> {
        let physical = match &self.physical {
            ResourceCleanup::Confirmed(_) => None,
            ResourceCleanup::Unconfirmed(error) => Some(error.clone()),
        };
        combine_failures(physical, self.completion_failure.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExecutionReportState {
    Independent {
        provider_result: Option<Result<ExecutionOutcome, AgentError>>,
        source: ExecutionReportSource,
        failure: Option<AgentError>,
        session_state: ProviderSessionState,
    },
    Finalized(FinalizedExecutionReport),
}

type IndependentExecutionParts<'a> = (
    Option<&'a Result<ExecutionOutcome, AgentError>>,
    ExecutionReportSource,
    Option<&'a AgentError>,
    &'a ProviderSessionState,
);
type FinalizedExecutionParts<'a> = (
    &'a FinalizedExecutionSource,
    &'a ResourceCleanup,
    Option<&'a AgentError>,
    &'a FinalizedExecutionProjection,
);

/// Provider result and any separate delivery or audit failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionReport {
    state: Box<ExecutionReportState>,
}
impl ExecutionReport {
    /// Retain the actual provider result (None if unknown), independent failure,
    /// and provider session status. No constructor infers status from an error.
    pub fn new(
        provider_result: Option<Result<ExecutionOutcome, AgentError>>,
        failure: Option<AgentError>,
        session_state: ProviderSessionState,
    ) -> Self {
        Self {
            state: Box::new(ExecutionReportState::Independent {
                source: ExecutionReportSource::Provider,
                provider_result: provider_result.map(|result| result.map_err(AgentError::bounded)),
                failure: failure.map(AgentError::bounded),
                session_state,
            }),
        }
    }
    /// Build a provider settlement from one validated finalized recipe.
    pub(crate) fn finalized_provider(
        provider_result: Option<Result<ExecutionOutcome, AgentError>>,
        physical: ResourceCleanup,
        completion_failure: Option<AgentError>,
        projection: FinalizedExecutionProjection,
    ) -> Self {
        let provider_result = provider_result.map(|result| result.map_err(AgentError::bounded));
        Self {
            state: Box::new(ExecutionReportState::Finalized(
                FinalizedExecutionReport::new(
                    FinalizedExecutionSource::Provider(provider_result),
                    physical,
                    completion_failure,
                    projection,
                ),
            )),
        }
    }
    /// Build a locally cancelled settlement from one validated finalized recipe.
    pub(crate) fn finalized_local_cancellation(
        physical: ResourceCleanup,
        completion_failure: Option<AgentError>,
        projection: FinalizedExecutionProjection,
    ) -> Self {
        Self {
            state: Box::new(ExecutionReportState::Finalized(
                FinalizedExecutionReport::new(
                    FinalizedExecutionSource::LocalCancellation,
                    physical,
                    completion_failure,
                    projection,
                ),
            )),
        }
    }
    /// Internal finalized authority for current storage encoding.
    pub(crate) fn finalized_parts(&self) -> Option<FinalizedExecutionParts<'_>> {
        let ExecutionReportState::Finalized(report) = self.state.as_ref() else {
            return None;
        };
        Some((
            &report.source,
            &report.physical,
            report.completion_failure.as_ref(),
            &report.projection,
        ))
    }
    pub(crate) fn independent_parts(&self) -> Option<IndependentExecutionParts<'_>> {
        let ExecutionReportState::Independent {
            provider_result,
            source,
            failure,
            session_state,
        } = self.state.as_ref()
        else {
            return None;
        };
        Some((
            provider_result.as_ref(),
            *source,
            failure.as_ref(),
            session_state,
        ))
    }
    /// Record local cancellation and separate cleanup evidence without inventing a provider reply.
    /// Use this only when settling an invocation stopped by its owning Agent. The
    /// Agent requires its own captured stop and persists that cause and caller; the
    /// report cannot authorize cancellation. Provider-originated cancellation belongs
    /// in the actual provider result instead.
    /// Unconfirmed cleanup still projects an error; local intent never confirms termination.
    pub fn cancelled_locally(report: CleanupReport) -> Self {
        Self {
            state: Box::new(ExecutionReportState::Independent {
                provider_result: None,
                failure: None,
                session_state: ProviderSessionState::CleanupReported(report),
                source: ExecutionReportSource::LocalCancellation,
            }),
        }
    }
    /// Whether local cancellation settled the invocation without a provider response.
    pub fn source(&self) -> ExecutionReportSource {
        match self.state.as_ref() {
            ExecutionReportState::Independent { source, .. } => *source,
            ExecutionReportState::Finalized(report) => match &report.source {
                FinalizedExecutionSource::Provider(_) => ExecutionReportSource::Provider,
                FinalizedExecutionSource::LocalCancellation => {
                    ExecutionReportSource::LocalCancellation
                }
            },
        }
    }
    /// Result actually observed from the provider, or None when unknown.
    pub fn provider_result(&self) -> Option<&Result<ExecutionOutcome, AgentError>> {
        match self.state.as_ref() {
            ExecutionReportState::Independent {
                provider_result, ..
            } => provider_result.as_ref(),
            ExecutionReportState::Finalized(report) => match &report.source {
                FinalizedExecutionSource::Provider(result) => result.as_ref(),
                FinalizedExecutionSource::LocalCancellation => None,
            },
        }
    }
    /// Independent observation, delivery, or audit failure.
    pub fn failure(&self) -> Option<&AgentError> {
        match self.state.as_ref() {
            ExecutionReportState::Independent { failure, .. } => failure.as_ref(),
            ExecutionReportState::Finalized(report) => report.failure.as_ref(),
        }
    }
    /// Explicit provider session status when settlement was published.
    pub fn session_state(&self) -> &ProviderSessionState {
        match self.state.as_ref() {
            ExecutionReportState::Independent { session_state, .. } => session_state,
            ExecutionReportState::Finalized(report) => &report.session_state,
        }
    }
    /// Consumer projection retaining a known outcome alongside secondary failure.
    pub fn into_result(self) -> Result<ExecutionOutcome, AgentError> {
        let (provider_result, source, failure, session_state) = match *self.state {
            ExecutionReportState::Independent {
                provider_result,
                source,
                failure,
                session_state,
            } => (provider_result, source, failure, session_state),
            ExecutionReportState::Finalized(report) => {
                let failure = combine_failures(report.failure.clone(), report.physical_failure());
                return match (report.source, failure) {
                    (FinalizedExecutionSource::Provider(Some(result)), Some(error)) => {
                        Err(AgentError::ExecutionObservation {
                            error: Box::new(error),
                            execution_result: Some(Box::new(result)),
                        }
                        .bounded())
                    }
                    (FinalizedExecutionSource::Provider(None), Some(error)) => Err(error),
                    (FinalizedExecutionSource::Provider(Some(result)), None) => result,
                    (FinalizedExecutionSource::Provider(None), None) => {
                        Err(AgentError::SubmissionUnresolved)
                    }
                    (FinalizedExecutionSource::LocalCancellation, Some(error)) => {
                        Err(AgentError::ExecutionObservation {
                            error: Box::new(error),
                            execution_result: Some(Box::new(Ok(ExecutionOutcome::Cancelled))),
                        }
                        .bounded())
                    }
                    (FinalizedExecutionSource::LocalCancellation, None) => {
                        Ok(ExecutionOutcome::Cancelled)
                    }
                };
            }
        };
        let (cleanup_failure, cleanup_primary) = match session_state {
            ProviderSessionState::CleanupReported(report) => {
                let primary = report.operation_failure().cloned();
                (report.into_result().err(), primary)
            }
            _ => (None, None),
        };
        let failure = match (failure, cleanup_failure) {
            (Some(operation_error), Some(cleanup_error))
                if cleanup_primary.as_ref() == Some(&operation_error) =>
            {
                Some(cleanup_error)
            }
            (Some(operation_error), Some(cleanup_error)) if operation_error != cleanup_error => {
                Some(AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(operation_error),
                    cleanup_error: Box::new(cleanup_error),
                })
            }
            (first, second) => first.or(second),
        };
        match (provider_result, failure) {
            (Some(Err(primary)), Some(error)) if primary == error => Err(primary),
            (None, Some(error)) => Err(error),
            (result, Some(error)) => Err(AgentError::ExecutionObservation {
                error: Box::new(error),
                execution_result: result.map(Box::new),
            }),
            (Some(result), None) => result,
            (None, None) if source == ExecutionReportSource::LocalCancellation => {
                Ok(ExecutionOutcome::Cancelled)
            }
            (None, None) => Err(AgentError::SubmissionUnresolved),
        }
    }
}

fn combine_failures(first: Option<AgentError>, second: Option<AgentError>) -> Option<AgentError> {
    match (first, second) {
        (Some(first_error), Some(subsequent_error)) => {
            Some(AgentError::MultipleOperationFailures {
                first_error: Box::new(first_error),
                subsequent_error: Box::new(subsequent_error),
            })
        }
        (first, second) => first.or(second),
    }
}

/// Whether execution was rejected before dispatch or subsequently settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderExecutionReply {
    /// No input was dispatched and no provider observations are authorized.
    Rejected(AgentError),
    /// The adapter owned dispatch; settlement and resource status are explicit.
    Finished(ExecutionReport),
}
impl ProviderExecutionReply {
    /// Public consumer projection. Internal code retains the original report.
    pub fn into_result(self) -> Result<ExecutionOutcome, AgentError> {
        match self {
            Self::Rejected(error) => Err(error),
            Self::Finished(settlement) => settlement.into_result(),
        }
    }
}
/// Execution always distinguishes admission rejection from owned dispatch.
pub type ProviderExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = ProviderExecutionReply> + Send + 'a>>;

/// Cause observed at the failing reader boundary, independent of diagnostic wrappers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationFailureCause {
    /// The source observed expiry of its operation deadline.
    DeadlineExceeded,
    /// The source failed for a reason other than deadline expiry.
    ExecutionFailed,
}
/// Failed observation source. This does not confirm physical resource cleanup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationFailure {
    error: AgentError,
    cause: ObservationFailureCause,
}
impl ObservationFailure {
    /// Retain `cause` from the failing operation and independently bound its diagnostic.
    pub fn new(error: AgentError, cause: ObservationFailureCause) -> Self {
        Self {
            error: error.bounded(),
            cause,
        }
    }
    /// Bounded diagnostic detail; never used to infer the shutdown cause.
    pub fn error(&self) -> &AgentError {
        &self.error
    }
    /// Cause supplied by the failing source, before diagnostic projection.
    pub fn cause(&self) -> ObservationFailureCause {
        self.cause
    }
    /// Consumer diagnostic projection. Orchestration must retain the cause first.
    pub fn into_error(self) -> AgentError {
        self.error
    }
}
/// An observation wait retains its causal failure independently of diagnostics.
pub type ProviderObservationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ExecutionEvent>, ObservationFailure>> + Send + 'a>>;
