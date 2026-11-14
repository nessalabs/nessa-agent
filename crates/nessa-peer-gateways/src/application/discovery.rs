//! Discovery key agreement and the LAN announcement of a paired gateway.
//!
//! Pairing phases stay `Pending`, `Active`, and `Revoked`. The discovery
//! field on that same record moves `none → keyed → offered → installed`
//! after `Active`, over the pinned session. The announcement carries a dial
//! address, a port, and one rotating token per installed peer. It does not
//! carry a key, a fingerprint, or any other stable gateway id, and it cannot
//! enroll a listener.
//!
//! The contract is the pairing table in `docs/design/access-grants.md`.
use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use subtle::ConstantTimeEq;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

/// How long one announcement token stays the token for its window.
/// A listener also accepts the window on either side.
pub const DISCOVERY_WINDOW: Duration = Duration::from_millis(300_000);

const LABEL: &[u8] = b"nessa-discovery-mac-v1";
const _: () = assert!(LABEL.len() == 22);

const INITIATOR: &str = "gateway";

/// Which side of the enrollment this record is. Dialer and grantor are those
/// roles, not which generation id won a cross.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Dialer,
    Grantor,
}

/// The enrollment phase this discovery step is allowed to see. Discovery
/// does not write it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Pending,
    Active,
    Revoked,
}

/// The discovery field. `None` is a record with no key. Alpha does not
/// invent one.
#[derive(Clone, Eq, PartialEq)]
pub enum Discovery {
    None,
    Keyed {
        generation: [u8; 32],
        private_key: [u8; 32],
    },
    Offered {
        generation: [u8; 32],
        private_key: [u8; 32],
        peer_public: [u8; 32],
    },
    Installed {
        generation: [u8; 32],
        mac: [u8; 32],
    },
}

impl Drop for Discovery {
    fn drop(&mut self) {
        match self {
            Self::Keyed { private_key, .. } | Self::Offered { private_key, .. } => {
                private_key.zeroize();
            }
            Self::Installed { mac, .. } => mac.zeroize(),
            Self::None => {}
        }
    }
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Keyed { generation, .. } => formatter
                .debug_struct("Keyed")
                .field("generation", generation)
                .field("private_key", &"[REDACTED]")
                .finish(),
            Self::Offered {
                generation,
                peer_public,
                ..
            } => formatter
                .debug_struct("Offered")
                .field("generation", generation)
                .field("private_key", &"[REDACTED]")
                .field("peer_public", peer_public)
                .finish(),
            Self::Installed { generation, .. } => formatter
                .debug_struct("Installed")
                .field("generation", generation)
                .field("mac", &"[REDACTED]")
                .finish(),
        }
    }
}

impl Discovery {
    /// The field name the announcement and the audit use.
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Keyed { .. } => "keyed",
            Self::Offered { .. } => "offered",
            Self::Installed { .. } => "installed",
        }
    }

    pub fn installed_mac(&self) -> Option<&[u8; 32]> {
        match self {
            Self::Installed { mac, .. } => Some(mac),
            _ => None,
        }
    }

    fn generation(&self) -> Option<[u8; 32]> {
        match self {
            Self::None => None,
            Self::Keyed { generation, .. }
            | Self::Offered { generation, .. }
            | Self::Installed { generation, .. } => Some(*generation),
        }
    }
}

/// One durable step. `durable` is false when the field does not change: the
/// caller sends `send` and does not write the record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    pub discovery: Discovery,
    pub audit: Option<Audit>,
    pub send: Vec<Outbound>,
    pub durable: bool,
}

/// The audit kept in the same write as a durable discovery change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Audit {
    pub before: &'static str,
    pub after: &'static str,
    pub cause: &'static str,
    pub initiator: &'static str,
}

fn audit(before: &'static str, after: &'static str, cause: &'static str) -> Audit {
    Audit {
        before,
        after,
        cause,
        initiator: INITIATOR,
    }
}

/// A message on the pinned session. `Publish`'s public half is this side's
/// X25519 public key, never the stored peer public.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outbound {
    Publish {
        generation: [u8; 32],
        public_key: [u8; 32],
    },
    Stored {
        generation: [u8; 32],
    },
    Installed {
        generation: [u8; 32],
    },
    Reject {
        generation: [u8; 32],
    },
}

