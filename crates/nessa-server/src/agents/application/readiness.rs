use crate::agents::application::ports::{AgentProbe, ProbeFailure};
use crate::agents::domain::{AgentId, HostAnswer, Readiness};

/// Report what stands between each agent and running.
///
/// This asks and maps; it does not decide. The two answers go to
/// `Readiness::from_host`, which owns what they mean together.
pub struct ReadAgentReadiness<'a> {
    /// The host being asked.
    pub probe: &'a dyn AgentProbe,
}

impl ReadAgentReadiness<'_> {
    /// Ask the host what it can about one agent and let the domain rule.
    ///
    /// An agent this server has nothing configured for is not asked about at
    /// all. Asking anyway would put two "this machine could not answer" lines
    /// in the log for every such agent on every check, about a machine that was
    /// never the problem. A future agent that needs no account would omit the
    /// second answer for the same reason: there would be no sign-in on this
    /// machine to find.
    pub fn execute(&self, agent: AgentId) -> Readiness {
        Readiness::from_host(self.probe.evidence(agent).map(|evidence| {
            (
                answer(agent, "installed", evidence.installed),
                evidence
                    .authenticated
                    .map(|answer_| answer(agent, "authenticated", answer_)),
            )
        }))
    }

    /// Every agent the server reports on, in listing order.
    pub fn all(&self) -> Vec<(AgentId, Readiness)> {
        AgentId::ALL
            .iter()
            .map(|agent| (*agent, self.execute(*agent)))
            .collect()
    }
}

/// Carry a probe failure across as "undetermined" rather than as a no, and say
/// so once where it happens. A question this machine could not answer is worth
/// a diagnostic line even though the person is still told something useful.
fn answer(agent: AgentId, question: &str, result: Result<bool, ProbeFailure>) -> HostAnswer {
    match result {
        Ok(true) => HostAnswer::Yes,
        Ok(false) => HostAnswer::No,
        Err(failure) => {
            tracing::debug!(
                ?agent,
                question,
                ?failure,
                "this machine could not answer about an agent"
            );
            HostAnswer::Undetermined
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/agents/readiness.rs"]
mod tests;
