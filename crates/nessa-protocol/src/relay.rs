//! Relay frames: the preamble a gateway and a reader each speak to a relay
//! before the relay joins their two connections and only copies bytes
//! (issue #266, `docs/design/auth/relay.md`).
//!
//! ```text
//! gateway ── Serve{key} ──▶ relay ── Challenge{nonce} ──▶ gateway ── Proof{signature} ──▶ relay
//!   relay ── Registered ──▶ gateway   (this connection is now the key's serve connection)
//!   relay ── Ping ──▶ gateway ── Pong ──▶ relay   (every PING_INTERVAL)
//! reader ── Connect{key} ──▶ relay
//!   no serve connection for key: relay ── Offline ──▶ reader, closed
//!   else relay ── Incoming{token} ──▶ gateway (on its serve connection)
//!     gateway ── Accept{key, token} ──▶ relay (on a new connection)
//!     relay ── Joined ──▶ reader and gateway; then their own bytes, end to end
//!     no Accept within JOIN_DEADLINE: relay ── NoAnswer ──▶ reader, closed
//! ```
//!
//! Arrows are frames, in order. A frame is a version byte, a type byte and
//! a length byte, then a body whose length each type fixes: no frame is
//! longer than [`MAX_FRAME`] bytes, and anything else is
//! [`RelayFrameError`]. Readers of the preamble read exactly a header and
//! then exactly its body, never through a buffer, so the bytes after
//! `Joined` are left on the socket for the two ends.
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    fmt,
    io::{self, Read, Write},
    time::Duration,
};

/// The preamble version this build speaks.
pub const VERSION: u8 = 1;
/// Version, type and body length.
pub const HEADER: usize = 3;
/// The longest body a relay frame carries. A signed tunnel origin is an
/// Ed25519 signature ([`ORIGIN_SIGNATURE`] bytes) and then the origin text.
pub const MAX_PUBLISHED: usize = 255;
/// Ed25519 signature prefixed to a published tunnel origin.
pub const ORIGIN_SIGNATURE: usize = 64;
/// The longest origin text that still fits once its signature is prefixed.
/// Longer than this is not published; a quick or named hostname fits.
pub const MAX_ORIGIN: usize = MAX_PUBLISHED - ORIGIN_SIGNATURE;
/// The longest frame: a header and a published tunnel origin.
pub const MAX_FRAME: usize = HEADER + MAX_PUBLISHED;
/// How long a connection may take to send its whole preamble, or a
/// gateway its proof.
pub const PREAMBLE_DEADLINE: Duration = Duration::from_secs(10);
/// How long a reader waits for the gateway it asked for to accept.
pub const JOIN_DEADLINE: Duration = Duration::from_secs(5);
/// How long a reader's direct dial has to connect before it asks the relay
/// too. A direct dial to a home address from another network usually times
/// out rather than being refused, so without this the relay would wait out
/// the whole connect budget.
pub const DIRECT_HEAD_START: Duration = Duration::from_millis(750);
/// How often the relay pings a serve connection.
pub const PING_INTERVAL: Duration = Duration::from_secs(15);
/// Pings a serve connection may leave unanswered before the relay drops
/// it: so a gateway that went away is `Offline` within
/// `PING_INTERVAL * (MISSED_PINGS + 1)`.
pub const MISSED_PINGS: u32 = 3;

/// A gateway's raw Ed25519 public key: the id it registers under.
pub type GatewayKey = [u8; 32];
/// The relay's name for one waiting reader, sent only to the gateway it
/// asked for.
pub type JoinToken = [u8; 16];

