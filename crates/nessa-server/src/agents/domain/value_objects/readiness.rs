use crate::agents::domain::value_objects::HostAnswer;

/// What stands between an agent and running.
///
/// The states are kept apart because they call for different things from the
/// person: nothing, a sign-in, an install, or — where this build was never set
/// up for the agent — nothing they can do on this machine at all. Collapsing
/// them into "unavailable" would be easier to produce and useless to act on,
/// and collapsing the last into the third is worse than useless: it is an
/// instruction to install what may already be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Installed and signed in. The only state that may be offered.
    Ready,
    /// Installed, but nothing here is signed in to it.
    NeedsAuthentication,
    /// The adapter is not installed beside this server.
    NotInstalled,
    /// This server has nothing configured to launch for this agent.
    ///
    /// Apart from [`Self::NotInstalled`] because it is a fact about this
    /// installation and not about the machine. Reported as "not installed", a
    /// person with the agent already on their machine would be sent to install
    /// it again, and nothing would change when they did.
    NotConfigured,
    /// Installed, but this machine would not say whether anything is signed in
    /// to it. Kept apart from a plain no so that a locked keychain is not
    /// reported as a fact about the person's account.
    AuthenticationUnknown,
}

impl Readiness {
    /// What the host's answers make of an agent, where there was anything to
    /// ask about.
    ///
    /// `None` is an agent this server has no configuration to launch. It comes
    /// before the other two questions rather than as a third answer among them,
    /// because where nothing would be launched neither question is about this
    /// agent's presence on the machine — they are about a launch nobody
    /// configured.
    ///
    /// Otherwise installation is decided first, because it is the reason
    /// signing in would not help. An agent this machine cannot locate is one
    /// this server cannot start, so an undetermined installation is reported as
    /// not installed — the same advice, honestly reached.
    pub fn from_host(answers: Option<(HostAnswer, HostAnswer)>) -> Self {
        let Some((installed, authenticated)) = answers else {
            return Readiness::NotConfigured;
        };
        match (installed, authenticated) {
            (HostAnswer::No | HostAnswer::Undetermined, _) => Readiness::NotInstalled,
            (HostAnswer::Yes, HostAnswer::Yes) => Readiness::Ready,
            (HostAnswer::Yes, HostAnswer::No) => Readiness::NeedsAuthentication,
            (HostAnswer::Yes, HostAnswer::Undetermined) => Readiness::AuthenticationUnknown,
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/agents/domain.rs"]
mod tests;
