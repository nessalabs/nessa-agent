//! What the use case asks the host, and what it hands the domain. The rules
//! themselves are the domain's, and are tested there.

use super::*;
use crate::agents_test_support::StubAgentProbe;

fn readiness(probe: StubAgentProbe) -> Readiness {
    ReadAgentReadiness { probe: &probe }.execute(AgentId::Claude)
}

#[test]
fn hands_the_two_answers_to_the_domain_rule() {
    assert_eq!(
        readiness(StubAgentProbe::answering(true, true)),
        Readiness::Ready
    );
    assert_eq!(
        readiness(StubAgentProbe::answering(true, false)),
        Readiness::NeedsAuthentication
    );
    assert_eq!(
        readiness(StubAgentProbe::answering(false, false)),
        Readiness::NotInstalled
    );
}

#[test]
fn a_question_the_host_could_not_answer_is_carried_across_as_undetermined() {
    // The use case must not turn a probe failure into a plain no on the way in.
    assert_eq!(
        readiness(StubAgentProbe {
            installed: Ok(true),
            authenticated: Err(ProbeFailure::Unanswered),
        }),
        Readiness::AuthenticationUnknown
    );
    assert_eq!(
        readiness(StubAgentProbe {
            installed: Err(ProbeFailure::NothingToAsk),
            authenticated: Ok(true),
        }),
        Readiness::NotInstalled
    );
}

#[test]
fn reports_every_agent_it_knows() {
    let probe = StubAgentProbe::answering(true, true);
    let all = ReadAgentReadiness { probe: &probe }.all();
    assert_eq!(all.len(), AgentId::ALL.len());
    assert_eq!(all[0], (AgentId::Claude, Readiness::Ready));
}

#[test]
fn an_agent_with_nothing_to_launch_is_not_asked_about_this_machine() {
    // Both other questions are about the machine, and neither was ever about
    // this agent: nothing would be launched for it. Asking anyway would put two
    // "could not answer" lines in the log on every check, about a machine that
    // was never the problem — and answering from them would tell someone who
    // has the agent to go and install it.
    struct NothingConfigured;
    impl AgentProbe for NothingConfigured {
        fn configured(&self, _: AgentId) -> bool {
            false
        }
        fn installed(&self, _: AgentId) -> Result<bool, ProbeFailure> {
            panic!("an unconfigured agent must not be asked about this machine")
        }
        fn authenticated(&self, _: AgentId) -> Result<bool, ProbeFailure> {
            panic!("an unconfigured agent must not be asked about this machine")
        }
    }
    assert_eq!(
        ReadAgentReadiness {
            probe: &NothingConfigured
        }
        .execute(AgentId::Claude),
        Readiness::NotConfigured
    );
}
