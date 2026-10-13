//! Peer gateways as a principal kind: who a peer enrollment names
//! (`docs/design/auth/peer-gateways.md`, row H1). What the kind can never hold
//! (rows H4 and I1) is the Cedar bundle's, tested in its adapter.
use nessa_auth::domain::{
    pairing::{is_peer_principal, peer_principal, ConsentClass, DeviceKey},
    PrincipalId,
};

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
    // Anyone holding only the id can tell a peer's principal from another.
    assert!(is_peer_principal(&peer));
    assert!(!is_peer_principal(&owner));
    assert_eq!(
        ConsentClass::parse("gateway-conversation-read"),
        Some(ConsentClass::DeviceRead)
    );
    assert_eq!(
        ConsentClass::parse("peer-gateway-conversation-read"),
        Some(ConsentClass::PeerRead)
    );
    assert_eq!(
        ConsentClass::parse("peer-gateway-environment-serve"),
        Some(ConsentClass::PeerEnvironment)
    );
    assert_eq!(ConsentClass::parse("gateway"), None);
    // An environment peer is the same principal as a reading peer of the
    // same key: one key, one principal, whatever it was enrolled for.
    assert_eq!(
        ConsentClass::PeerEnvironment
            .credential_principal(&owner, &key)
            .unwrap(),
        peer
    );
    assert!(ConsentClass::PeerEnvironment.is_peer());
    assert!(ConsentClass::PeerRead.is_peer());
    assert!(!ConsentClass::DeviceRead.is_peer());
}