/// A message read from the peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Inbound {
    Publish {
        generation: [u8; 32],
        public_key: [u8; 32],
    },
    Stored {
        generation: [u8; 32],
    },
    Installed {
        generation: [u8; 32],
    },
    Reject {
        generation: [u8; 32],
    },
}

/// In-memory view of one side. `peer_stored` is this session only: a restart
/// learns it again when the peer resends `stored`.
#[derive(Clone)]
pub struct Session {
    pub role: Role,
    pub phase: Phase,
    pub discovery: Discovery,
    pub peer_stored: bool,
}

impl Session {
    pub fn new(role: Role, phase: Phase, discovery: Discovery) -> Self {
        Self {
            role,
            phase,
            discovery,
            peer_stored: false,
        }
    }

    /// What a reconnect sends for the stored field. Nothing is minted here.
    pub fn reconnect(&self) -> Vec<Outbound> {
        if self.phase != Phase::Active {
            return Vec::new();
        }
        match &self.discovery {
            Discovery::None => Vec::new(),
            Discovery::Keyed {
                generation,
                private_key,
            } => vec![Outbound::Publish {
                generation: *generation,
                public_key: public_half(private_key),
            }],
            Discovery::Offered {
                generation,
                private_key,
                ..
            } => vec![
                Outbound::Publish {
                    generation: *generation,
                    public_key: public_half(private_key),
                },
                Outbound::Stored {
                    generation: *generation,
                },
            ],
            Discovery::Installed { generation, .. } => {
                vec![Outbound::Installed {
                    generation: *generation,
                }]
            }
        }
    }

    /// The dialer mints one generation from `none`. The private key is
    /// durable before its public half is sent: the commit carries no send
    /// until the caller has stored it and calls [`Self::reconnect`].
    pub fn mint(&self, generation: [u8; 32], private_key: [u8; 32]) -> Option<Commit> {
        if self.phase != Phase::Active || self.role != Role::Dialer {
            return None;
        }
        if !matches!(self.discovery, Discovery::None) {
            return None;
        }
        if public_half(&private_key) == [0; 32] {
            return None;
        }
        Some(Commit {
            discovery: Discovery::Keyed {
                generation,
                private_key,
            },
            audit: Some(audit("none", "keyed", "local key")),
            send: vec![Outbound::Publish {
                generation,
                public_key: public_half(&private_key),
            }],
            durable: true,
        })
    }

    /// Apply one inbound message. The returned session is the state after
    /// every commit. A caller that crashes stores only the commits it
    /// persisted and starts the next session with `peer_stored` false.
    pub fn receive(
        &self,
        incoming: Inbound,
        entropy: &mut dyn FnMut() -> [u8; 32],
    ) -> (Session, Vec<Commit>) {
        if self.phase != Phase::Active {
            return (self.clone(), Vec::new());
        }
        let mut session = self.clone();
        let commits = session.apply(incoming, entropy);
        (session, commits)
    }
}

impl Session {
    fn apply(&mut self, incoming: Inbound, entropy: &mut dyn FnMut() -> [u8; 32]) -> Vec<Commit> {
        match incoming {
            Inbound::Publish {
                generation,
                public_key,
            } => self.on_publish(generation, public_key, entropy),
            Inbound::Stored { generation } => self.on_stored(generation),
            Inbound::Installed { generation } => self.on_installed(generation),
            Inbound::Reject { generation } => self.on_reject(generation, entropy),
        }
    }

    fn on_publish(
        &mut self,
        generation: [u8; 32],
        public_key: [u8; 32],
        entropy: &mut dyn FnMut() -> [u8; 32],
    ) -> Vec<Commit> {
        if public_key == [0; 32] {
            return self.reject_public(generation);
        }
        if let Some(ours) = self.discovery.generation() {
            if ours != generation {
                return self.cross(generation, public_key, entropy);
            }
        }
        match &self.discovery {
            Discovery::Installed {
                generation: ours, ..
            } if *ours == generation => {
                vec![self.resend()]
            }
            Discovery::Installed { .. } => Vec::new(),
            Discovery::None => {
                if self.role != Role::Grantor {
                    return Vec::new();
                }
                self.accept(generation, public_key, entropy)
            }
            Discovery::Keyed { private_key, .. } => {
                let private_key = *private_key;
                self.offer(generation, private_key, public_key)
            }
            Discovery::Offered {
                private_key,
                peer_public,
                ..
            } => {
                if *peer_public == public_key {
                    return vec![self.resend()];
                }
                let private_key = *private_key;
                self.offer(generation, private_key, public_key)
            }
        }
    }