/// One preamble frame.
#[derive(Clone, PartialEq, Eq)]
pub enum RelayFrame {
    /// A gateway asks to be registered under `key`.
    Serve {
        /// The key it must prove.
        key: GatewayKey,
    },
    /// A reader asks to be joined to the gateway registered under `key`.
    Connect {
        /// The gateway it wants.
        key: GatewayKey,
    },
    /// A gateway takes the reader the relay named `token`.
    Accept {
        /// The gateway's own key.
        key: GatewayKey,
        /// The token its serve connection was sent.
        token: JoinToken,
    },
    /// The relay's fresh nonce for a `Serve`.
    Challenge {
        /// What the gateway signs.
        nonce: [u8; 32],
    },
    /// The gateway's signature over the relay proof message for the nonce.
    Proof {
        /// An Ed25519 signature.
        signature: [u8; 64],
    },
    /// The proof verified: this is the key's serve connection.
    Registered,
    /// A reader waits for the gateway under `token`.
    Incoming {
        /// What the gateway names in its `Accept`.
        token: JoinToken,
    },
    /// The relay asks a serve connection whether it is still there.
    Ping,
    /// The gateway answers a `Ping`.
    Pong,
    /// The two connections are joined; what follows is the other end's.
    Joined,
    /// The relay holds no serve connection for the key asked for.
    Offline,
    /// The key has a serve connection, but its gateway did not accept in
    /// time.
    NoAnswer,
    /// The gateway names its current tunnel origin. Empty clears it.
    Publish {
        /// The origin, or none.
        origin: Box<PublishedOrigin>,
    },
    /// A reader asks for the tunnel origin of the gateway registered under `key`.
    Lookup {
        /// The gateway it wants.
        key: GatewayKey,
    },
    /// The reader's proof it holds a paired device's tunnel secret.
    Seal {
        /// HMAC over the lookup context, the gateway key and the challenge.
        mac: [u8; 32],
    },
    /// The relay asks the gateway whether `mac` is a paired device's.
    Admit {
        /// Which lookup this answers.
        token: JoinToken,
        /// The challenge the reader sealed.
        nonce: [u8; 32],
        /// The reader's seal.
        mac: [u8; 32],
    },
    /// The gateway accepts the lookup `token` names.
    Allow {
        /// The lookup to answer.
        token: JoinToken,
    },
    /// The gateway refuses the lookup `token` names.
    Deny {
        /// The lookup to refuse.
        token: JoinToken,
    },
    /// The tunnel origin for a lookup the gateway allowed. Empty means none.
    Located {
        /// The origin, or none.
        origin: Box<PublishedOrigin>,
    },
    /// The relay will not say the origin: the gateway refused, or the
    /// lookup could not be asked.
    Unlisted,
}

/// A tunnel origin carried in a relay frame. Empty means there is none.
/// The bytes past `len` are zero.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublishedOrigin {
    len: u8,
    bytes: [u8; MAX_PUBLISHED],
}

impl PublishedOrigin {
    /// `text` as an origin, before the gateway signs it. Empty, non-ASCII, or
    /// longer than [`MAX_ORIGIN`] does not fit in a signed frame.
    pub fn new(text: &str) -> Option<Self> {
        Self::raw(text.as_bytes()).filter(|_| !text.is_empty() && text.is_ascii())
    }

    /// The bytes a relay carries for `text`: its Ed25519 signature, then the text.
    pub fn signed(signature: &[u8; ORIGIN_SIGNATURE], text: &str) -> Option<Self> {
        if text.is_empty() || !text.is_ascii() || text.len() > MAX_ORIGIN {
            return None;
        }
        let mut raw = [0; MAX_PUBLISHED];
        raw[..ORIGIN_SIGNATURE].copy_from_slice(signature);
        raw[ORIGIN_SIGNATURE..ORIGIN_SIGNATURE + text.len()].copy_from_slice(text.as_bytes());
        Some(Self {
            len: u8::try_from(ORIGIN_SIGNATURE + text.len()).unwrap_or(u8::MAX),
            bytes: raw,
        })
    }

