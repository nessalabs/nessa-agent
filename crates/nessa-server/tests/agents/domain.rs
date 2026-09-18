//! What the host's answers make of an agent. No probe, no transport.

use super::*;

#[test]
fn offers_an_agent_only_when_it_is_installed_and_signed_in() {
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::Yes, HostAnswer::Yes))),
        Readiness::Ready
    );
}

#[test]
fn an_installed_agent_nobody_is_signed_in_to_asks_for_a_sign_in() {
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::Yes, HostAnswer::No))),
        Readiness::NeedsAuthentication
    );
}

#[test]
fn a_missing_agent_says_so_rather_than_asking_for_a_sign_in() {
    // Signing in would not help, so it must not be what the person is told.
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::No, HostAnswer::No))),
        Readiness::NotInstalled
    );
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::No, HostAnswer::Yes))),
        Readiness::NotInstalled
    );
}

#[test]
fn an_agent_this_machine_could_not_find_is_one_it_cannot_start() {
    // An undetermined install reaches the same advice as a missing one, but by
    // a rule that is written down rather than by a probe returning false.
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::Undetermined, HostAnswer::Yes))),
        Readiness::NotInstalled
    );
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::Undetermined, HostAnswer::Undetermined))),
        Readiness::NotInstalled
    );
}

#[test]
fn a_sign_in_the_machine_would_not_confirm_is_not_reported_as_absent() {
    // A locked keychain is not a fact about the person's account, and must not
    // arrive at the same state as one.
    assert_eq!(
        Readiness::from_host(Some((HostAnswer::Yes, HostAnswer::Undetermined))),
        Readiness::AuthenticationUnknown
    );
    assert_ne!(
        Readiness::from_host(Some((HostAnswer::Yes, HostAnswer::Undetermined))),
        Readiness::from_host(Some((HostAnswer::Yes, HostAnswer::No)))
    );
}

#[test]
fn an_agent_this_server_was_never_set_up_for_is_not_one_to_go_and_install() {
    // The two questions are about this machine. Where nothing would be
    // launched, neither of them was ever about this agent, and answering "not
    // installed" would send someone who already has it to install it again.
    assert_eq!(Readiness::from_host(None), Readiness::NotConfigured);
    assert_ne!(Readiness::from_host(None), Readiness::NotInstalled);
}
