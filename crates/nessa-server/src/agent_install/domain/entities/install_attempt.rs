use std::fmt;

use crate::agent_install::domain::{
    AgentName, ArchiveDigest, InstallRequest, InstallTransition, InstallTransitionError,
    InstallTransitionKind, RecoveryFailureEvidence, RecoveryState, RollbackState, RuntimeArtifact,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttemptStage {
    Started,
    Verified,
    Finished,
}

/// The single owner of one install attempt's legal evidence sequence.
pub struct InstallAttempt {
    agent: AgentName,
    target: RuntimeArtifact,
    request: InstallRequest,
    stage: AttemptStage,
}

impl InstallAttempt {
    pub fn start(
        agent: AgentName,
        target: RuntimeArtifact,
        request: InstallRequest,
    ) -> (Self, InstallTransition) {
        let transition = InstallTransition::simple(
            InstallTransitionKind::Started,
            agent.clone(),
            target.clone(),
            request.clone(),
        );
        (
            Self {
                agent,
                target,
                request,
                stage: AttemptStage::Started,
            },
            transition,
        )
    }

    pub fn verified(&mut self) -> Result<InstallTransition, InstallAttemptError> {
        self.require(AttemptStage::Started)?;
        self.stage = AttemptStage::Verified;
        Ok(self.simple(InstallTransitionKind::Verified))
    }

    pub fn rejected(
        &mut self,
        actual: ArchiveDigest,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.require(AttemptStage::Started)?;
        let transition = InstallTransition::rejected(
            self.agent.clone(),
            self.target.clone(),
            actual,
            self.request.clone(),
        )?;
        self.stage = AttemptStage::Finished;
        Ok(transition)
    }

    pub fn installed(&mut self) -> Result<InstallTransition, InstallAttemptError> {
        self.require(AttemptStage::Verified)?;
        self.stage = AttemptStage::Finished;
        Ok(self.simple(InstallTransitionKind::Installed))
    }

    pub fn replaced(
        &mut self,
        previous: RuntimeArtifact,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.require(AttemptStage::Verified)?;
        let transition = InstallTransition::replaced(
            self.agent.clone(),
            previous,
            self.target.clone(),
            self.request.clone(),
        )?;
        self.stage = AttemptStage::Finished;
        Ok(transition)
    }

    pub fn rolled_back(
        &mut self,
        rollback: RollbackState,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.require(AttemptStage::Verified)?;
        let transition = InstallTransition::rolled_back(
            self.agent.clone(),
            self.target.clone(),
            rollback,
            self.request.clone(),
        )?;
        self.stage = AttemptStage::Finished;
        Ok(transition)
    }

    pub fn recovery_incomplete(
        &mut self,
        state: RecoveryState,
        failures: RecoveryFailureEvidence,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.require(AttemptStage::Verified)?;
        let transition = InstallTransition::recovery_incomplete(
            self.agent.clone(),
            self.target.clone(),
            state,
            failures,
            self.request.clone(),
        )?;
        self.stage = AttemptStage::Finished;
        Ok(transition)
    }

    pub fn reused(&mut self) -> Result<(), InstallAttemptError> {
        self.require(AttemptStage::Verified)?;
        self.stage = AttemptStage::Finished;
        Ok(())
    }

    fn simple(&self, kind: InstallTransitionKind) -> InstallTransition {
        InstallTransition::simple(
            kind,
            self.agent.clone(),
            self.target.clone(),
            self.request.clone(),
        )
    }

    fn require(&self, expected: AttemptStage) -> Result<(), InstallAttemptError> {
        (self.stage == expected)
            .then_some(())
            .ok_or(InstallAttemptError::WrongStage)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallAttemptError {
    WrongStage,
    Contradictory(InstallTransitionError),
}

impl From<InstallTransitionError> for InstallAttemptError {
    fn from(error: InstallTransitionError) -> Self {
        Self::Contradictory(error)
    }
}

impl fmt::Display for InstallAttemptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongStage => formatter.write_str("install transition is out of order"),
            Self::Contradictory(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InstallAttemptError {}

#[cfg(test)]
#[path = "../../../../tests/agent_install/install_attempt.rs"]
mod tests;
