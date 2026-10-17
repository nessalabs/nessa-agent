//! Read-only access to the live aggregate's existing pending review collection.
#![deny(missing_docs)]
use super::value_objects::review_content::fresh_key;
use super::{
    PermissionId, PermissionOptions, PermissionRequest, PermissionStateView, ReviewContent,
};
use crate::domain::agent_execution::questions::{AgentQuestion, QuestionId};
use crate::domain::agent_execution::tools::ToolCallId;
use crate::domain::agent_execution::{executions::ExecutionId, sessions::ExecutionSessionId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

pub(crate) type PendingPermissions = Arc<Mutex<HashMap<PermissionId, PermissionRequest>>>;
/// The questions an execution's agent is waiting on, as the adapter that
/// holds each lists it (`QuestionListing`).
pub(crate) type OpenQuestions = Arc<Mutex<HashMap<QuestionId, PendingQuestion>>>;

/// One question the agent is waiting on, as a person is shown it: its
/// identities, a key of its own, and exactly what it asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingQuestion {
    /// The execution that waits on it.
    pub execution_id: ExecutionId,
    /// The question's identity within its execution.
    pub id: QuestionId,
    /// What it asks, exactly.
    pub question: Arc<AgentQuestion>,
    key: u128,
    opened: u64,
}
impl PendingQuestion {
    /// A question opened now, under a key of its own.
    pub fn new(execution_id: ExecutionId, id: QuestionId, question: AgentQuestion) -> Self {
        let (key, opened) = fresh_key();
        Self {
            execution_id,
            id,
            question: Arc::new(question),
            key,
            opened,
        }
    }
    /// This question's own key, as 32 lowercase hex digits.
    pub fn key(&self) -> String {
        format!("{:032x}", self.key)
    }
    /// When it was opened, relative to every review and question of this process.
    pub fn opened(&self) -> u64 {
        self.opened
    }
}

/// The handle through which the adapter holding a question lists it while it
/// waits, and takes it out once it is answered, withdrawn or ended. Weak: once
/// the execution settles nothing it listed is listed.
#[derive(Clone, Debug)]
pub(crate) struct QuestionListing(Weak<Mutex<HashMap<QuestionId, PendingQuestion>>>);
impl QuestionListing {
    pub(crate) fn new(open: &OpenQuestions) -> Self {
        Self(Arc::downgrade(open))
    }
    pub(crate) fn list(&self, question: PendingQuestion) {
        if let Some(open) = self.0.upgrade() {
            if let Ok(mut open) = open.lock() {
                open.insert(question.id.clone(), question);
            }
        }
    }
    /// The key question `id` is listed under, while it is listed.
    pub(crate) fn key_of(&self, id: &QuestionId) -> Option<String> {
        let open = self.0.upgrade()?;
        let open = open.lock().ok()?;
        open.get(id).map(PendingQuestion::key)
    }
    pub(crate) fn unlist(&self, id: &QuestionId) {
        if let Some(open) = self.0.upgrade() {
            if let Ok(mut open) = open.lock() {
                open.remove(id);
            }
        }
    }
}

/// Failure to query a live review owner. These faults do not change durable history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionAuthorityError {
    /// The owning execution has ended or its aggregate was dropped.
    Unavailable,
    /// An unexpected panic poisoned the owning collection.
    Poisoned,
    /// A backend supplied a handle for another session or execution.
    IdentityMismatch,
}
/// A weak read handle issued by one live session aggregate for one execution.
/// It retains identities, never pending requests or authority after owner drop.
#[derive(Clone, Debug)]
pub struct PermissionAuthority {
    session: ExecutionSessionId,
    execution: ExecutionId,
    pending: Weak<Mutex<HashMap<PermissionId, PermissionRequest>>>,
    questions: Weak<Mutex<HashMap<QuestionId, PendingQuestion>>>,
}
impl PermissionAuthority {
    pub(crate) fn allocation_bytes(&self) -> usize {
        self.session
            .as_str()
            .len()
            .saturating_add(self.execution.as_str().len())
    }

    pub(crate) fn new(
        session: ExecutionSessionId,
        execution: ExecutionId,
        pending: &PendingPermissions,
        questions: &OpenQuestions,
    ) -> Self {
        Self {
            session,
            execution,
            pending: Arc::downgrade(pending),
            questions: Arc::downgrade(questions),
        }
    }
    /// Provider context that issued this handle.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session
    }
    /// Exact execution whose pending collection this handle reads.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution
    }
    /// Whether this review remains in the owning domain collection now.
    /// This does not reserve an answer; the command must still validate its owner.
    ///
    /// # Errors
    /// Reports dropped ownership or a poisoned collection without awaiting I/O.
    pub fn pending(&self, id: &PermissionId) -> Result<bool, PermissionAuthorityError> {
        let pending = self
            .pending
            .upgrade()
            .ok_or(PermissionAuthorityError::Unavailable)?;
        let pending = pending
            .lock()
            .map_err(|_| PermissionAuthorityError::Poisoned)?;
        Ok(pending.contains_key(id))
    }
}

/// One review still waiting in the owning collection, as a person is shown
/// it: its identities, its offered choices and its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReview {
    /// The execution that waits on it.
    pub execution_id: ExecutionId,
    /// The review's identity within its execution.
    pub id: PermissionId,
    /// The tool whose input is reviewed.
    pub tool_id: ToolCallId,
    /// The choices it offers, exactly.
    pub options: PermissionOptions,
    /// What a person is shown of it.
    pub content: Arc<ReviewContent>,
}
impl PermissionAuthority {
    /// Every review still waiting whose content was recorded, oldest first.
    /// This reserves nothing; an answer must still validate its owner.
    ///
    /// # Errors
    /// Reports dropped ownership or a poisoned collection without awaiting I/O.
    pub fn reviews(&self) -> Result<Vec<PendingReview>, PermissionAuthorityError> {
        let pending = self
            .pending
            .upgrade()
            .ok_or(PermissionAuthorityError::Unavailable)?;
        let pending = pending
            .lock()
            .map_err(|_| PermissionAuthorityError::Poisoned)?;
        let mut reviews: Vec<PendingReview> = pending
            .values()
            .filter(|request| matches!(request.state(), PermissionStateView::Pending))
            .filter_map(|request| {
                Some(PendingReview {
                    execution_id: self.execution.clone(),
                    id: request.id().clone(),
                    tool_id: request.tool_id().clone(),
                    options: request.options().clone(),
                    content: request.content()?.clone(),
                })
            })
            .collect();
        reviews.sort_by_key(|review| review.content.opened());
        Ok(reviews)
    }
    /// Every question the agent is still waiting on, oldest first. This
    /// reserves nothing; an answer must still validate its owner.
    ///
    /// # Errors
    /// Reports dropped ownership or a poisoned collection without awaiting I/O.
    pub fn questions(&self) -> Result<Vec<PendingQuestion>, PermissionAuthorityError> {
        let open = self
            .questions
            .upgrade()
            .ok_or(PermissionAuthorityError::Unavailable)?;
        let open = open
            .lock()
            .map_err(|_| PermissionAuthorityError::Poisoned)?;
        let mut questions: Vec<PendingQuestion> = open.values().cloned().collect();
        questions.sort_by_key(PendingQuestion::opened);
        Ok(questions)
    }
}

#[cfg(test)]
#[path = "../../../../tests/domain/agent_execution/permissions/authority.rs"]
mod tests;
