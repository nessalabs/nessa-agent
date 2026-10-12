//! The host's lease audit on disk: what accounting answers from it.
use super::*;
use crate::env_serve::application::EndCause;
use nessa_protocol::lease::Sandbox;

fn granted(lease: &str) -> LedgerEntry {
    LedgerEntry::Granted {
        lease: lease.into(),
        peer: crate::env_serve::domain::Peer::Ssh,
        agent: "claude".into(),
        sandbox: Sandbox::HarnessDefault,
        lease_ms: None,
    }
}

fn ended(lease: &str, forced: bool) -> LedgerEntry {
    LedgerEntry::Ended {
        lease: lease.into(),
        cleanup: Cleanup::Confirmed { forced },
        cause: EndCause::Asked,
    }
}

/// An end answers for the grant before it, never for one after it: a lease
/// granted again after its end is uncertain until it ends again, and a line
/// this build cannot read is skipped.
#[test]
fn accounting_answers_the_last_grant_s_end() {
    let directory = tempfile::tempdir().unwrap();
    let ledger = FileLedger::open(directory.path().join("leases.jsonl")).unwrap();
    assert_eq!(ledger.accounted("a").unwrap(), Cleanup::NotHeld);
    ledger.record(&granted("a")).unwrap();
    assert_eq!(ledger.accounted("a").unwrap(), Cleanup::Uncertain);
    ledger.record(&ended("a", true)).unwrap();
    assert_eq!(
        ledger.accounted("a").unwrap(),
        Cleanup::Confirmed { forced: true }
    );
    {
        let mut file = ledger.append.lock().unwrap();
        file.write_all(b"{not a line this build reads}\n").unwrap();
    }
    ledger.record(&granted("a")).unwrap();
    assert_eq!(ledger.accounted("a").unwrap(), Cleanup::Uncertain);
    ledger.record(&ended("a", false)).unwrap();
    assert_eq!(
        ledger.accounted("a").unwrap(),
        Cleanup::Confirmed { forced: false }
    );
    assert_eq!(ledger.accounted("b").unwrap(), Cleanup::NotHeld);
}

/// Each grant and refusal on disk names its peer, `ssh` or the dialed
/// gateway's pin as the grants file spells it, and a refusal what was
/// asked; a line written before those fields still accounts.
#[test]
fn grants_and_refusals_name_their_peer_and_old_lines_still_read() {
    use crate::env_serve::application::Asked;
    use crate::env_serve::domain::Peer;
    use crate::env_serve::infrastructure::peer_name;
    use nessa_protocol::lease::GrantRefusal;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("leases.jsonl");
    let ledger = FileLedger::open(path.clone()).unwrap();
    let dialed = Peer::Gateway { pin: [3; 44] };
    ledger.record(&granted("ssh-lease")).unwrap();
    ledger
        .record(&LedgerEntry::Refused {
            lease: "refused".into(),
            peer: dialed,
            reason: GrantRefusal::NotGranted,
            asked: Asked::Agent {
                agent: "codex".into(),
                sandbox: Sandbox::HarnessDefault,
            },
        })
        .unwrap();
    {
        let mut file = ledger.append.lock().unwrap();
        file.write_all(
            br#"{"kind":"granted","lease":"old","agent":"claude","sandbox":"harnessDefault"}
"#,
        )
        .unwrap();
    }
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["peer"], "ssh");
    assert_eq!(
        lines[1],
        serde_json::json!({
            "kind": "refused",
            "lease": "refused",
            "peer": peer_name(&dialed),
            "reason": "notGranted",
            "agent": "codex",
            "sandbox": "harnessDefault",
        })
    );
    assert_eq!(ledger.accounted("old").unwrap(), Cleanup::Uncertain);
    assert_eq!(ledger.accounted("refused").unwrap(), Cleanup::NotHeld);
}
