use super::BackgroundRecord;
use crate::conversation::domain::{
    BackgroundCommand, BackgroundTaskId, TaskInitiator, TaskOutcome, TaskStop,
};
use nessa_protocol::conversation::domain::ConversationId;
use std::{future::Future, pin::Pin};
use tokio::sync::watch;

pub type BackgroundRun<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackgroundTaskError {
    Unavailable,
    InvalidInput,
    NotFound,
    Conflict,
    Capacity,
    Storage,
    UnsupportedEnvironment,
    UnobservedCall,
    CleanupUnverified,
    CleanupIncomplete(Vec<BackgroundCleanupFailure>),
}

/// Each failed cleanup attempt retains its original target and typed cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundCleanupFailure {
    pub conversation: Option<ConversationId>,
    pub task: Option<BackgroundTaskId>,
    pub cause: Box<BackgroundTaskError>,
}

/// Process effects, supplied at composition; independent of provider lifetimes.
pub trait BackgroundRunner: Send + Sync {
    fn run(
        &self,
        command: BackgroundCommand,
        workspace: String,
        stop: watch::Receiver<Option<TaskStop>>,
    ) -> BackgroundRun<'_, TaskOutcome>;
    fn shutdown(&self) -> BackgroundRun<'_, Result<(), BackgroundTaskError>>;
}

/// Current facts and transition audit commit atomically. A replacement must
/// advance the expected revision; retries cannot change the original source.
pub trait BackgroundStore: Send + Sync {
    fn read(
        &self,
        conversation: &ConversationId,
        task: &BackgroundTaskId,
    ) -> BackgroundRun<'_, Result<Option<BackgroundRecord>, BackgroundTaskError>>;
    fn save(
        &self,
        record: BackgroundRecord,
        previous: Option<u64>,
    ) -> BackgroundRun<'_, Result<(), BackgroundTaskError>>;
    fn forget_conversation(
        &self,
        conversation: &ConversationId,
        initiator: TaskInitiator,
    ) -> BackgroundRun<'_, Result<(), BackgroundTaskError>>;
    fn pending(&self) -> BackgroundRun<'_, Result<Vec<BackgroundRecord>, BackgroundTaskError>>;
    fn for_calls(
        &self,
        conversation: &ConversationId,
        calls: Vec<(String, String)>,
    ) -> BackgroundRun<'_, Result<Vec<BackgroundRecord>, BackgroundTaskError>>;
}

/// Decode a retained provider argument object at its representation boundary.
pub trait BackgroundCallMatcher: Send + Sync {
    fn matches(
        &self,
        arguments: &str,
        task: &BackgroundTaskId,
        command: &BackgroundCommand,
    ) -> bool;
}

impl BackgroundTaskError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::Conflict => "request_conflict",
            Self::Capacity => "capacity",
            Self::Storage => "storage",
            Self::UnsupportedEnvironment => "unsupported_environment",
            Self::UnobservedCall => "unobserved_call",
            Self::CleanupUnverified => "cleanup_unverified",
            Self::CleanupIncomplete(_) => "cleanup_incomplete",
        }
    }
}