    fn accept(
        &mut self,
        generation: [u8; 32],
        public_key: [u8; 32],
        entropy: &mut dyn FnMut() -> [u8; 32],
    ) -> Vec<Commit> {
        let private_key = entropy();
        if public_half(&private_key) == [0; 32]
            || shared_secret(&private_key, &public_key).is_none()
        {
            return self.reject_public(generation);
        }
        let offered = Discovery::Offered {
            generation,
            private_key,
            peer_public: public_key,
        };
        let mut commits = vec![self.wrote(
            offered,
            audit("none", "offered", "accept dialer generation"),
            self.outbound_after_offer(generation, &private_key),
        )];
        if self.peer_stored {
            if let Some(install) =
                self.install_from(commits[0].discovery.clone(), "discovery install")
            {
                self.discovery = install.discovery.clone();
                commits.push(install);
            }
        } else {
            self.discovery = commits[0].discovery.clone();
        }
        commits
    }

    fn offer(
        &mut self,
        generation: [u8; 32],
        private_key: [u8; 32],
        public_key: [u8; 32],
    ) -> Vec<Commit> {
        if shared_secret(&private_key, &public_key).is_none() {
            return self.reject_public(generation);
        }
        let before = self.discovery.name();
        let offered = Discovery::Offered {
            generation,
            private_key,
            peer_public: public_key,
        };
        let mut commits = vec![self.wrote(
            offered,
            audit(before, "offered", "peer public"),
            vec![Outbound::Stored { generation }],
        )];
        if self.peer_stored {
            if let Some(install) =
                self.install_from(commits[0].discovery.clone(), "discovery install")
            {
                self.discovery = install.discovery.clone();
                commits.push(install);
            }
        } else {
            self.discovery = commits[0].discovery.clone();
        }
        commits
    }

    fn cross(
        &mut self,
        generation: [u8; 32],
        public_key: [u8; 32],
        entropy: &mut dyn FnMut() -> [u8; 32],
    ) -> Vec<Commit> {
        // The enrollment dialer's generation wins. A grantor that already
        // minted replaces its private key for that generation and does not
        // mint a new id. The public half in this message is for the winning
        // generation, so the offer is the next commit, after the new private
        // key is the one that would be stored.
        if self.role == Role::Dialer {
            return vec![self.resend()];
        }
        let private_key = entropy();
        if public_half(&private_key) == [0; 32] {
            return Vec::new();
        }
        let before = self.discovery.name();
        self.peer_stored = false;
        let keyed = Discovery::Keyed {
            generation,
            private_key,
        };
        let commit = self.wrote(
            keyed,
            audit(before, "keyed", "dialer generation"),
            vec![Outbound::Publish {
                generation,
                public_key: public_half(&private_key),
            }],
        );
        self.discovery = commit.discovery.clone();
        let mut commits = vec![commit];
        let mut offered = self.offer(generation, private_key, public_key);
        commits.append(&mut offered);
        commits
    }

    fn on_stored(&mut self, generation: [u8; 32]) -> Vec<Commit> {
        if self.discovery.generation() != Some(generation) {
            return Vec::new();
        }
        if !matches!(
            self.discovery,
            Discovery::Keyed { .. } | Discovery::Offered { .. }
        ) {
            return Vec::new();
        }
        self.peer_stored = true;
        if let Discovery::Offered { .. } = &self.discovery {
            if let Some(install) = self.install_from(self.discovery.clone(), "discovery install") {
                self.discovery = install.discovery.clone();
                return vec![install];
            }
        }
        Vec::new()
    }