    /// The signature and origin text, when this body is a signed origin.
    pub fn signature_and_text(&self) -> Option<([u8; ORIGIN_SIGNATURE], &str)> {
        let bytes = self.as_bytes();
        if bytes.len() <= ORIGIN_SIGNATURE {
            return None;
        }
        let mut signature = [0; ORIGIN_SIGNATURE];
        signature.copy_from_slice(&bytes[..ORIGIN_SIGNATURE]);
        let text = std::str::from_utf8(&bytes[ORIGIN_SIGNATURE..]).ok()?;
        if text.is_empty() || !text.is_ascii() {
            return None;
        }
        Some((signature, text))
    }

    fn raw(raw: &[u8]) -> Option<Self> {
        if raw.is_empty() || raw.len() > MAX_ORIGIN {
            return None;
        }
        let mut bytes = [0; MAX_PUBLISHED];
        bytes[..raw.len()].copy_from_slice(raw);
        Some(Self {
            len: u8::try_from(raw.len()).unwrap_or(u8::MAX),
            bytes,
        })
    }

    /// No origin.
    pub fn clear() -> Self {
        Self {
            len: 0,
            bytes: [0; MAX_PUBLISHED],
        }
    }

    /// The origin bytes, with no padding.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// The origin text, or `None` when it is empty or not UTF-8.
    pub fn text(&self) -> Option<&str> {
        if self.len == 0 {
            None
        } else {
            std::str::from_utf8(self.as_bytes()).ok()
        }
    }
}

impl fmt::Debug for PublishedOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((_, text)) = self.signature_and_text() {
            return write!(formatter, "PublishedOrigin({text})");
        }
        match self.text() {
            Some(text) => write!(formatter, "PublishedOrigin({text})"),
            None => formatter.write_str("PublishedOrigin(none)"),
        }
    }
}

/// HMAC-SHA256 over `nessa-tunnel-lookup-v1`, the gateway key and `nonce`,
/// keyed by the device's tunnel secret. The relay sees the mac, not the secret.
pub fn tunnel_lookup_mac(secret: &[u8; 32], gateway: &GatewayKey, nonce: &[u8; 32]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("hmac accepts a 32 byte key");
    mac.update(b"nessa-tunnel-lookup-v1\0");
    mac.update(gateway);
    mac.update(nonce);
    let bytes = mac.finalize().into_bytes();
    let mut out = [0; 32];
    out.copy_from_slice(&bytes);
    out
}

impl fmt::Debug for RelayFrame {
    /// Keys and tokens are shown short, signatures and nonces not at all.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serve { key } => write!(f, "Serve({})", key_id(key)),
            Self::Connect { key } => write!(f, "Connect({})", key_id(key)),
            Self::Accept { key, .. } => write!(f, "Accept({})", key_id(key)),
            Self::Challenge { .. } => f.write_str("Challenge"),
            Self::Proof { .. } => f.write_str("Proof"),
            Self::Registered => f.write_str("Registered"),
            Self::Incoming { .. } => f.write_str("Incoming"),
            Self::Ping => f.write_str("Ping"),
            Self::Pong => f.write_str("Pong"),
            Self::Joined => f.write_str("Joined"),
            Self::Offline => f.write_str("Offline"),
            Self::NoAnswer => f.write_str("NoAnswer"),
            Self::Publish { origin } => write!(f, "Publish({origin:?})"),
            Self::Lookup { key } => write!(f, "Lookup({})", key_id(key)),
            Self::Seal { .. } => f.write_str("Seal"),
            Self::Admit { .. } => f.write_str("Admit"),
            Self::Allow { .. } => f.write_str("Allow"),
            Self::Deny { .. } => f.write_str("Deny"),
            Self::Located { origin } => write!(f, "Located({origin:?})"),
            Self::Unlisted => f.write_str("Unlisted"),
        }
    }
}

/// Longest relay address: a DNS name's limit.
pub const MAX_RELAY_ADDRESS: usize = 255;

