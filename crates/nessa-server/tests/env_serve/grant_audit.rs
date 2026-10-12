//! The owner's changes of the grants, audited: each one keeps a record of
//! who made it, when, to which peer, and the grant before and after, before
//! it takes effect; one that cannot be audited is not made.
use super::*;
use crate::env_serve::application::{change_grant, ChangeFailed, PeerGrants};
use crate::env_serve::domain::{AgentName, CommandGrant, Peer, PeerGrant};
use crate::env_serve::infrastructure::grants::Replace;
use crate::env_serve::infrastructure::FileGrants;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::PrivateTempFile;
use nessa_sdk::domain::agent_execution::leases::CommandWork;
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

/// What every record of the changes below holds, but its time and outcome.
fn record(at: u64, peer: &Peer, operation: Value, before: Value, after: Value) -> Value {
    json!({
        "schemaVersion": 1,
        "committedAtMs": at,
        "initiator": "unix:1000",
        "peer": peer_name(peer),
        "operation": operation,
        "before": before,
        "after": after,
    })
}

fn with(mut record: Value, outcome: &str, durable: Option<bool>) -> Value {
    record["outcome"] = json!(outcome);
    if let Some(durable) = durable {
        record["durable"] = json!(durable);
    }
    record
}

/// A grant and an ungrant each append `intended`, then `written`, with the
/// peer's whole grant before and after, who made it and when, to a private
/// journal; a change that leaves the grant as it was appends `unchanged`
/// alone.
#[test]
fn a_grant_writes_intended_then_written() {
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
    assert_eq!(granted.outcome, GrantOutcome::Written { durable: true });
    change_grant(
        &grants,
        &audit,
        &At(2_000),
        "unix:1000",
        gateway,
        GrantOperation::UngrantCommands,
    )
    .unwrap();
    change_grant(
        &grants,
        &audit,
        &At(3_000),
        "unix:1000",
        gateway,
        GrantOperation::UngrantCommands,
    )
    .unwrap();
    let full = json!({"agents": ["claude"], "commands": {"maxTimeoutMs": 60_000}});
    let grant = record(
        1_000,
        &gateway,
        json!({"op": "grant", "agents": ["claude"], "maxTimeoutMs": 60_000}),
        json!({}),
        full.clone(),
    );
    let ungrant = |at| {
        record(
            at,
            &gateway,
            json!({"op": "ungrant", "commands": true}),
            full.clone(),
            json!({"agents": ["claude"]}),
        )
    };
    let unchanged = record(
        3_000,
        &gateway,
        json!({"op": "ungrant", "commands": true}),
        json!({"agents": ["claude"]}),
        json!({"agents": ["claude"]}),
    );
    assert_eq!(
        records(directory.path()),
        [
            with(grant.clone(), "intended", None),
            with(grant, "written", Some(true)),
            with(ungrant(2_000), "intended", None),
            with(ungrant(2_000), "written", Some(true)),
            with(unchanged, "unchanged", None),
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
}

/// Replaces grants.json failing as a test says: the rename, or the
/// directory's sync after it.
struct Failing {
    rename: bool,
}
impl Replace for Failing {
    fn persist(&self, file: PrivateTempFile, destination: &Path) -> std::io::Result<()> {
        if self.rename {
            return Err(std::io::Error::other("the rename failed"));
        }
        file.persist(destination)
    }
    fn sync(&self, _directory: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("the sync failed"))
    }
}

/// A rename that fails leaves `intended` then `not_written`, with the
/// before and after the change was asked for, and the grant as it was.
#[test]
fn a_rename_failure_writes_intended_then_not_written() {
    let directory = private_directory();
    let grants = FileGrants::replacing(directory.path(), Box::new(Failing { rename: true }));
    let audit = FileGrantAudit::new(directory.path());
    let refused = change_grant(
        &grants,
        &audit,
        &At(5),
        "unix:1000",
        Peer::Ssh,
        GrantOperation::UngrantCommands,
    )
    .unwrap_err();
    assert!(
        matches!(&refused, ChangeFailed::Store(reason) if reason.contains("the rename failed")),
        "{refused:?}"
    );
    let asked = record(
        5,
        &Peer::Ssh,
        json!({"op": "ungrant", "commands": true}),
        json!({"agents": "*", "commands": {"maxTimeoutMs": CommandWork::MAX_TIMEOUT_MS}}),
        json!({"agents": "*"}),
    );
    assert_eq!(
        records(directory.path()),
        [
            with(asked.clone(), "intended", None),
            with(asked, "not_written", None),
        ]
    );
    assert_eq!(grants.entry(&Peer::Ssh), Ok(None));
    assert!(!directory.path().join("grants.json").exists());
}

/// A rename that succeeds and a directory sync that then fails: the change
/// is in force and recorded `written`, not durable, and the command fails
/// saying it is not yet durable.
#[test]
fn a_sync_failure_after_the_rename_writes_written_and_is_not_durable() {
    let directory = private_directory();
    let grants = FileGrants::replacing(directory.path(), Box::new(Failing { rename: false }));
    let audit = FileGrantAudit::new(directory.path());
    let refused = change_grant(
        &grants,
        &audit,
        &At(6),
        "unix:1000",
        Peer::Ssh,
        GrantOperation::UngrantAll,
    )
    .unwrap_err();
    assert!(
        matches!(&refused, ChangeFailed::NotDurable(reason) if reason.contains("the sync failed")),
        "{refused:?}"
    );
    let kept = records(directory.path());
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[0]["outcome"], "intended");
    assert_eq!(kept[1]["outcome"], "written");
    assert_eq!(kept[1]["durable"], false);
    assert_eq!(grants.entry(&Peer::Ssh), Ok(Some(PeerGrant::nothing())));
}

/// Keeps the first record it is given, and refuses every one after.
struct Once(std::sync::Mutex<usize>, FileGrantAudit);
impl GrantAudit for Once {
    fn record(&self, change: &GrantChange) -> Result<(), String> {
        let mut kept = self.0.lock().unwrap();
        if *kept > 0 {
            return Err("the disk filled".into());
        }
        *kept += 1;
        self.1.record(change)
    }
}

/// An outcome record that cannot be kept after a rename that succeeded: the
/// change is in force, the journal ends at its intent, and the command
/// fails saying the confirmation was not recorded.
#[test]
fn an_outcome_that_cannot_be_recorded_leaves_the_change_unconfirmed() {
    let directory = private_directory();
    let grants = FileGrants::new(directory.path());
    let audit = Once(Default::default(), FileGrantAudit::new(directory.path()));
    let refused = change_grant(
        &grants,
        &audit,
        &At(7),
        "unix:1000",
        Peer::Ssh,
        GrantOperation::UngrantAll,
    )
    .unwrap_err();
    assert_eq!(refused, ChangeFailed::Unconfirmed("the disk filled".into()));
    let kept = records(directory.path());
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["outcome"], "intended");
    assert_eq!(grants.entry(&Peer::Ssh), Ok(Some(PeerGrant::nothing())));
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
    assert_eq!(records(directory.path()).len(), recorded + 2);
}