    fn on_installed(&mut self, generation: [u8; 32]) -> Vec<Commit> {
        match &self.discovery {
            Discovery::Installed {
                generation: ours, ..
            } if *ours == generation => Vec::new(),
            Discovery::Offered {
                generation: ours, ..
            } if *ours == generation => {
                let Some(install) = self.install_from(self.discovery.clone(), "peer installed")
                else {
                    return self.reject_public(generation);
                };
                self.discovery = install.discovery.clone();
                vec![install]
            }
            _ => Vec::new(),
        }
    }

    fn on_reject(
        &mut self,
        generation: [u8; 32],
        entropy: &mut dyn FnMut() -> [u8; 32],
    ) -> Vec<Commit> {
        if self.discovery.generation() != Some(generation) {
            return Vec::new();
        }
        if !matches!(
            self.discovery,
            Discovery::Keyed { .. } | Discovery::Offered { .. }
        ) {
            return Vec::new();
        }
        let private_key = entropy();
        if public_half(&private_key) == [0; 32] {
            return Vec::new();
        }
        let before = self.discovery.name();
        self.peer_stored = false;
        let keyed = Discovery::Keyed {
            generation,
            private_key,
        };
        let commit = self.wrote(
            keyed,
            audit(before, "keyed", "public key rejected"),
            vec![Outbound::Publish {
                generation,
                public_key: public_half(&private_key),
            }],
        );
        self.discovery = commit.discovery.clone();
        vec![commit]
    }

    fn reject_public(&mut self, generation: [u8; 32]) -> Vec<Commit> {
        let before = self.discovery.name();
        let (discovery, durable, after) = match &self.discovery {
            Discovery::Offered {
                generation: ours,
                private_key,
                ..
            } if *ours == generation => (
                Discovery::Keyed {
                    generation,
                    private_key: *private_key,
                },
                true,
                "keyed",
            ),
            other => (other.clone(), false, other.name()),
        };
        if durable {
            self.discovery = discovery.clone();
            self.peer_stored = false;
        }
        vec![Commit {
            discovery,
            audit: Some(audit(
                before,
                after,
                "public key rejected or shared secret all zeros",
            )),
            send: vec![Outbound::Reject { generation }],
            durable,
        }]
    }

    fn install_from(&self, discovery: Discovery, cause: &'static str) -> Option<Commit> {
        let Discovery::Offered {
            generation,
            private_key,
            peer_public,
        } = discovery
        else {
            return None;
        };
        let own = public_half(&private_key);
        if own == [0; 32] || peer_public == [0; 32] {
            return None;
        }
        let shared = shared_secret(&private_key, &peer_public)?;
        let (dialer, grantor) = match self.role {
            Role::Dialer => (own, peer_public),
            Role::Grantor => (peer_public, own),
        };
        let mac = mac_key(&generation, &shared, &dialer, &grantor);
        Some(Commit {
            discovery: Discovery::Installed { generation, mac },
            audit: Some(audit("offered", "installed", cause)),
            send: vec![Outbound::Installed { generation }],
            durable: true,
        })
    }

    fn outbound_after_offer(&self, generation: [u8; 32], private_key: &[u8; 32]) -> Vec<Outbound> {
        vec![
            Outbound::Publish {
                generation,
                public_key: public_half(private_key),
            },
            Outbound::Stored { generation },
        ]
    }

    fn wrote(&self, discovery: Discovery, audit: Audit, send: Vec<Outbound>) -> Commit {
        Commit {
            discovery,
            audit: Some(audit),
            send,
            durable: true,
        }
    }

    fn resend(&self) -> Commit {
        Commit {
            discovery: self.discovery.clone(),
            audit: None,
            send: self.reconnect(),
            durable: false,
        }
    }
}

/// X25519(private, basepoint 9): the 32-byte little-endian u-coordinate.
pub fn public_half(private_key: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*private_key);
    PublicKey::from(&secret).to_bytes()
}

/// RFC 7748 shared secret. `None` when it is 32 zero bytes, which is not installed.
pub fn shared_secret(private_key: &[u8; 32], peer_public: &[u8; 32]) -> Option<[u8; 32]> {
    let secret = StaticSecret::from(*private_key);
    let public = PublicKey::from(*peer_public);
    let shared = secret.diffie_hellman(&public).to_bytes();
    if bool::from(shared.ct_eq(&[0; 32])) {
        None
    } else {
        Some(shared)
    }
}

