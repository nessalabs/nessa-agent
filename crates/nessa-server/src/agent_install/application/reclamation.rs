use std::fmt;

use crate::agent_install::domain::ReclamationEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclamationPersistenceStage {
    Read,
    RetainObligation,
    RetainAdmission,
    RetainOutcome,
    RetainAuditAcknowledgement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationPersistenceFailure {
    stage: ReclamationPersistenceStage,
    detail: String,
}

impl ReclamationPersistenceFailure {
    pub fn new(stage: ReclamationPersistenceStage, detail: String) -> Self {
        Self { stage, detail }
    }

    pub fn stage(&self) -> ReclamationPersistenceStage {
        self.stage
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ReclamationPersistenceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime reclamation persistence failed at {:?}: {}",
            self.stage, self.detail
        )
    }
}

impl std::error::Error for ReclamationPersistenceFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationAuditFailure {
    detail: String,
}

impl ReclamationAuditFailure {
    pub fn new(detail: String) -> Self {
        Self { detail }
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ReclamationAuditFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime reclamation audit failed: {}",
            self.detail
        )
    }
}

impl std::error::Error for ReclamationAuditFailure {}

pub trait ReclamationAudit: Send + Sync {
    fn record(&self, event: &ReclamationEvent) -> Result<(), ReclamationAuditFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationWarning {
    Persistence(ReclamationPersistenceFailure),
    Outcome(ReclamationEvent),
    Audit {
        event: ReclamationEvent,
        failure: ReclamationAuditFailure,
    },
}
