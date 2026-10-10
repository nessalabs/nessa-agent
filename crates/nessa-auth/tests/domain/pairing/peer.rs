//! Peer gateways as a principal kind: who a peer enrollment names, and what
//! that kind can never hold (`docs/design/auth/peer-gateways.md`, rows H1 and H4).
use nessa_auth::domain::{
    pairing::{peer_principal, ConsentClass, DeviceKey},
    Action, PrincipalId, PrincipalKind, CONVERSATION_AUTHORITY_ACTIONS,
};

/// Row H4: a gateway principal can hold the read grant and none of the
/// conversation authority's actions; the other kinds are not narrowed here.
#[test]
fn a_gateway_principal_can_hold_only_the_read_grant() {
    let action = |name: &str| Action::new(name).unwrap();
    assert!(PrincipalKind::Gateway.may_hold(&action("conversation.read")));
    for refused in [
        "conversation.write",
        "credential.manage",
        "server.read",
        "anything.else",
    ] {
        assert!(
            !PrincipalKind::Gateway.may_hold(&action(refused)),
            "{refused}"
        );
    }
    // Every action of the conversation authority is refused to a peer, by
    // name from the one list of them.
    assert!(!CONVERSATION_AUTHORITY_ACTIONS.is_empty());
    for authority in CONVERSATION_AUTHORITY_ACTIONS {
        assert!(
            !PrincipalKind::Gateway.may_hold(&action(authority)),
            "{authority}"
        );
    }
    for kind in [
        PrincipalKind::Human,
        PrincipalKind::Agent,
        PrincipalKind::Integration,
    ] {
        assert!(kind.may_hold(&action("conversation.write")));
    }
}

/// Row H1: a device's credential names the owner; a peer's names the gateway
/// principal of the key it pinned, the same for the same key and different
/// for another.
#[test]
fn the_class_decides_which_principal_a_credential_names() {
    let owner = PrincipalId::new("owner").unwrap();
    let key = DeviceKey::new([0xab; DeviceKey::LENGTH]);
    let other = DeviceKey::new([0x01; DeviceKey::LENGTH]);
    assert_eq!(
        ConsentClass::DeviceRead
            .credential_principal(&owner, &key)
            .unwrap(),
        owner
    );
    let peer = ConsentClass::PeerRead
        .credential_principal(&owner, &key)
        .unwrap();
    assert_eq!(peer, peer_principal(&key).unwrap());
    assert_eq!(peer.as_str(), format!("gateway:{}", "ab".repeat(32)));
    assert_ne!(peer, owner);
    assert_ne!(peer, peer_principal(&other).unwrap());
    assert_eq!(
        ConsentClass::parse("gateway-conversation-read"),
        Some(ConsentClass::DeviceRead)
    );
    assert_eq!(
        ConsentClass::parse("peer-gateway-conversation-read"),
        Some(ConsentClass::PeerRead)
    );
    assert_eq!(ConsentClass::parse("gateway"), None);
}
