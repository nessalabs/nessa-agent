//! Persisted enrollment eligibility and retry selection, used by the LAN scheduler.
use super::*;

fn record() -> PeerRecord {
    let key = DeviceKey::new([8; 32]);
    let mut pin = [0; 44];
    pin[12..].copy_from_slice(key.bytes());
    PeerRecord {
        key,
        pin,
        address: "127.0.0.1:9988".parse().unwrap(),
        relay: None,
        intent: PublicIntent::new(
            InvitationId::new([1; 16]),
            AttemptId::new([2; 16]),
            ConsentIntentId::new([3; 16]),
            1,
            5000,
            ConsentClass::PeerRead,
        )
        .unwrap(),
        phase: PeerPhase::Active {
            credential: CredentialId::new("peer-credential").unwrap(),
            receiver: ResourceId::new("peer-receiver").unwrap(),
        },
        discovery: Discovery::None,
        discovery_enabled: true,
        discovery_json: None,
        credential_current: true,
    }
}

#[test]
fn legacy_enrollment_without_eligibility_stays_silent_after_restore() {
    let record = record();
    let own = [6; 44];
    let bytes = encode(&record, &own).unwrap();
    assert!(decode(&bytes, &record.key, &own)
        .unwrap()
        .needs_discovery_exchange());
    let mut old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    old.as_object_mut().unwrap().remove("discoveryEnabled");
    let restored = decode(&serde_json::to_vec(&old).unwrap(), &record.key, &own).unwrap();
    assert!(!restored.discovery_enabled);
    assert!(!restored.needs_discovery_exchange());
    assert!(matches!(restored.discovery(), Discovery::None));
}

#[test]
fn installed_dialer_retries_after_restart_until_remote_effect_can_be_delivered() {
    let record = record();
    let discovery = Discovery::Installed {
        generation: [2; 32],
        mac: [3; 32],
    };
    let installed = PeerRecord {
        discovery_json: discovery_to_json(&discovery, None),
        discovery,
        ..record
    };
    let own = [6; 44];
    let restored = decode(&encode(&installed, &own).unwrap(), &installed.key, &own).unwrap();
    assert!(restored.needs_discovery_exchange());
    assert_eq!(restored.discovery().installed_mac(), Some(&[3; 32]));
    assert!(!PeerRecord {
        credential_current: false,
        ..restored.clone()
    }
    .needs_discovery_exchange());
    assert!(!PeerRecord {
        phase: PeerPhase::Revoked,
        ..restored
    }
    .needs_discovery_exchange());
}
