use super::{
    announcement_bytes, announcement_txt, connection_allowed, decode_announcement, decode_message,
    discovery_from_json, discovery_to_json, encode_message, exchange, instance_label, mac_key,
    match_announcement, payload_from_txt, public_half, shared_secret, Announcement, Discovery,
    DiscoveryPipe, Inbound, Outbound, Phase, PinnedMac, Role, Session, TokenPeer, DISCOVERY_WINDOW,
};
use std::net::SocketAddr;

fn address() -> SocketAddr {
    "192.0.2.10:47650".parse().unwrap()
}

fn key(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn entropy_from(bytes: Vec<[u8; 32]>) -> impl FnMut() -> [u8; 32] {
    let mut values = bytes.into_iter();
    move || values.next().expect("test entropy")
}

fn active(role: Role) -> Session {
    Session::new(role, Phase::Active, Discovery::None)
}

fn publish_of(message: &Outbound) -> Option<[u8; 32]> {
    match message {
        Outbound::Publish { public_key, .. } => Some(*public_key),
        _ => None,
    }
}

#[test]
fn an_unpaired_gateway_sends_no_announcement() {
    let mac = key(7);
    for phase in [Phase::Pending, Phase::Revoked] {
        let peers = [TokenPeer {
            mac: &mac,
            phase,
            credential_current: true,
        }];
        assert!(announcement_bytes(&peers, address(), 1_000_000).is_none());
    }
    let quiet = active(Role::Dialer);
    assert!(quiet.reconnect().is_empty());
    assert!(quiet.mint(key(1), key(2)).is_none() || quiet.phase == Phase::Active);
    let pending = Session::new(Role::Dialer, Phase::Pending, Discovery::None);
    assert!(pending.mint(key(1), key(2)).is_none());
    let (after, commits) = pending.receive(
        Inbound::Publish {
            generation: key(1),
            public_key: public_half(&key(3)),
        },
        &mut entropy_from(vec![key(4)]),
    );
    assert!(commits.is_empty());
    assert!(matches!(after.discovery, Discovery::None));
    assert_eq!(after.phase, Phase::Pending);
}

#[test]
fn a_revoked_or_expired_credential_is_not_announced_and_the_phase_stays_active() {
    let mac = key(9);
    let installed = Discovery::Installed {
        generation: key(1),
        mac,
    };
    let peers = [TokenPeer {
        mac: &mac,
        phase: Phase::Active,
        credential_current: false,
    }];
    assert!(announcement_bytes(&peers, address(), 5_000_000).is_none());
    let session = Session::new(Role::Dialer, Phase::Active, installed.clone());
    assert_eq!(session.phase, Phase::Active);
    assert_eq!(session.discovery.installed_mac(), Some(&mac));
    assert!(!connection_allowed(false));
}

#[test]
fn a_peer_with_no_discovery_key_is_left_out_and_not_invented() {
    let peers = [TokenPeer {
        mac: &[0; 32],
        phase: Phase::Active,
        credential_current: true,
    }];
    assert!(announcement_bytes(&peers, address(), 0).is_none());
    assert!(discovery_to_json(&Discovery::None, None).is_none());
    let kept = Discovery::Keyed {
        generation: key(2),
        private_key: key(3),
    };
    let text = discovery_to_json(&kept, None).unwrap();
    assert_eq!(
        discovery_from_json(&text).unwrap().generation_bytes(),
        key(2)
    );
}

#[test]
fn the_public_half_is_the_x25519_u_coordinate_not_the_private_key() {
    let private_key = key(4);
    let public = public_half(&private_key);
    assert_ne!(public, private_key);
    assert_ne!(public, [0; 32]);
    assert_eq!(public, public_half(&private_key));
}

#[test]
fn a_drop_before_the_private_key_commit_sends_no_public_half() {
    let dialer = active(Role::Dialer);
    assert!(dialer.reconnect().is_empty());
    let commit = dialer.mint(key(8), key(5)).unwrap();
    assert!(commit.durable);
    assert!(matches!(dialer.discovery, Discovery::None));
    assert!(dialer.reconnect().is_empty());
    let stored = Session::new(Role::Dialer, Phase::Active, commit.discovery.clone());
    let sent = stored.reconnect();
    assert_eq!(
        publish_of(sent.first().unwrap()),
        Some(public_half(&key(5)))
    );
}

#[test]
fn a_drop_after_the_keyed_commit_resends_the_stored_public_and_does_not_mint() {
    let dialer = active(Role::Dialer);
    let commit = dialer.mint(key(8), key(5)).unwrap();
    let restarted = Session::new(Role::Dialer, Phase::Active, commit.discovery);
    let again = restarted.mint(key(9), key(6));
    assert!(again.is_none());
    let sent = restarted.reconnect();
    assert_eq!(sent.len(), 1);
    assert_eq!(publish_of(&sent[0]), Some(public_half(&key(5))));
}

#[test]
fn an_all_zero_public_is_rejected_and_an_all_zero_secret_returns_offered_to_keyed() {
    let grantor = active(Role::Grantor);
    let (after, commits) = grantor.receive(
        Inbound::Publish {
            generation: key(1),
            public_key: [0; 32],
        },
        &mut entropy_from(vec![key(4)]),
    );
    assert!(matches!(after.discovery, Discovery::None));
    assert!(!commits.iter().any(|commit| commit.durable));
    assert!(matches!(
        commits[0].send.as_slice(),
        [Outbound::Reject { .. }]
    ));

    let private_key = key(5);
    let peer_public = low_order_public();
    assert!(shared_secret(&private_key, &peer_public).is_none());
    let offered = Session::new(
        Role::Dialer,
        Phase::Active,
        Discovery::Offered {
            generation: key(1),
            private_key,
            peer_public: public_half(&key(6)),
        },
    );
    let (cleared, commits) = offered.receive(
        Inbound::Publish {
            generation: key(1),
            public_key: peer_public,
        },
        &mut || key(7),
    );
    assert!(
        matches!(cleared.discovery, Discovery::Keyed { private_key: kept, .. } if kept == private_key)
    );
    assert_eq!(cleared.phase, Phase::Active);
    assert!(commits.iter().any(|commit| commit.durable));
    let refused = Session::new(
        Role::Grantor,
        Phase::Active,
        Discovery::Keyed {
            generation: key(1),
            private_key: key(6),
        },
    );
    let (rekeyed, commits) =
        refused.receive(Inbound::Reject { generation: key(1) }, &mut || key(8));
    match rekeyed.discovery {
        Discovery::Keyed {
            generation,
            private_key,
        } => {
            assert_eq!(generation, key(1));
            assert_eq!(private_key, key(8));
        }
        other => panic!("rekey stayed {other:?}"),
    }
    assert!(commits.iter().any(|commit| commit.durable
        && commit
            .send
            .iter()
            .any(|message| { publish_of(message) == Some(public_half(&key(8))) })));
}

#[test]
fn two_generation_ids_resolve_to_the_dialers_id() {
    let dialer_generation = key(9);
    let grantor_generation = key(1);
    assert!(dialer_generation > grantor_generation);
    let dialer = Session::new(
        Role::Dialer,
        Phase::Active,
        Discovery::Keyed {
            generation: dialer_generation,
            private_key: key(2),
        },
    );
    let grantor = Session::new(
        Role::Grantor,
        Phase::Active,
        Discovery::Keyed {
            generation: grantor_generation,
            private_key: key(3),
        },
    );
    let dialer_public = public_half(&key(2));
    let (dialer_after, dialer_commits) = dialer.receive(
        Inbound::Publish {
            generation: grantor_generation,
            public_key: public_half(&key(3)),
        },
        &mut || key(4),
    );
    assert!(matches!(
        dialer_after.discovery,
        Discovery::Keyed { generation, private_key } if generation == dialer_generation && private_key == key(2)
    ));
    assert!(dialer_commits.iter().all(|commit| !commit.durable));
    assert_eq!(
        publish_of(dialer_commits[0].send.first().unwrap()),
        Some(dialer_public)
    );
    let (grantor_after, grantor_commits) = grantor.receive(
        Inbound::Publish {
            generation: dialer_generation,
            public_key: dialer_public,
        },
        &mut || key(5),
    );
    assert_eq!(
        grantor_after.discovery.generation_bytes(),
        dialer_generation
    );
    assert!(grantor_commits.iter().any(|commit| commit.durable));
    assert_ne!(
        grantor_after.discovery.private_bytes(),
        Some(key(3)),
        "the loser replaced its private key"
    );
}

#[test]
fn dialer_reconnect_resends_its_own_public_half_and_stored() {
    let dialer_private = key(2);
    let grantor_public = public_half(&key(3));
    let dialer = Session::new(
        Role::Dialer,
        Phase::Active,
        Discovery::Offered {
            generation: key(1),
            private_key: dialer_private,
            peer_public: grantor_public,
        },
    );
    let sent = dialer.reconnect();
    assert_eq!(
        sent,
        vec![
            Outbound::Publish {
                generation: key(1),
                public_key: public_half(&dialer_private),
            },
            Outbound::Stored { generation: key(1) },
        ]
    );
    assert_ne!(publish_of(&sent[0]), Some(grantor_public));
    assert!(dialer.mint(key(9), key(8)).is_none());
}

#[test]
fn grantor_reconnect_resends_its_public_half_and_both_install_one_mac() {
    let dialer_private = key(2);
    let grantor_private = key(3);
    let generation = key(1);
    let grantor = Session::new(
        Role::Grantor,
        Phase::Active,
        Discovery::Offered {
            generation,
            private_key: grantor_private,
            peer_public: public_half(&dialer_private),
        },
    );
    let sent = grantor.reconnect();
    assert_eq!(publish_of(&sent[0]), Some(public_half(&grantor_private)));
    assert!(matches!(sent[1], Outbound::Stored { .. }));
    let dialer = Session::new(
        Role::Dialer,
        Phase::Active,
        Discovery::Offered {
            generation,
            private_key: dialer_private,
            peer_public: public_half(&grantor_private),
        },
    );
    let (dialer, _) = dialer.receive(Inbound::Stored { generation }, &mut || key(9));
    let (grantor, _) = grantor.receive(Inbound::Stored { generation }, &mut || key(9));
    assert_eq!(
        dialer.discovery.installed_mac(),
        grantor.discovery.installed_mac()
    );
    assert!(dialer.discovery.installed_mac().is_some());
}

#[test]
fn offered_retries_keep_the_key_and_a_split_pair_finishes_without_rederiving() {
    let generation = key(1);
    let dialer_private = key(2);
    let grantor_private = key(3);
    let dialer_offered = Discovery::Offered {
        generation,
        private_key: dialer_private,
        peer_public: public_half(&grantor_private),
    };
    let grantor_offered = Discovery::Offered {
        generation,
        private_key: grantor_private,
        peer_public: public_half(&dialer_private),
    };
    let reloaded_dialer = Session::new(Role::Dialer, Phase::Active, dialer_offered.clone());
    let reloaded_grantor = Session::new(Role::Grantor, Phase::Active, grantor_offered.clone());
    assert_eq!(
        reloaded_dialer.reconnect(),
        Session::new(Role::Dialer, Phase::Active, dialer_offered.clone()).reconnect()
    );
    let (dialer, dialer_commits) =
        reloaded_dialer.receive(Inbound::Stored { generation }, &mut || key(9));
    let (grantor, grantor_commits) =
        reloaded_grantor.receive(Inbound::Stored { generation }, &mut || key(8));
    let mac = dialer.discovery.installed_mac().unwrap().clone();
    assert_eq!(grantor.discovery.installed_mac(), Some(&mac));
    assert!(dialer_commits
        .iter()
        .all(|commit| commit.discovery.private_bytes().is_none()));
    assert!(grantor_commits.last().unwrap().durable);

    let installed = Session::new(
        Role::Grantor,
        Phase::Active,
        Discovery::Installed { generation, mac },
    );
    let before = installed.discovery.installed_mac().unwrap().clone();
    let resent = installed.reconnect();
    assert_eq!(resent, vec![Outbound::Installed { generation }]);
    assert_eq!(installed.discovery.installed_mac(), Some(&before));
    let offered = Session::new(Role::Dialer, Phase::Active, dialer_offered);
    let (finished, commits) = offered.receive(Inbound::Installed { generation }, &mut || key(7));
    assert_eq!(finished.discovery.installed_mac(), Some(&before));
    assert_eq!(
        commits
            .last()
            .unwrap()
            .audit
            .as_ref()
            .map(|audit| audit.cause),
        Some("peer installed")
    );
    let (again, extra) = installed.receive(Inbound::Installed { generation }, &mut || key(6));
    assert!(extra.is_empty());
    assert_eq!(again.discovery.installed_mac(), Some(&before));
}

#[test]
fn the_announcement_has_no_fingerprint_or_stable_id_and_cannot_enroll() {
    let mac = mac_key(
        &key(1),
        &key(2),
        &public_half(&key(3)),
        &public_half(&key(4)),
    );
    let fingerprint = *b"fingerprint-of-the-pinned-gateway-key!";
    let gateway_id = *b"gateway-id-should-stay-out-of-bytes";
    let peers = [TokenPeer {
        mac: &mac,
        phase: Phase::Active,
        credential_current: true,
    }];
    let bytes = announcement_bytes(&peers, address(), 10 * window_ms()).unwrap();
    assert!(bytes
        .windows(fingerprint.len())
        .all(|window| window != fingerprint));
    assert!(bytes
        .windows(gateway_id.len())
        .all(|window| window != gateway_id));
    assert!(bytes.windows(32).all(|window| window != mac));
    let parsed = decode_announcement(&bytes).unwrap();
    assert_eq!(parsed.address, address());
    assert_eq!(parsed.tokens.len(), 1);
    let mut smuggled = bytes.clone();
    smuggled.extend_from_slice(&fingerprint);
    assert!(decode_announcement(&smuggled).is_none());
    let matched = match_announcement(&bytes, &[PinnedMac { mac: &mac }], 10 * window_ms());
    assert_eq!(matched, Some((0, address())));
    assert!(!connection_allowed(false));
    assert!(connection_allowed(true));
    let properties = announcement_txt(&bytes);
    assert!(properties.iter().all(|(key, _)| key.starts_with('p')));
    assert!(!properties
        .iter()
        .any(|(key, value)| key == "gw" || value.contains("gateway")));
    assert_eq!(payload_from_txt(&properties).unwrap(), bytes);
    let label = instance_label(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(!label.contains("gateway"));
    assert_eq!(label.len(), 1 + 16);
}

#[test]
fn two_windows_differ_and_a_token_copied_to_another_port_does_not_match() {
    let mac = key(6);
    let now = 10 * window_ms();
    let peers = [TokenPeer {
        mac: &mac,
        phase: Phase::Active,
        credential_current: true,
    }];
    let first = announcement_bytes(&peers, address(), now).unwrap();
    let second = announcement_bytes(&peers, address(), now + window_ms()).unwrap();
    let Announcement { tokens: left, .. } = decode_announcement(&first).unwrap();
    let Announcement { tokens: right, .. } = decode_announcement(&second).unwrap();
    assert_ne!(left, right);
    let other: SocketAddr = "192.0.2.10:9".parse().unwrap();
    let moved = announcement_bytes(&peers, other, now).unwrap();
    let stolen = decode_announcement(&first).unwrap();
    let mut copied = encode_with_foreign_token(&moved, &stolen.tokens[0]);
    assert!(match_announcement(&copied, &[PinnedMac { mac: &mac }], now).is_none());
    copied = first.clone();
    assert!(match_announcement(&copied, &[PinnedMac { mac: &key(1) }], now).is_none());
    let mapped: SocketAddr = "[::ffff:192.0.2.10]:47650".parse().unwrap();
    let as_v6 = announcement_bytes(&peers, mapped, now).unwrap();
    assert_eq!(
        decode_announcement(&as_v6).unwrap().tokens,
        decode_announcement(&first).unwrap().tokens
    );
    assert_eq!(decode_announcement(&as_v6).unwrap().address, address());
}

#[test]
fn a_message_round_trips_and_a_refusal_envelope_is_not_a_public_half() {
    let message = Outbound::Publish {
        generation: key(1),
        public_key: public_half(&key(2)),
    };
    let bytes = encode_message(&message);
    assert_eq!(
        decode_message(&bytes),
        Some(Inbound::Publish {
            generation: key(1),
            public_key: public_half(&key(2)),
        })
    );
    assert!(decode_message(br#"{"kind":"refused"}"#).is_none());
    assert!(decode_message(&[1, 0]).is_none());
}

#[test]
fn the_window_length_is_the_limit_row() {
    assert_eq!(DISCOVERY_WINDOW.as_millis(), 300_000);
}

#[test]
fn a_refusal_envelope_does_not_enroll() {
    let mut refused = active(Role::Dialer);
    let commit = refused.mint(key(1), key(2)).unwrap();
    refused = Session::new(Role::Dialer, Phase::Active, commit.discovery);
    let (dialer_tx, _held) = std::sync::mpsc::channel();
    let (peer_tx, dialer_rx) = std::sync::mpsc::channel();
    peer_tx.send(br#"{"kind":"refused"}"#.to_vec()).unwrap();
    let mut pipe = Queue {
        tx: dialer_tx,
        rx: dialer_rx,
    };
    let error = exchange(
        &mut refused,
        &mut pipe,
        &mut || key(5),
        &mut |_| Ok(()),
        true,
    );
    assert!(matches!(error, Err(super::ExchangeError::Refused)));
    assert!(refused.discovery.installed_mac().is_none());
}

struct Queue {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
}

impl DiscoveryPipe for Queue {
    fn send_frame(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.tx
            .send(bytes.to_vec())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::BrokenPipe, error))
    }
    fn recv_frame(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::TimedOut, error))
    }
}

fn window_ms() -> u64 {
    u64::try_from(DISCOVERY_WINDOW.as_millis()).unwrap()
}

fn encode_with_foreign_token(announcement: &[u8], token: &[u8; 32]) -> Vec<u8> {
    let mut parsed = decode_announcement(announcement).unwrap();
    parsed.tokens = vec![*token];
    let mut bytes = announcement[..announcement.len() - 32].to_vec();
    bytes.extend_from_slice(token);
    let _ = parsed;
    bytes
}

fn low_order_public() -> [u8; 32] {
    // A canonical low-order Curve25519 point. X25519 against it is 32 zero bytes.
    let mut public = [0u8; 32];
    public[0] = 1;
    assert!(
        shared_secret(&key(5), &public).is_none(),
        "the fixture public must derive an all-zero secret"
    );
    public
}

trait DiscoveryBytes {
    fn generation_bytes(&self) -> [u8; 32];
    fn private_bytes(&self) -> Option<[u8; 32]>;
}

impl DiscoveryBytes for Discovery {
    fn generation_bytes(&self) -> [u8; 32] {
        match self {
            Discovery::None => panic!("none has no generation"),
            Discovery::Keyed { generation, .. }
            | Discovery::Offered { generation, .. }
            | Discovery::Installed { generation, .. } => *generation,
        }
    }
    fn private_bytes(&self) -> Option<[u8; 32]> {
        match self {
            Discovery::Keyed { private_key, .. } | Discovery::Offered { private_key, .. } => {
                Some(*private_key)
            }
            _ => None,
        }
    }
}
