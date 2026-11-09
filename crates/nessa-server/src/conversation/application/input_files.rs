//! Original uploaded input becomes confirmed links under one execution lease.
//!
//! admission -> exact holds + submitted evidence -> durable SDK input
//! dispatch -> asked evidence -> local placement / LeaseHold::fetch -> outcome evidence
//! Physical work and its outcome outlive a dropped provider preparation waiter.
use super::{
    ConversationAttachments, ConversationFileLinkAudit, EnvironmentFuture, FetchAsk, FetchFailure,
    FetchPlaced, HeldRange, HeldRangeError, HeldSource, HeldSourceFailure, LeaseHold,
};
use futures_util::FutureExt;
use nessa_auth::{
    application::ports::Clock,
    domain::{OrganizationId, PrincipalId},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
        providers::{InputFileFuture, InputFileResolver, PreparedInputFile, UserFileError},
        sessions::PublishedFile,
    },
    domain::agent_execution::{
        executions::ExecutionId,
        leases::LeaseId,
        prompts::{LinkedFile, UploadedFileReference},
    },
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{watch, Semaphore};

/// Consequential stages of original-input naming and automatic delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputFileStage {
    Submitted,
    Asked,
    Confirmed,
    Failed,
    Cancelled,
    Uncertain,
}
/// Immutable input/delivery evidence. System delivery joins the sender's saved
/// submission by execution identity; it does not fabricate a tool call or person.
#[derive(Clone, Debug)]
pub struct InputFileAudit {
    pub conversation: ConversationId,
    pub organization: OrganizationId,
    pub execution: String,
    pub lease: Option<String>,
    pub attempt: String,
    pub file: UploadedFileReference,
    pub stage: InputFileStage,
    pub destination: Option<String>,
    pub delivered: bool,
    pub failure: Option<UserFileError>,
    pub initiator: Option<(PrincipalId, String)>,
    pub observed_at_ms: u64,
}

/// Placement and time are injected by composition, separately from upload storage.
pub(crate) struct InputFileDelivery {
    pub local: Arc<dyn LocalInputFilePlacement>,
    pub timer: Arc<dyn crate::timer::Timer>,
}
/// Local filesystem boundary, injected separately from upload storage.
/// Constructing a placement future performs no I/O; physical work stays owned
/// until completion even when its waiter disappears.
pub(crate) trait LocalInputFilePlacement: Send + Sync {
    fn place(
        &self,
        request: LocalInputFile,
    ) -> EnvironmentFuture<'static, Result<String, UserFileError>>;
}
// The request port remains shared; only Unix composition consumes its fields.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct LocalInputFile {
    pub workspace: String,
    pub file: UploadedFileReference,
    pub source: Arc<dyn HeldSource>,
    pub cancelled: Arc<AtomicBool>,
    pub delivered: Arc<AtomicBool>,
}

/// Audit writes own any physical persistence after their waiter is dropped.
/// A deadline prevents a failed audit adapter from retaining delivery capacity.
pub(super) async fn record_input(
    timer: &dyn crate::timer::Timer,
    audit: &dyn ConversationFileLinkAudit,
    record: InputFileAudit,
) -> Result<(), super::ConversationError> {
    crate::timer::within(
        timer,
        timer.now() + std::time::Duration::from_secs(10),
        AssertUnwindSafe(async { audit.record_input(record).await }).catch_unwind(),
    )
    .await
    .map_err(|_| super::ConversationError::Audit)?
    .unwrap_or(Err(super::ConversationError::Audit))
}

