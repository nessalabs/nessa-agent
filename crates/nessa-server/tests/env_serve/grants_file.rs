//! The grants on disk: what is read from them, what a change writes, and
//! that a file this build cannot read is an error, never no grants.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

fn write_private(directory: &Path, text: &str) {
    let path = directory.join(FILE);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn gateway() -> Peer {
    Peer::Gateway { pin: [9; 44] }
}

/// No file is no entries: every peer has its defaults.
#[test]
fn an_absent_file_is_no_entries() {
    let directory = private_directory();
    let grants = FileGrants::new(directory.path());
    assert!(grants.read().unwrap().is_empty());
    assert_eq!(grants.entry(&Peer::Ssh), Ok(None));
    assert_eq!(grants.entry(&gateway()), Ok(None));
}

/// What a change writes is read back as it was given, in the file's
/// versioned shape, private to this account.
#[test]
fn a_change_is_written_privately_and_read_back() {
    let directory = private_directory();
    let grants = FileGrants::new(directory.path());
    let claude = Agents::Named([AgentName::new("claude").unwrap()].into());
    let entry = PeerGrant::new(claude, Some(CommandGrant::new(60_000).unwrap()));
    grants
        .change(&gateway(), |old| {
            assert_eq!(old, None);
            Ok::<_, ()>(Some(entry.clone()))
        })
        .unwrap();
    grants
        .change(&Peer::Ssh, |_| Ok::<_, ()>(Some(PeerGrant::nothing())))
        .unwrap();
    assert_eq!(grants.entry(&gateway()), Ok(Some(entry.clone())));
    assert_eq!(grants.entry(&Peer::Ssh), Ok(Some(PeerGrant::nothing())));
    let text = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
    let written: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "schemaVersion": 1,
            "peers": {
                "ssh": {},
                gateway().name(): {"agents": ["claude"], "commands": {"maxTimeoutMs": 60000}},
            }
        })
    );
    let mode = std::fs::metadata(directory.path().join(FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0);
    // A refused change writes nothing; removing an entry removes it.
    assert!(matches!(
        grants.change(&gateway(), |_| Err::<Option<PeerGrant>, _>("no")),
        Err(Changed::Refused("no"))
    ));
    assert_eq!(grants.entry(&gateway()), Ok(Some(entry)));
    grants.change(&gateway(), |_| Ok::<_, ()>(None)).unwrap();
    assert_eq!(grants.entry(&gateway()), Ok(None));
}

/// `"*"` is every agent, and a missing `agents` or `commands` is none.
#[test]
fn every_agent_and_missing_fields_are_read_as_written() {
    let directory = private_directory();
    write_private(
        directory.path(),
        &format!(
            r#"{{"schemaVersion":1,"peers":{{"ssh":{{"agents":"*"}},"{}":{{"commands":{{"maxTimeoutMs":5}}}}}}}}"#,
            gateway().name()
        ),
    );
    let grants = FileGrants::new(directory.path());
    assert_eq!(
        grants.entry(&Peer::Ssh),
        Ok(Some(PeerGrant::new(Agents::All, None)))
    );
    assert_eq!(
        grants.entry(&gateway()),
        Ok(Some(PeerGrant::new(
            Agents::Named(Default::default()),
            Some(CommandGrant::new(5).unwrap())
        )))
    );
}

/// Another schema version, a file past the bound, one not private, or one
/// not of this shape is unreadable: refused, never taken as no grants.
#[test]
fn a_file_this_build_cannot_read_is_refused() {
    let large = format!(
        r#"{{"schemaVersion":1,"peers":{{}},"pad":"{}"}}"#,
        "x".repeat(MAX_BYTES as usize)
    );
    for text in [
        r#"{"schemaVersion":2,"peers":{}}"#,
        r#"{"peers":{}}"#,
        r#"{"schemaVersion":1,"peers":{"ssh":{"agents":"all"}}}"#,
        r#"{"schemaVersion":1,"peers":{"ssh":{"agents":["*"]}}}"#,
        r#"{"schemaVersion":1,"peers":{"ssh":{"commands":{"maxTimeoutMs":0}}}}"#,
        r#"{"schemaVersion":1,"peers":{"somebody":{}}}"#,
        r#"{"schemaVersion":1,"peers":{"ssh":{"agent":["claude"]}}}"#,
        "not json",
        &large,
    ] {
        let directory = private_directory();
        write_private(directory.path(), text);
        let grants = FileGrants::new(directory.path());
        assert!(
            matches!(grants.read(), Err(GrantsError::Unreadable(_))),
            "{text}"
        );
        assert!(grants.entry(&Peer::Ssh).is_err(), "{text}");
        // A change is not made over it either.
        assert!(matches!(
            grants.change(&Peer::Ssh, |_| Ok::<_, ()>(None)),
            Err(Changed::Grants(GrantsError::Unreadable(_)))
        ));
    }
    let version = private_directory();
    write_private(version.path(), r#"{"schemaVersion":2,"peers":{}}"#);
    let refused = FileGrants::new(version.path()).read().unwrap_err();
    assert!(
        refused.to_string().contains("schema version 2"),
        "{refused}"
    );
    let shared = private_directory();
    write_private(shared.path(), r#"{"schemaVersion":1,"peers":{}}"#);
    std::fs::set_permissions(
        shared.path().join(FILE),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(matches!(
        FileGrants::new(shared.path()).read(),
        Err(GrantsError::Unreadable(_))
    ));
}
