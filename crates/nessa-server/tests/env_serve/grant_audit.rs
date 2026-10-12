//! The owner's changes of the grants, audited: each one keeps a record of
//! who made it, when, to which peer, and the grant before and after, before
//! it takes effect; one that cannot be audited is not made.
use super::*;
use crate::env_serve::application::{change_grant, ChangeFailed, PeerGrants};
use crate::env_serve::domain::{AgentName, CommandGrant, Peer, PeerGrant};
use crate::env_serve::infrastructure::FileGrants;
use nessa_auth::application::ports::Clock;
use std::os::unix::fs::PermissionsExt;

struct At(u64);
impl Clock for At {
    fn unix_milliseconds(&self) -> u64 {
        self.0
    }
}

struct Refusing;
impl GrantAudit for Refusing {
    fn record(&self, _change: &GrantChange) -> Result<(), String> {
        Err("the disk is full".into())
    }
}

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

fn records(directory: &Path) -> Vec<Value> {
    std::fs::read_to_string(directory.join(FILE))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn claude() -> Agents {
    Agents::Named([AgentName::new("claude").unwrap()].into())
}

/// A grant and an ungrant each append one record, with the peer's whole
/// grant before and after, who made it and when, to a private journal.
#[test]
fn a_grant_and_an_ungrant_each_append_one_record() {
    let directory = private_directory();
    let grants = FileGrants::new(directory.path());
    let audit = FileGrantAudit::new(directory.path());
    let gateway = Peer::Gateway { pin: [4; 44] };
    let granted = change_grant(
        &grants,
        &audit,
        &At(1_000),
        "unix:1000",
        gateway,
        GrantOperation::Grant {
            agents: Some(claude()),
            commands: Some(CommandGrant::new(60_000).unwrap()),
        },
    )
    .unwrap();
    assert_eq!(granted.outcome, GrantOutcome::Changed);
    change_grant(
        &grants,
        &audit,
        &At(2_000),
        "unix:1000",
        gateway,
        GrantOperation::UngrantCommands,
    )
    .unwrap();
    let kept = records(directory.path());
    assert_eq!(
        kept,
        [
            json!({
                "schemaVersion": 1,
                "committedAtMs": 1_000,
                "initiator": "unix:1000",
                "peer": peer_name(&gateway),
                "operation": {"op": "grant", "agents": ["claude"], "maxTimeoutMs": 60_000},
                "before": {},
                "after": {"agents": ["claude"], "commands": {"maxTimeoutMs": 60_000}},
                "outcome": "changed",
            }),
            json!({
                "schemaVersion": 1,
                "committedAtMs": 2_000,
                "initiator": "unix:1000",
                "peer": peer_name(&gateway),
                "operation": {"op": "ungrant", "commands": true},
                "before": {"agents": ["claude"], "commands": {"maxTimeoutMs": 60_000}},
                "after": {"agents": ["claude"]},
                "outcome": "changed",
            }),
        ]
    );
    assert_eq!(
        grants.entry(&gateway),
        Ok(Some(PeerGrant::new(claude(), None)))
    );
    let mode = std::fs::metadata(directory.path().join(FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0);
    // A change that leaves the grant as it was is kept too, as unchanged.
    change_grant(
        &grants,
        &audit,
        &At(3_000),
        "unix:1000",
        gateway,
        GrantOperation::UngrantCommands,
    )
    .unwrap();
    assert_eq!(records(directory.path())[2]["outcome"], "unchanged");
}

/// A change whose record cannot be kept is not made: grants.json stays
/// byte for byte as it was, and the command fails saying why.
#[test]
fn a_change_that_cannot_be_audited_leaves_the_grants_unchanged() {
    let directory = private_directory();
    let grants = FileGrants::new(directory.path());
    let audit = FileGrantAudit::new(directory.path());
    change_grant(
        &grants,
        &audit,
        &At(1),
        "unix:1000",
        Peer::Ssh,
        GrantOperation::UngrantAll,
    )
    .unwrap();
    let before = std::fs::read(directory.path().join("grants.json")).unwrap();
    let refused = change_grant(
        &grants,
        &Refusing,
        &At(2),
        "unix:1000",
        Peer::Ssh,
        GrantOperation::Grant {
            agents: Some(Agents::All),
            commands: None,
        },
    )
    .unwrap_err();
    assert_eq!(refused, ChangeFailed::Audit("the disk is full".into()));
    assert_eq!(
        std::fs::read(directory.path().join("grants.json")).unwrap(),
        before
    );
    assert_eq!(grants.entry(&Peer::Ssh), Ok(Some(PeerGrant::nothing())));
    // No temporary file is left beside it.
    let names: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        names
            .iter()
            .all(|name| ["grants.json", "grants.lock", FILE].contains(&name.as_str())),
        "{names:?}"
    );
    // Taking every agent by name from a grant of all is refused and not
    // audited.
    let recorded = records(directory.path()).len();
    change_grant(
        &grants,
        &audit,
        &At(3),
        "unix:1000",
        Peer::Ssh,
        GrantOperation::Grant {
            agents: Some(Agents::All),
            commands: None,
        },
    )
    .unwrap();
    assert_eq!(
        change_grant(
            &grants,
            &audit,
            &At(4),
            "unix:1000",
            Peer::Ssh,
            GrantOperation::UngrantAgent(AgentName::new("claude").unwrap()),
        )
        .unwrap_err(),
        ChangeFailed::AllAgents
    );
    assert_eq!(records(directory.path()).len(), recorded + 1);
}
