//! Stored input is resolved for one provider delivery, not rewritten in history.
#![deny(missing_docs)]
use crate::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{LinkedFile, UploadedFileReference},
};
use std::{future::Future, pin::Pin};

/// Why a stored input could not become a confirmed file link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserFileError {
    /// This execution environment offers no file preparation.
    NotOffered,
    /// The conversation no longer holds the exact file.
    NotHeld,
    /// Storage or transfer is unavailable.
    Unavailable,
    /// Content or prepared-reference correlation disagrees with the input.
    Mismatch,
    /// Another file occupies the destination.
    Occupied,
    /// The owning execution or lease stopped.
    Cancelled,
    /// Required delivery evidence could not be committed.
    Audit,
    /// Delivery may have placed bytes but confirmation is missing.
    Uncertain,
}
/// A confirmed link together with the original reference it resolves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedInputFile {
    reference: UploadedFileReference,
    file: LinkedFile,
}
impl PreparedInputFile {
    /// Correlate confirmed placement with its original input.
    pub fn new(reference: UploadedFileReference, file: LinkedFile) -> Self {
        Self { reference, file }
    }
    /// Original input, checked by the provider wrapper before dispatch.
    pub fn reference(&self) -> &UploadedFileReference {
        &self.reference
    }
    pub(crate) fn into_file(self) -> LinkedFile {
        self.file
    }
}
/// Preparation result for one dispatch. Dropping it cancels interest in delivery;
/// an adapter retains ownership of physical work and required evidence until settled.
pub type InputFileFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<Vec<PreparedInputFile>, super::super::agents::AgentError>>
            + Send
            + 'a,
    >,
>;
/// Injected preparation bound to one conversation and execution environment.
///
/// Resolve original references in order. Verify bytes before returning confirmed
/// links, and do not replace an occupied destination. The SDK preserves original
/// input in storage and verifies reference correlation on the returned links.
/// Implementations must bound transfers and finish required evidence independently
/// of a dropped consumer; preparation does not authorize provider dispatch.
pub trait InputFileResolver: Send + Sync {
    /// Prepare one execution's files, or return its typed failure. A retry of
    /// already-saved delivery is settled by the SDK before this boundary is used.
    fn resolve(
        &self,
        execution: ExecutionId,
        files: Vec<UploadedFileReference>,
    ) -> InputFileFuture<'_>;
}
