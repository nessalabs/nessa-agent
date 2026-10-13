//! Admission asks the lease policy over the facts as read, and attaches the
//! terms to its yes; grants that cannot be read refuse without asking.
use super::*;
use crate::env_serve::domain::{Agents, CommandGrant, MaxLease};
use nessa_sdk::domain::agent_execution::leases::CommandWork;
use std::sync::Mutex;

/// Grants as a test sets them: an entry, none, or unreadable.
struct Grants(Result<Option<PeerGrant>, String>);
impl PeerGrants for Grants {
    fn entry(&self, _peer: &Peer) -> Result<Option<PeerGrant>, String> {
        self.0.clone()
    }
}

/// A policy answering `allow`, keeping what it was asked.
struct Answering {
    allow: bool,
    asked: Mutex<Vec<(Peer, bool, PeerGrant, String)>>,
}
impl Answering {
    fn new(allow: bool) -> Self {
        Self {
            allow,
            asked: Mutex::new(Vec::new()),
        }
    }
}
impl LeasePolicyEvaluator for Answering {
    fn evaluate(&self, facts: &LeaseFacts<'_>) -> LeaseDecision {
        self.asked.lock().unwrap().push((
            *facts.peer,
            facts.written,
            facts.grant.clone(),
            format!("{:?}", facts.ask),
        ));
        if self.allow {
            LeaseDecision::Allow { policies: vec![] }
        } else {
            LeaseDecision::Deny(Denial::NotPermitted)
        }
    }
}

const DIALED: Peer = Peer::Gateway { pin: [3; 44] };

/// The policy is asked with the peer, its entry as written, and what was
/// asked; with no entry, the grant `unwritten` says it holds. Its refusal
/// is `NotGranted`.
#[test]
fn the_policy_decides_over_the_facts_as_read() {
    let entry = PeerGrant::new(Agents::All, None);
    let grants = Grants(Ok(Some(entry.clone())));
    let deny = Answering::new(false);
    assert_eq!(
        admit_agent(&grants, &deny, &DIALED, "claude"),
        Err(NotGranted)
    );
    assert_eq!(
        admit_command(&grants, &deny, &DIALED, 1_000),
        Err(NotGranted)
    );
    assert_eq!(
        *deny.asked.lock().unwrap(),
        vec![
            (DIALED, true, entry.clone(), "Agent(\"claude\")".to_owned()),
            (DIALED, true, entry, "Command".to_owned()),
        ]
    );
    let none = Grants(Ok(None));
    let allow = Answering::new(true);
    assert_eq!(admit_agent(&none, &allow, &Peer::Ssh, "codex"), Ok(None));
    assert_eq!(admit_agent(&none, &allow, &DIALED, "codex"), Ok(None));
    let asked = allow.asked.lock().unwrap();
    assert_eq!((asked[0].1, &asked[0].2), (false, &unwritten(&Peer::Ssh)));
    assert_eq!((asked[1].1, &asked[1].2), (false, &unwritten(&DIALED)));
}

/// A yes carries the terms: the entry's lease limit, and a command's
/// timeout narrowed to the entry's longest; with no entry, the unwritten
/// grant's.
#[test]
fn a_yes_carries_the_terms() {
    let allow = Answering::new(true);
    let entry = PeerGrant::new(Agents::All, Some(CommandGrant::new(60_000).unwrap()))
        .with_max_lease(MaxLease::new(5_000).unwrap());
    let grants = Grants(Ok(Some(entry)));
    assert_eq!(
        admit_agent(&grants, &allow, &DIALED, "claude"),
        Ok(Some(5_000))
    );
    assert_eq!(admit_command(&grants, &allow, &DIALED, 120_000), Ok(60_000));
    assert_eq!(admit_command(&grants, &allow, &DIALED, 1_000), Ok(1_000));
    let none = Grants(Ok(None));
    assert_eq!(
        admit_command(&none, &allow, &Peer::Ssh, CommandWork::MAX_TIMEOUT_MS),
        Ok(CommandWork::MAX_TIMEOUT_MS)
    );
    // A yes with no command terms to run on is still refused.
    let no_commands = Grants(Ok(Some(PeerGrant::new(Agents::All, None))));
    assert_eq!(
        admit_command(&no_commands, &allow, &DIALED, 1_000),
        Err(NotGranted)
    );
}

/// Grants that cannot be read grant nothing, to SSH as to a dialed
/// gateway, and the policy is not asked.
#[test]
fn unreadable_grants_refuse_without_asking() {
    let allow = Answering::new(true);
    let broken = Grants(Err("not JSON".into()));
    for peer in [Peer::Ssh, DIALED] {
        assert_eq!(
            admit_agent(&broken, &allow, &peer, "claude"),
            Err(NotGranted)
        );
        assert_eq!(
            admit_command(&broken, &allow, &peer, 1_000),
            Err(NotGranted)
        );
    }
    assert!(allow.asked.lock().unwrap().is_empty());
}
