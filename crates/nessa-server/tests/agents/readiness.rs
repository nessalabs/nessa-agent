//! What the host is asked, and what that makes of an agent.

use super::*;
use crate::agents::domain::Readiness;

/// A host that answers whatever the test says it does.
struct Answers {
    installed: bool,
    authenticated: bool,
}

impl AgentProbe for Answers {
    fn installed(&self, _agent: AgentId) -> bool {
        self.installed
    }
    fn authenticated(&self, _agent: AgentId) -> bool {
        self.authenticated
    }
}

fn readiness(installed: bool, authenticated: bool) -> Readiness {
    let probe = Answers {
        installed,
        authenticated,
    };
    ReadAgentReadiness { probe: &probe }.execute(AgentId::Claude)
}

#[test]
fn offers_an_agent_only_when_it_is_installed_and_signed_in() {
    assert_eq!(readiness(true, true), Readiness::Ready);
}

#[test]
fn an_installed_agent_nobody_is_signed_in_to_asks_for_a_sign_in() {
    assert_eq!(readiness(true, false), Readiness::NeedsAuthentication);
}

#[test]
fn a_missing_agent_says_so_rather_than_asking_for_a_sign_in() {
    // Signing in would not help, so it must not be what the person is told.
    assert_eq!(readiness(false, false), Readiness::NotInstalled);
    assert_eq!(readiness(false, true), Readiness::NotInstalled);
}

#[test]
fn reports_every_agent_it_knows() {
    let probe = Answers {
        installed: true,
        authenticated: true,
    };
    let all = ReadAgentReadiness { probe: &probe }.all();
    assert_eq!(all.len(), AgentId::ALL.len());
    assert_eq!(all[0], (AgentId::Claude, Readiness::Ready));
}

#[test]
fn names_are_what_the_wire_and_the_interface_use() {
    assert_eq!(AgentId::Claude.as_str(), "claude");
    assert_eq!(Readiness::Ready.as_str(), "ready");
    assert_eq!(
        Readiness::NeedsAuthentication.as_str(),
        "needs-authentication"
    );
    assert_eq!(Readiness::NotInstalled.as_str(), "not-installed");
}
