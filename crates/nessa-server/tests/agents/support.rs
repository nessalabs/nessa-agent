//! A host that answers whatever the test says it does, so that no test asks the
//! developer's own machine about their credentials.

use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;

/// A host with both of its answers fixed in advance, including the answer that
/// it could not answer.
pub(crate) struct StubAgentProbe {
    pub(crate) installed: Result<bool, ProbeFailure>,
    pub(crate) authenticated: Result<bool, ProbeFailure>,
}

impl StubAgentProbe {
    /// A host that answers both questions plainly.
    pub(crate) fn answering(installed: bool, authenticated: bool) -> Self {
        Self {
            installed: Ok(installed),
            authenticated: Ok(authenticated),
        }
    }
}

impl AgentProbe for StubAgentProbe {
    fn installed(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.installed
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.authenticated
    }
}