#[derive(Clone)]
pub(crate) struct ConversationInputFiles {
    attachments: Arc<dyn ConversationAttachments>,
    audit: Arc<dyn ConversationFileLinkAudit>,
    clock: Arc<dyn Clock>,
    organization: OrganizationId,
    principal: PrincipalId,
    conversation: ConversationId,
    lease: LeaseId,
    hold: Arc<dyn LeaseHold>,
    workspace: String,
    local: Option<Arc<dyn LocalInputFilePlacement>>,
    lease_stopped: watch::Receiver<bool>,
    slots: Arc<Semaphore>,
    timer: Arc<dyn crate::timer::Timer>,
}
impl ConversationInputFiles {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        attachments: Arc<dyn ConversationAttachments>,
        audit: Arc<dyn ConversationFileLinkAudit>,
        clock: Arc<dyn Clock>,
        organization: OrganizationId,
        principal: PrincipalId,
        conversation: ConversationId,
        lease: LeaseId,
        hold: Arc<dyn LeaseHold>,
        workspace: String,
        local: Option<Arc<dyn LocalInputFilePlacement>>,
        lease_stopped: watch::Receiver<bool>,
        slots: Arc<Semaphore>,
        timer: Arc<dyn crate::timer::Timer>,
    ) -> Self {
        Self {
            attachments,
            audit,
            clock,
            organization,
            principal,
            conversation,
            lease,
            hold,
            workspace,
            local,
            lease_stopped,
            slots,
            timer,
        }
    }
    async fn record(&self, record: InputFileAudit) -> Result<(), AgentError> {
        record_input(self.timer.as_ref(), self.audit.as_ref(), record)
            .await
            .map_err(|_| AgentError::UserFile(UserFileError::Audit))
    }
    async fn one(
        &self,
        execution: &ExecutionId,
        file: UploadedFileReference,
        mut stop: watch::Receiver<bool>,
    ) -> Result<PreparedInputFile, AgentError> {
        if *stop.borrow() || *self.lease_stopped.borrow() {
            return Err(AgentError::Closed);
        }
        if !crate::timer::within(
            self.timer.as_ref(),
            self.timer.now() + std::time::Duration::from_secs(10),
            self.attachments
                .holds_file(&self.organization, &self.conversation, &file),
        )
        .await
        .map_err(|_| AgentError::UserFile(UserFileError::Unavailable))?
        .map_err(|_| AgentError::UserFile(UserFileError::Unavailable))?
        {
            return Err(AgentError::UserFile(UserFileError::NotHeld));
        }
        let published = PublishedFile::new(file.digest(), file.media_type().clone(), file.size())
            .map_err(|_| AgentError::UserFile(UserFileError::Mismatch))?;
        let source: Arc<dyn HeldSource> = Arc::new(InputSource {
            attachments: self.attachments.clone(),
            organization: self.organization.clone(),
            principal: self.principal.as_str().into(),
            conversation: self.conversation.clone(),
            file: published.clone(),
            timer: self.timer.clone(),
        });
        let attempt = uuid::Uuid::new_v4().to_string();
        let delivered = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let audit = InputFileAudit {
            conversation: self.conversation.clone(),
            organization: self.organization.clone(),
            execution: execution.as_str().into(),
            lease: Some(self.lease.as_str().into()),
            attempt: attempt.clone(),
            file: file.clone(),
            stage: InputFileStage::Asked,
            destination: Some(file.destination_name()),
            delivered: false,
            failure: None,
            initiator: None,
            observed_at_ms: self.clock.unix_milliseconds(),
        };
        self.record(audit.clone()).await?;
        let local = self.local.is_some();
        let placement = catch_unwind(AssertUnwindSafe(
            || -> EnvironmentFuture<'static, Result<String, UserFileError>> {
                if *stop.borrow() || *self.lease_stopped.borrow() {
                    return Box::pin(async { Err(UserFileError::Cancelled) });
                }
                if let Some(placement) = &self.local {
                    placement.place(LocalInputFile {
                        workspace: self.workspace.clone(),
                        file: file.clone(),
                        source,
                        cancelled: cancelled.clone(),
                        delivered: delivered.clone(),
                    })
                } else {
                    match self.hold.fetch(FetchAsk {
                        request: attempt,
                        destination: file.destination_name(),
                        file: published,
                        source,
                        delivered: delivered.clone(),
                    }) {
                        Some(fetch) => Box::pin(async move {
                            fetch
                                .await
                                .map(|placed| match placed {
                                    FetchPlaced::Placed { path }
                                    | FetchPlaced::AlreadyPlaced { path } => path,
                                })
                                .map_err(fetch_error)
                        }),
                        None => Box::pin(async { Err(UserFileError::NotOffered) }),
                    }
                }
            },
        ))
        .unwrap_or_else(|_| Box::pin(async { Err(UserFileError::Uncertain) }));
        let start_stop = stop.clone();
        let start_lease = self.lease_stopped.clone();
        let guarded = async move {
            if *start_stop.borrow() || *start_lease.borrow() {
                return Err(UserFileError::Cancelled);
            }
            placement.await
        };
        let mut placement = Box::pin(
            AssertUnwindSafe(guarded)
                .catch_unwind()
                .map(|result| result.unwrap_or(Err(UserFileError::Uncertain))),
        );
        let mut lease_stop = self.lease_stopped.clone();
        let result = tokio::select! {
            biased;
            result = &mut placement => result,
            _ = async { tokio::select! { _ = stop.wait_for(|value| *value) => {}, _ = lease_stop.wait_for(|value| *value) => {} } } => {
                cancelled.store(true, Ordering::SeqCst);
                // A local blocking writer retains its directory and temp-file
                // authority until joined. Remote fetch has its own abandoned-transfer
                // cleanup and acknowledgement semantics.
                if local { (&mut placement).await } else { Err(if delivered.load(Ordering::SeqCst) { UserFileError::Uncertain } else { UserFileError::Cancelled }) }
            }

        };
        // Drop a remote transfer before recording its end, so its structural
        // abandonment runs even if the audit sink blocks or fails.
        drop(placement);
        let reported = result.as_ref().ok().cloned();
        let expected = std::path::Path::new(&self.workspace).join(file.destination_name());
        let result = result.and_then(|path| {
            if expected.to_str() == Some(path.as_str()) && LinkedFile::new(path.clone()).is_ok() {
                Ok(path)
            } else {
                Err(UserFileError::Mismatch)
            }
        });
        let failure = result.as_ref().err().copied();
        let stage = match &result {
            Ok(_) => InputFileStage::Confirmed,
            Err(UserFileError::Cancelled) => InputFileStage::Cancelled,
            Err(UserFileError::Uncertain) => InputFileStage::Uncertain,
            Err(_) => InputFileStage::Failed,
        };
        let destination = reported.or(audit.destination.clone());
        let ended = InputFileAudit {
            stage,
            destination,
            delivered: delivered.load(Ordering::SeqCst) || result.is_ok(),
            failure,
            observed_at_ms: self.clock.unix_milliseconds(),
            ..audit
        };
        if self.record(ended).await.is_err() {
            return Err(match failure {
                Some(error) => AgentError::MultipleOperationFailures {
                    first_error: Box::new(AgentError::UserFile(error)),
                    subsequent_error: Box::new(AgentError::UserFile(UserFileError::Audit)),
                },
                None => AgentError::UserFile(UserFileError::Audit),
            });
        }
        let path = result.map_err(AgentError::UserFile)?;
        if *stop.borrow() || *self.lease_stopped.borrow() {
            return Err(AgentError::Closed);
        }
        let linked =
            LinkedFile::new(path).map_err(|_| AgentError::UserFile(UserFileError::Mismatch))?;
        Ok(PreparedInputFile::new(file, linked))
    }
}
impl InputFileResolver for ConversationInputFiles {
    fn resolve(
        &self,
        execution: ExecutionId,
        files: Vec<UploadedFileReference>,
    ) -> InputFileFuture<'_> {
        let this = self.clone();
        Box::pin(async move {
            let permit = this
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| AgentError::Busy)?;
            let deadline = this.timer.sleep(std::time::Duration::from_secs(120));
            let (mut answer, response) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let _permit = permit;
                let (cancel, stopping) = watch::channel(answer.is_closed());
                let work = async {
                    let mut prepared = Vec::with_capacity(files.len());
                    for file in files {
                        prepared.push(this.one(&execution, file, stopping.clone()).await?);
                    }
                    Ok(prepared)
                };
                tokio::pin!(work);
                let result = tokio::select! {
                    biased;
                    result = &mut work => result,
                    _ = answer.closed() => { cancel.send_replace(true); work.await },
                };
                let _ = answer.send(result);
            });
            tokio::select! {
                biased;
                result = response => result.map_err(|_| AgentError::UserFile(UserFileError::Unavailable))?,
                () = deadline => Err(AgentError::UserFile(UserFileError::Uncertain)),
            }
        })
    }
}