/// Where a relay listens: `host:port`, where the host is a name or an
/// address (an IPv6 one in brackets). Unlike a gateway's native address it
/// may be a name, since a relay is usually somewhere public; it is looked up
/// at each dial. The address carries no trust: whoever answers there is
/// asked for a key and joins nothing it cannot prove.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelayAddress(String);

impl RelayAddress {
    /// The `host:port` to dial.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<RelayAddress> for String {
    fn from(address: RelayAddress) -> Self {
        address.0
    }
}

impl TryFrom<String> for RelayAddress {
    type Error = String;
    fn try_from(address: String) -> Result<Self, Self::Error> {
        let refuse = || format!("relay must be host:port, at most {MAX_RELAY_ADDRESS} bytes");
        if address.len() > MAX_RELAY_ADDRESS
            || address.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(refuse());
        }
        match host_port(&address) {
            Some(port) if port != 0 => Ok(Self(address)),
            _ => Err(refuse()),
        }
    }
}

/// The port of `address` when it has the form `host:port`, the host a name
/// or an address (an IPv6 one in brackets); nothing is looked up. A relay
/// address and a gateway's dialed address share this shape.
pub fn host_port(address: &str) -> Option<u16> {
    let (host, port) = address.rsplit_once(':')?;
    let host_ok = !host.is_empty()
        && match host.strip_prefix('[') {
            Some(bracketed) => bracketed
                .strip_suffix(']')
                .is_some_and(|v6| v6.parse::<std::net::Ipv6Addr>().is_ok()),
            None => !host.contains(':'),
        };
    host_ok.then(|| port.parse().ok()).flatten()
}

/// A key shortened for logs: its first four bytes in hex.
pub fn key_id(key: &GatewayKey) -> String {
    key[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Why a relay did not join a reader to the gateway it asked for: its
/// `Offline` or `NoAnswer`, carried inside the reader's `ConnectionRefused`
/// error so whoever reports the failure can tell `away` from
/// `unreachable` (`docs/design/auth/relay.md`, "The sleeping gateway").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayRefusal {
    /// The gateway is not registered at the relay.
    Offline,
    /// The gateway is registered but did not accept in time.
    NoAnswer,
}

impl RelayRefusal {
    /// The refusal a reader's relay leg ends with for `frame`, if it is one.
    pub fn of(frame: RelayFrame) -> Option<Self> {
        match frame {
            RelayFrame::Offline => Some(Self::Offline),
            RelayFrame::NoAnswer => Some(Self::NoAnswer),
            _ => None,
        }
    }
    /// The refusal `error` carries, if a relay leg made it.
    pub fn carried(error: &io::Error) -> Option<Self> {
        error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<Self>())
            .copied()
    }
}

impl From<RelayRefusal> for io::Error {
    fn from(refusal: RelayRefusal) -> Self {
        io::Error::new(io::ErrorKind::ConnectionRefused, refusal)
    }
}

impl fmt::Display for RelayRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Offline => "the gateway is not registered at the relay",
            Self::NoAnswer => "the gateway did not accept through the relay in time",
        })
    }
}

impl std::error::Error for RelayRefusal {}

/// Why bytes are not a relay frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayFrameError {
    /// Another preamble version.
    Version(u8),
    /// A type this build does not know.
    Type(u8),
    /// A body length the type does not have.
    Length {
        /// The frame's type.
        kind: u8,
        /// The length it declared.
        length: u8,
    },
}

impl fmt::Display for RelayFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(version) => write!(f, "relay preamble version {version}"),
            Self::Type(kind) => write!(f, "unknown relay frame type {kind}"),
            Self::Length { kind, length } => {
                write!(f, "relay frame type {kind} cannot be {length} bytes long")
            }
        }
    }
}

impl std::error::Error for RelayFrameError {}