/// HKDF-SHA256 of the shared secret. Salt is the generation id. Info is the
/// label, then the dialer's public key, then the grantor's, with no length prefix.
pub fn mac_key(
    generation: &[u8; 32],
    shared: &[u8; 32],
    dialer_public: &[u8; 32],
    grantor_public: &[u8; 32],
) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(generation.as_slice()), shared.as_slice());
    let mut info = [0u8; 22 + 64];
    info[..22].copy_from_slice(LABEL);
    info[22..54].copy_from_slice(dialer_public);
    info[54..].copy_from_slice(grantor_public);
    let mut out = [0u8; 32];
    hk.expand(&info, &mut out)
        .expect("32-byte discovery MAC fits one SHA-256 block");
    out
}

/// One peer the announcement may name, already reduced to the facts P-announce reads.
#[derive(Clone, Copy, Debug)]
pub struct TokenPeer<'a> {
    pub mac: &'a [u8; 32],
    pub phase: Phase,
    pub credential_current: bool,
}

/// Parsed announcement. The address and port are the bytes the tokens were
/// computed over.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Announcement {
    pub address: SocketAddr,
    pub tokens: Vec<[u8; 32]>,
}

/// Bytes to send, or nothing when no peer may be announced.
pub fn announcement_bytes(
    peers: &[TokenPeer<'_>],
    address: SocketAddr,
    now_ms: u64,
) -> Option<Vec<u8>> {
    let window = window_ms()?;
    let eligible: Vec<[u8; 32]> = peers
        .iter()
        .filter(|peer| {
            peer.phase == Phase::Active && peer.credential_current && peer.mac != &[0; 32]
        })
        .map(|peer| token(peer.mac, now_ms / window, address))
        .collect();
    if eligible.is_empty() {
        return None;
    }
    let mut tokens = eligible;
    tokens.sort();
    Some(encode_announcement(address, &tokens))
}

/// Match `bytes` to a pinned MAC. The window on either side of `now_ms` counts.
/// The index is into `peers`. A MAC that is not installed here does not match.
pub fn match_announcement(
    bytes: &[u8],
    peers: &[PinnedMac<'_>],
    now_ms: u64,
) -> Option<(usize, SocketAddr)> {
    let announcement = decode_announcement(bytes)?;
    let window = window_ms()?;
    let index = now_ms / window;
    let windows = [index.wrapping_sub(1), index, index.saturating_add(1)];
    let consider_previous = index > 0;
    let consider_next = index < u64::MAX;
    for (token_index, token_bytes) in announcement.tokens.iter().enumerate() {
        let _ = token_index;
        for (peer_index, peer) in peers.iter().enumerate() {
            for (offset, window_index) in windows.iter().enumerate() {
                if offset == 0 && !consider_previous {
                    continue;
                }
                if offset == 2 && !consider_next {
                    continue;
                }
                let expected = token(peer.mac, *window_index, announcement.address);
                if bool::from(expected.ct_eq(token_bytes)) {
                    return Some((peer_index, announcement.address));
                }
            }
        }
    }
    None
}

/// A pinned MAC the listener can recompute. It is not in the announcement.
#[derive(Clone, Copy)]
pub struct PinnedMac<'a> {
    pub mac: &'a [u8; 32],
}

/// The pinned session still has to present a current credential. An
/// announcement does not enroll, and a wrong credential is refused.
pub fn connection_allowed(credential_current: bool) -> bool {
    credential_current
}

fn window_ms() -> Option<u64> {
    let window = u64::try_from(DISCOVERY_WINDOW.as_millis()).ok()?;
    (window > 0).then_some(window)
}

fn token(mac: &[u8; 32], window_index: u64, address: SocketAddr) -> [u8; 32] {
    let (family, octets) = canonical_address(address.ip());
    let mut input = Vec::with_capacity(8 + 1 + octets.len() + 2);
    input.extend_from_slice(&window_index.to_be_bytes());
    input.push(family);
    input.extend_from_slice(&octets);
    input.extend_from_slice(&address.port().to_be_bytes());
    let mut hmac = <Hmac<Sha256>>::new_from_slice(mac).expect("HMAC accepts a 32-byte key");
    hmac.update(&input);
    let bytes = hmac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

/// Family 4 or 6, and the address bytes the MAC covers. An IPv4-mapped IPv6
/// address is family 4 and those four IPv4 bytes.
pub fn canonical_address(ip: IpAddr) -> (u8, Vec<u8>) {
    match ip {
        IpAddr::V4(v4) => (4, v4.octets().to_vec()),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                (4, v4.octets().to_vec())
            } else {
                (6, v6.octets().to_vec())
            }
        }
    }
}

