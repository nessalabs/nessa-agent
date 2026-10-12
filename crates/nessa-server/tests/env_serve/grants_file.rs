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

/// A peer has one spelling: `ssh`, or its pin in standard base64, read back
/// exactly; nothing else names one.
#[test]
fn a_peer_is_spelled_one_way_and_read_back() {
    assert_eq!(parse_peer("ssh"), Ok(Peer::Ssh));
    assert_eq!(peer_name(&Peer::Ssh), "ssh");
    let gateway = Peer::Gateway { pin: [7; 44] };
    assert_eq!(peer_name(&gateway).len(), 60);
    assert_eq!(parse_peer(&peer_name(&gateway)), Ok(gateway));
    let too_short = STANDARD.encode([7u8; 43]);
    let unpadded = peer_name(&gateway).trim_end_matches('=').to_owned();
    let spaced = format!("{} ", peer_name(&gateway));
    for refused in ["", "SSH", "peer:ab", "*", &too_short, &unpadded, &spaced] {
        assert_eq!(parse_peer(refused), Err(InvalidPeer), "{refused:?}");
    }
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
                peer_name(&gateway()): {"agents": ["claude"], "commands": {"maxTimeoutMs": 60000}},
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
            peer_name(&gateway())
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

/// A change that would make the file larger than every reader takes is
/// refused before anything is written: the file stays as it was, readable.
#[test]
fn a_change_past_the_bound_is_refused_and_writes_nothing() {
    let gateway = |n: usize| {
        let mut pin = [0; 44];
        pin[..2].copy_from_slice(&u16::try_from(n).unwrap().to_be_bytes());
        Peer::Gateway { pin }
    };
    let size = |peers: &BTreeMap<Peer, PeerGrant>| {
        serde_json::to_vec_pretty(&printed(peers)).unwrap().len() as u64 + 1
    };
    // As many peers as the bound holds, written as a change would write them.
    let mut peers = BTreeMap::new();
    let mut next = BTreeMap::new();
    while size(&next) <= MAX_BYTES {
        peers = next.clone();
        next.insert(gateway(next.len()), PeerGrant::new(Agents::All, None));
    }
    let directory = private_directory();
    let text = serde_json::to_string_pretty(&printed(&peers)).unwrap() + "\n";
    write_private(directory.path(), &text);
    let grants = FileGrants::new(directory.path());
    assert_eq!(grants.read().unwrap(), peers);
    let refused = grants
        .change(&gateway(peers.len()), |_| {
            Ok::<_, ()>(Some(PeerGrant::new(Agents::All, None)))
        })
        .unwrap_err();
    assert!(
        matches!(refused, Changed::Grants(GrantsError::TooLarge)),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join(FILE)).unwrap(),
        text
    );
    // Taking one away still works.
    grants.change(&gateway(0), |_| Ok::<_, ()>(None)).unwrap();
    assert_eq!(grants.read().unwrap().len(), peers.len() - 1);
}

/// A peer named twice is not this shape, in either order: the file is
/// unreadable, never read as the last of the two.
#[test]
fn a_peer_named_twice_is_refused_in_either_order() {
    let wide = r#"{"agents":"*","commands":{"maxTimeoutMs":3600000}}"#;
    let narrow = r#"{"agents":["claude"]}"#;
    for (first, second) in [(wide, narrow), (narrow, wide)] {
        for name in ["ssh".to_owned(), peer_name(&gateway())] {
            let directory = private_directory();
            write_private(
                directory.path(),
                &format!(r#"{{"schemaVersion":1,"peers":{{"{name}":{first},"{name}":{second}}}}}"#),
            );
            let grants = FileGrants::new(directory.path());
            let refused = grants.read().unwrap_err();
            assert!(matches!(refused, GrantsError::Unreadable(_)), "{refused}");
            assert!(refused.to_string().contains("more than once"), "{refused}");
            assert!(grants.entry(&Peer::Ssh).is_err());
        }
    }
}