struct InputSource {
    attachments: Arc<dyn ConversationAttachments>,
    organization: OrganizationId,
    principal: String,
    conversation: ConversationId,
    file: PublishedFile,
    timer: Arc<dyn crate::timer::Timer>,
}
impl HeldSource for InputSource {
    fn read(
        &self,
        offset: u64,
        max: usize,
    ) -> EnvironmentFuture<'_, Result<Vec<u8>, HeldSourceFailure>> {
        Box::pin(async move {
            crate::timer::within(
                self.timer.as_ref(),
                self.timer.now() + std::time::Duration::from_secs(10),
                self.attachments.read_held(HeldRange {
                    organization_id: self.organization.clone(),
                    principal_id: self.principal.clone(),
                    conversation_id: self.conversation.clone(),
                    file: self.file.clone(),
                    offset,
                    max_bytes: max.min(32 * 1024),
                }),
            )
            .await
            .map_err(|_| HeldSourceFailure::Unavailable)?
            .map_err(|error| match error {
                HeldRangeError::NotHeld => HeldSourceFailure::NotHeld,
                HeldRangeError::Busy | HeldRangeError::Unavailable => {
                    HeldSourceFailure::Unavailable
                }
            })
        })
    }
}
fn fetch_error(error: FetchFailure) -> UserFileError {
    match error {
        FetchFailure::NotHeld => UserFileError::NotHeld,
        FetchFailure::SourceUnavailable | FetchFailure::ChannelUnavailable => {
            UserFileError::Unavailable
        }
        FetchFailure::Unanswered => UserFileError::Uncertain,
        FetchFailure::Refused(reason) => match reason {
            nessa_protocol::lease::FetchRefusal::Unavailable => UserFileError::NotOffered,
            nessa_protocol::lease::FetchRefusal::Occupied => UserFileError::Occupied,
            nessa_protocol::lease::FetchRefusal::Mismatch
            | nessa_protocol::lease::FetchRefusal::Invalid
            | nessa_protocol::lease::FetchRefusal::Size
            | nessa_protocol::lease::FetchRefusal::OutsideWorkspace => UserFileError::Mismatch,
            nessa_protocol::lease::FetchRefusal::AuditUnavailable => UserFileError::Audit,
            nessa_protocol::lease::FetchRefusal::LeaseEnded => UserFileError::Cancelled,
            nessa_protocol::lease::FetchRefusal::NoDirectory
            | nessa_protocol::lease::FetchRefusal::Busy
            | nessa_protocol::lease::FetchRefusal::PlacingFailed => UserFileError::Unavailable,
        },
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/input_files.rs"]
pub(in crate::conversation::application) mod tests;