fn encode_announcement(address: SocketAddr, tokens: &[[u8; 32]]) -> Vec<u8> {
    let (family, octets) = canonical_address(address.ip());
    let mut bytes = Vec::with_capacity(1 + 1 + octets.len() + 2 + 2 + tokens.len() * 32);
    bytes.push(1);
    bytes.push(family);
    bytes.extend_from_slice(&octets);
    bytes.extend_from_slice(&address.port().to_be_bytes());
    let count = u16::try_from(tokens.len()).unwrap_or(u16::MAX);
    bytes.extend_from_slice(&count.to_be_bytes());
    for token in tokens.iter().take(usize::from(count)) {
        bytes.extend_from_slice(token);
    }
    bytes
}

/// `None` when the bytes are not an announcement. Trailing bytes are refused,
/// so a fingerprint cannot be appended.
pub fn decode_announcement(bytes: &[u8]) -> Option<Announcement> {
    if bytes.first().copied() != Some(1) || bytes.len() < 1 + 1 + 4 + 2 + 2 {
        return None;
    }
    let family = bytes[1];
    let address_len = match family {
        4 => 4,
        6 => 16,
        _ => return None,
    };
    if bytes.len() < 2 + address_len + 4 {
        return None;
    }
    let octets = &bytes[2..2 + address_len];
    let port = u16::from_be_bytes(bytes[2 + address_len..4 + address_len].try_into().ok()?);
    let count = u16::from_be_bytes(bytes[4 + address_len..6 + address_len].try_into().ok()?);
    let tokens_at = 6 + address_len;
    let token_bytes = usize::from(count).checked_mul(32)?;
    if bytes.len() != tokens_at + token_bytes {
        return None;
    }
    let mut tokens = Vec::with_capacity(usize::from(count));
    for index in 0..usize::from(count) {
        let start = tokens_at + index * 32;
        let mut token = [0u8; 32];
        token.copy_from_slice(&bytes[start..start + 32]);
        tokens.push(token);
    }
    let ip = match family {
        4 => IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3])),
        6 => {
            let mut mapped = [0u8; 16];
            mapped.copy_from_slice(octets);
            IpAddr::V6(Ipv6Addr::from(mapped))
        }
        _ => return None,
    };
    Some(Announcement {
        address: SocketAddr::new(ip, port),
        tokens,
    })
}

/// Encode one discovery message. The tag is the first byte, never `{`, so a
/// refusal envelope cannot be read as a public half.
pub fn encode_message(message: &Outbound) -> Vec<u8> {
    let (tag, generation, public_key) = match message {
        Outbound::Publish {
            generation,
            public_key,
        } => (1u8, *generation, Some(*public_key)),
        Outbound::Stored { generation } => (2, *generation, None),
        Outbound::Installed { generation } => (3, *generation, None),
        Outbound::Reject { generation } => (4, *generation, None),
    };
    let mut bytes = Vec::with_capacity(1 + 32 + 32);
    bytes.push(tag);
    bytes.extend_from_slice(&generation);
    if let Some(public_key) = public_key {
        bytes.extend_from_slice(&public_key);
    }
    bytes
}

pub fn decode_message(bytes: &[u8]) -> Option<Inbound> {
    let tag = *bytes.first()?;
    let generation = bytes.get(1..33)?.try_into().ok()?;
    match tag {
        1 => {
            if bytes.len() != 65 {
                return None;
            }
            let public_key = bytes[33..65].try_into().ok()?;
            Some(Inbound::Publish {
                generation,
                public_key,
            })
        }
        2 => (bytes.len() == 33).then_some(Inbound::Stored { generation }),
        3 => (bytes.len() == 33).then_some(Inbound::Installed { generation }),
        4 => (bytes.len() == 33).then_some(Inbound::Reject { generation }),
        _ => None,
    }
}

