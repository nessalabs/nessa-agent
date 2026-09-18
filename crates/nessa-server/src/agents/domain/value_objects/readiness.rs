use crate::agents::domain::value_objects::HostAnswer;

/// What stands between an agent and running.
///
/// The states are kept apart because they call for different things from the
/// person: nothing, a sign-in, or an install. Collapsing them into
/// "unavailable" would be easier to produce and useless to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Installed and signed in. The only state that may be offered.
    Ready,
    /// Installed, but nothing here is signed in to it.
    NeedsAuthentication,
    /// The adapter is not installed beside this server.
    NotInstalled,
    /// Installed, but this machine would not say whether anything is signed in
    /// to it. Kept apart from a plain no so that a locked keychain is not
    /// reported as a fact about the person's account.
    AuthenticationUnknown,
}

impl Readiness {
    /// What two answers from the host make of an agent.
    ///
    /// Installation is decided first, because it is the reason signing in would
    /// not help. An agent this machine cannot locate is one this server cannot
    /// start, so an undetermined installation is reported as not installed —
    /// the same advice, honestly reached.
    pub fn from_host(installed: HostAnswer, authenticated: HostAnswer) -> Self {
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