const SERVE: u8 = 1;
const CONNECT: u8 = 2;
const ACCEPT: u8 = 3;
const CHALLENGE: u8 = 4;
const PROOF: u8 = 5;
const REGISTERED: u8 = 6;
const INCOMING: u8 = 7;
const PING: u8 = 8;
const PONG: u8 = 9;
const JOINED: u8 = 10;
const OFFLINE: u8 = 11;
const NO_ANSWER: u8 = 12;
const PUBLISH: u8 = 13;
const LOOKUP: u8 = 14;
const SEAL: u8 = 15;
const ADMIT: u8 = 16;
const ALLOW: u8 = 17;
const DENY: u8 = 18;
const LOCATED: u8 = 19;
const UNLISTED: u8 = 20;

/// How long a body may be. `Fixed` is exact. `AtMost` is a published origin.
enum Body {
    Fixed(usize),
    AtMost(usize),
}

/// The body rule for a frame type, or `None` for an unknown type.
fn body_rule(kind: u8) -> Option<Body> {
    match kind {
        SERVE | CONNECT | CHALLENGE | LOOKUP | SEAL => Some(Body::Fixed(32)),
        ACCEPT => Some(Body::Fixed(48)),
        PROOF => Some(Body::Fixed(64)),
        INCOMING | ALLOW | DENY => Some(Body::Fixed(16)),
        ADMIT => Some(Body::Fixed(80)),
        REGISTERED | PING | PONG | JOINED | OFFLINE | NO_ANSWER | UNLISTED => Some(Body::Fixed(0)),
        PUBLISH | LOCATED => Some(Body::AtMost(MAX_PUBLISHED)),
        _ => None,
    }
}

/// A checked header: how many body bytes to read next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    kind: u8,
    length: usize,
}

impl Header {
    /// How many body bytes follow it.
    pub fn body(self) -> usize {
        self.length
    }
}

/// Check a header before reading its body: the version, a known type, and
/// the length that type has.
///
/// # Errors
/// The version, type or length is not one this build speaks.
pub fn parse_header(bytes: [u8; HEADER]) -> Result<Header, RelayFrameError> {
    let [version, kind, length] = bytes;
    if version != VERSION {
        return Err(RelayFrameError::Version(version));
    }
    let length = usize::from(length);
    let length = match body_rule(kind).ok_or(RelayFrameError::Type(kind))? {
        Body::Fixed(expected) if length == expected => expected,
        Body::AtMost(max) if length <= max => length,
        Body::Fixed(_) | Body::AtMost(_) => {
            return Err(RelayFrameError::Length {
                kind,
                length: u8::try_from(length).unwrap_or(u8::MAX),
            })
        }
    };
    Ok(Header { kind, length })
}

/// The frame a checked header and its body make.
///
/// # Errors
/// `body` is not as long as the header said.
pub fn parse_body(header: Header, body: &[u8]) -> Result<RelayFrame, RelayFrameError> {
    if body.len() != header.length {
        return Err(RelayFrameError::Length {
            kind: header.kind,
            length: u8::try_from(body.len()).unwrap_or(u8::MAX),
        });
    }
    let array32 = |from: usize| -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes.copy_from_slice(&body[from..from + 32]);
        bytes
    };
    let token = |from: usize| -> JoinToken {
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&body[from..from + 16]);
        bytes
    };
    Ok(match header.kind {
        SERVE => RelayFrame::Serve { key: array32(0) },
        CONNECT => RelayFrame::Connect { key: array32(0) },
        ACCEPT => RelayFrame::Accept {
            key: array32(0),
            token: token(32),
        },
        CHALLENGE => RelayFrame::Challenge { nonce: array32(0) },
        PROOF => {
            let mut signature = [0; 64];
            signature.copy_from_slice(body);
            RelayFrame::Proof { signature }
        }
        REGISTERED => RelayFrame::Registered,
        INCOMING => RelayFrame::Incoming { token: token(0) },
        PING => RelayFrame::Ping,
        PONG => RelayFrame::Pong,
        JOINED => RelayFrame::Joined,
        OFFLINE => RelayFrame::Offline,
        NO_ANSWER => RelayFrame::NoAnswer,
        PUBLISH => RelayFrame::Publish {
            origin: Box::new(published(body)),
        },
        LOOKUP => RelayFrame::Lookup { key: array32(0) },
        SEAL => RelayFrame::Seal { mac: array32(0) },
        ADMIT => RelayFrame::Admit {
            token: token(0),
            nonce: array32(16),
            mac: array32(48),
        },
        ALLOW => RelayFrame::Allow { token: token(0) },
        DENY => RelayFrame::Deny { token: token(0) },
        LOCATED => RelayFrame::Located {
            origin: Box::new(published(body)),
        },
        UNLISTED => RelayFrame::Unlisted,
        kind => return Err(RelayFrameError::Type(kind)),
    })
}