/// TXT properties for one announcement payload. Keys are `p0`, `p1`, … and
/// the values are base64 of successive slices. No other key is written.
pub fn announcement_txt(payload: &[u8]) -> Vec<(String, String)> {
    let text = base64::engine::general_purpose::STANDARD.encode(payload);
    text.as_bytes()
        .chunks(180)
        .enumerate()
        .map(|(index, chunk)| {
            (
                format!("p{index}"),
                String::from_utf8(chunk.to_vec()).unwrap_or_default(),
            )
        })
        .collect()
}

pub fn payload_from_txt(properties: &[(impl AsRef<str>, impl AsRef<str>)]) -> Option<Vec<u8>> {
    let mut parts = Vec::new();
    for (key, value) in properties {
        let index: usize = key.as_ref().strip_prefix('p')?.parse().ok()?;
        parts.push((index, value.as_ref()));
    }
    parts.sort_by_key(|(index, _)| *index);
    if parts
        .iter()
        .enumerate()
        .any(|(index, (got, _))| *got != index)
    {
        return None;
    }
    let joined: String = parts.into_iter().map(|(_, value)| value).collect();
    base64::engine::general_purpose::STANDARD
        .decode(joined)
        .ok()
}

/// Instance label for the DNS-SD name. It is not derived from a gateway id.
pub fn instance_label(random: &[u8; 8]) -> String {
    let mut label = String::from("n");
    for byte in random {
        label.push_str(&format!("{byte:02x}"));
    }
    label
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAudit {
    before: String,
    after: String,
    cause: String,
    initiator: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "camelCase")]
enum StoredDiscovery {
    Keyed {
        generation: String,
        private_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit: Option<StoredAudit>,
    },
    Offered {
        generation: String,
        private_key: String,
        peer_public: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit: Option<StoredAudit>,
    },
    Installed {
        generation: String,
        mac: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit: Option<StoredAudit>,
    },
}

/// The discovery field and the audit of the write that stored it, as one JSON
/// object. `None` stays absent from the record.
pub fn discovery_to_json(discovery: &Discovery, audit: Option<&Audit>) -> Option<String> {
    let stored_audit = audit.map(|audit| StoredAudit {
        before: audit.before.to_owned(),
        after: audit.after.to_owned(),
        cause: audit.cause.to_owned(),
        initiator: audit.initiator.to_owned(),
    });
    let stored = match discovery {
        Discovery::None => return None,
        Discovery::Keyed {
            generation,
            private_key,
        } => StoredDiscovery::Keyed {
            generation: b64(generation),
            private_key: b64(private_key),
            audit: stored_audit,
        },
        Discovery::Offered {
            generation,
            private_key,
            peer_public,
        } => StoredDiscovery::Offered {
            generation: b64(generation),
            private_key: b64(private_key),
            peer_public: b64(peer_public),
            audit: stored_audit,
        },
        Discovery::Installed { generation, mac } => StoredDiscovery::Installed {
            generation: b64(generation),
            mac: b64(mac),
            audit: stored_audit,
        },
    };
    serde_json::to_string(&stored).ok()
}

pub fn discovery_from_json(text: &str) -> Option<Discovery> {
    let stored: StoredDiscovery = serde_json::from_str(text).ok()?;
    Some(match stored {
        StoredDiscovery::Keyed {
            generation,
            private_key,
            ..
        } => Discovery::Keyed {
            generation: unb64(&generation)?,
            private_key: unb64(&private_key)?,
        },
        StoredDiscovery::Offered {
            generation,
            private_key,
            peer_public,
            ..
        } => Discovery::Offered {
            generation: unb64(&generation)?,
            private_key: unb64(&private_key)?,
            peer_public: unb64(&peer_public)?,
        },
        StoredDiscovery::Installed {
            generation, mac, ..
        } => Discovery::Installed {
            generation: unb64(&generation)?,
            mac: unb64(&mac)?,
        },
    })
}

/// Why the granting side will not run discovery. None of these enroll a listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryRefusal {
    /// The proved key is not an Active paired gateway.
    NotPaired,
    /// The peer credential is revoked or expired. The phase is left as it was.
    NotCurrent,
    /// The discovery record could not be read or written.
    Unavailable,
}

/// The granting side of one discovery session, on an already pinned TLS connection.
pub trait GrantorDiscovery: Send + Sync {
    fn serve(
        &self,
        peer: nessa_auth::domain::pairing::DeviceKey,
        pipe: &mut dyn DiscoveryPipe,
    ) -> Result<(), DiscoveryRefusal>;
}

/// One framed discovery message on the pinned session, after `openDiscovery`.
pub trait DiscoveryPipe {
    fn send_frame(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn recv_frame(&mut self) -> std::io::Result<Vec<u8>>;
}

/// Why a discovery exchange stopped. `Refused` is the gateway's redacted
/// refusal: the listener is not enrolled and the credential is not accepted.
#[derive(Debug)]
pub enum ExchangeError {
    Io(std::io::Error),
    Refused,
    BadMessage,
}

/// Run one side of the exchange. `sends_first` is the dialer: it writes the
/// resume messages before it reads. Each durable commit is handed to
/// `persist` before its messages are sent. A `{` frame is the enrollment
/// refusal envelope, not a discovery message. Once this side is installed
/// and has sent `installed`, the peer closing the session ends the exchange.
pub fn exchange(
    session: &mut Session,
    pipe: &mut dyn DiscoveryPipe,
    entropy: &mut dyn FnMut() -> [u8; 32],
    persist: &mut dyn FnMut(&Commit) -> Result<(), std::io::Error>,
    sends_first: bool,
) -> Result<(), ExchangeError> {
    let mut need_send = true;
    if !sends_first {
        match read_one(session, pipe, entropy, persist) {
            Ok(_) => need_send = true,
            Err(error) => return finish_or(session, error),
        }
    }
    for _ in 0..12 {
        if need_send {
            for message in session.reconnect() {
                send_frame(session, pipe, &encode_message(&message))?;
            }
            need_send = false;
        }
        if session.discovery.installed_mac().is_some() {
            return Ok(());
        }
        match read_one(session, pipe, entropy, persist) {
            Ok(true) => need_send = true,
            Ok(false) => {}
            Err(error) => return finish_or(session, error),
        }
    }
    if session.discovery.installed_mac().is_some() {
        Ok(())
    } else {
        Err(ExchangeError::BadMessage)
    }
}

fn send_frame(
    session: &Session,
    pipe: &mut dyn DiscoveryPipe,
    bytes: &[u8],
) -> Result<(), ExchangeError> {
    match pipe.send_frame(bytes) {
        Ok(()) => Ok(()),
        Err(error) if session.discovery.installed_mac().is_some() => {
            finish_or(session, ExchangeError::Io(error))
        }
        Err(error) => Err(ExchangeError::Io(error)),
    }
}

fn finish_or(session: &Session, error: ExchangeError) -> Result<(), ExchangeError> {
    match error {
        ExchangeError::Io(ref io)
            if session.discovery.installed_mac().is_some()
                && matches!(
                    io.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                ) =>
        {
            Ok(())
        }
        other => Err(other),
    }
}

/// `Ok(true)` when the field changed and the resume messages should be sent.
fn read_one(
    session: &mut Session,
    pipe: &mut dyn DiscoveryPipe,
    entropy: &mut dyn FnMut() -> [u8; 32],
    persist: &mut dyn FnMut(&Commit) -> Result<(), std::io::Error>,
) -> Result<bool, ExchangeError> {
    let frame = pipe.recv_frame().map_err(ExchangeError::Io)?;
    if frame.first() == Some(&b'{') {
        return Err(ExchangeError::Refused);
    }
    let inbound = decode_message(&frame).ok_or(ExchangeError::BadMessage)?;
    let before = session.discovery.name();
    let (next, commits) = session.receive(inbound, entropy);
    for commit in &commits {
        if commit.durable {
            persist(commit).map_err(ExchangeError::Io)?;
        }
        for message in &commit.send {
            send_frame(session, pipe, &encode_message(message))?;
        }
    }
    let changed = next.discovery.name() != before;
    *session = next;
    Ok(changed)
}

fn b64(bytes: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Option<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    <[u8; 32]>::try_from(bytes).ok()
}

#[cfg(test)]
#[path = "../../tests/discovery.rs"]
mod tests;