fn published(body: &[u8]) -> PublishedOrigin {
    let mut bytes = [0; MAX_PUBLISHED];
    bytes[..body.len()].copy_from_slice(body);
    PublishedOrigin {
        len: u8::try_from(body.len()).unwrap_or(u8::MAX),
        bytes,
    }
}

impl RelayFrame {
    /// The frame's bytes: header, then body.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, body): (u8, &[&[u8]]) = match self {
            Self::Serve { key } => (SERVE, &[key]),
            Self::Connect { key } => (CONNECT, &[key]),
            Self::Accept { key, token } => (ACCEPT, &[key, token]),
            Self::Challenge { nonce } => (CHALLENGE, &[nonce]),
            Self::Proof { signature } => (PROOF, &[signature]),
            Self::Registered => (REGISTERED, &[]),
            Self::Incoming { token } => (INCOMING, &[token]),
            Self::Ping => (PING, &[]),
            Self::Pong => (PONG, &[]),
            Self::Joined => (JOINED, &[]),
            Self::Offline => (OFFLINE, &[]),
            Self::NoAnswer => (NO_ANSWER, &[]),
            Self::Publish { origin } => (PUBLISH, &[origin.as_bytes()]),
            Self::Lookup { key } => (LOOKUP, &[key]),
            Self::Seal { mac } => (SEAL, &[mac]),
            Self::Admit { token, nonce, mac } => (ADMIT, &[token, nonce, mac]),
            Self::Allow { token } => (ALLOW, &[token]),
            Self::Deny { token } => (DENY, &[token]),
            Self::Located { origin } => (LOCATED, &[origin.as_bytes()]),
            Self::Unlisted => (UNLISTED, &[]),
        };
        let length: usize = body.iter().map(|part| part.len()).sum();
        let mut bytes = Vec::with_capacity(HEADER + length);
        bytes.extend_from_slice(&[
            VERSION,
            kind,
            u8::try_from(length).expect("every relay frame body is under 256 bytes"),
        ]);
        for part in body {
            bytes.extend_from_slice(part);
        }
        bytes
    }
}

/// Read exactly one frame from a blocking stream, and not a byte more.
///
/// # Errors
/// The stream failed or ended, or what it sent is not a frame
/// (`InvalidData`).
pub fn read_frame(stream: &mut impl Read) -> io::Result<RelayFrame> {
    let mut header = [0; HEADER];
    stream.read_exact(&mut header)?;
    let header = parse_header(header).map_err(invalid)?;
    let mut body = [0; MAX_FRAME - HEADER];
    let body = &mut body[..header.body()];
    stream.read_exact(body)?;
    parse_body(header, body).map_err(invalid)
}

/// Write one frame to a blocking stream.
///
/// # Errors
/// The stream failed.
pub fn write_frame(stream: &mut impl Write, frame: &RelayFrame) -> io::Result<()> {
    stream.write_all(&frame.encode())?;
    stream.flush()
}

fn invalid(error: RelayFrameError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
#[path = "../tests/relay/frames.rs"]
mod tests;
