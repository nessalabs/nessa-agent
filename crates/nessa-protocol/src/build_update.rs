//! How a paired environment that speaks another lease protocol gets its
//! gateway's build: frames that never change, so an environment of any
//! protocol and a gateway of any other still understand each other here.
//!
//! ```text
//! environment ── Hello{protocol: P'} ──▶ gateway (P' ≠ P)
//!   gateway ── VersionRefused{protocol: P} ──▶ environment; the connection ends
//! environment: may this gateway update this build? (its own policy) ──▶ no: stays as it is
//! environment dials again ── UpdateWanted{os, arch, libc} ──▶ gateway  (first frame, no Hello)
//!   gateway has no build for that platform ── UpdateRefused{reason} ──▶ environment
//!   otherwise ── Update{digest, size, protocol: P} ──▶ environment
//!             ── UpdatePart{data} … ──▶ environment   (the bytes, in order, base64)
//!             ── UpdateEnd ──▶ environment; the connection ends
//! environment: SHA-256 is digest, size is size, and the copy itself says P?
//!   ──▶ kept by its digest, and run in place of this build
//! ```
//!
//! Arrows are frames, in the order they are sent. Unlike the lease frames
//! (`lease`), which change with every protocol and are checked against the
//! protocol's fingerprint, these are read the same by every build: a gateway
//! refuses an environment it cannot speak to, and the environment learns
//! from the refusal itself which build to fetch. So nothing here may change
//! shape: a field may never be renamed, removed or given another meaning,
//! and a reader ignores fields it does not know
//! (`every_frame_keeps_its_shape`). A new kind of frame is ignored by an
//! older reader, which then ends the exchange as it would for nonsense.
//!
//! The gateway decides nothing about whether an environment takes its build:
//! the environment's owner does, through that environment's own policy. The
//! gateway sends only its own build, or a release build of the same sources
//! it pins, and the environment checks every byte before it runs any.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

/// Most bytes one [`FromGateway::UpdatePart`] carries before base64: well
/// within a lease frame's bound once encoded.
pub const UPDATE_PART_BYTES: usize = 96 * 1024;

/// The largest build an environment takes.
pub const MOST_UPDATE_BYTES: u64 = 1 << 30;

/// How long a gateway may take to send its build, from the ask to its end.
/// The host asking waits that long and a little more, for its own steps.
pub const UPDATE_SEND_DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);

/// What a gateway sends in this exchange.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FromGateway {
    /// The environment's hello named another protocol than `protocol`, the
    /// gateway's: it is not served on this connection, which ends.
    VersionRefused {
        /// The lease protocol the gateway speaks.
        protocol: String,
    },
    /// The build that follows: `size` bytes whose SHA-256 is `digest`, which
    /// says it speaks `protocol` when asked (`nessa env protocol`). Both
    /// are lowercase hex.
    Update {
        digest: String,
        size: u64,
        protocol: String,
    },
    /// The next bytes of the build, base64.
    UpdatePart { data: String },
    /// Every byte was sent.
    UpdateEnd,
    /// No build is sent: `reason` says why, for people.
    UpdateRefused { reason: String },
}

/// What an environment sends in this exchange.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FromEnvironment {
    /// The first frame of a connection that asks for the gateway's build,
    /// for this platform, in the words `nessa env serve`'s install probe
    /// uses: `linux`, `x86_64`, `gnu`.
    UpdateWanted {
        os: String,
        arch: String,
        libc: String,
    },
}

/// The frame of this exchange a gateway's frame body carries, if it is one.
pub fn from_gateway(body: &[u8]) -> Option<FromGateway> {
    serde_json::from_slice(body).ok()
}

/// The frame of this exchange an environment's frame body carries, if it
/// is one.
pub fn from_environment(body: &[u8]) -> Option<FromEnvironment> {
    serde_json::from_slice(body).ok()
}

/// One frame, framed as lease frames are.
///
/// # Errors
/// The frame does not fit a lease frame.
pub fn encode<T: Serialize>(frame: &T) -> Result<Vec<u8>, crate::lease::FrameRefused> {
    crate::lease::encode(frame)
}

/// `bytes` as an [`FromGateway::UpdatePart`].
pub fn part(bytes: &[u8]) -> FromGateway {
    FromGateway::UpdatePart {
        data: STANDARD.encode(bytes),
    }
}

/// The bytes an [`FromGateway::UpdatePart`] carries; `None` when they are
/// not base64.
pub fn part_bytes(data: &str) -> Option<Vec<u8>> {
    STANDARD.decode(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn wire<T: Serialize>(frame: &T) -> Value {
        serde_json::to_value(frame).unwrap()
    }

    /// Every build reads these alike: a change here strands environments
    /// that run an older build, with no way left to update them.
    #[test]
    fn every_frame_keeps_its_shape() {
        assert_eq!(
            wire(&FromGateway::VersionRefused {
                protocol: "0123456789abcdef".into()
            }),
            json!({"type": "versionRefused", "protocol": "0123456789abcdef"})
        );
        assert_eq!(
            wire(&FromGateway::Update {
                digest: "ab".into(),
                size: 3,
                protocol: "cd".into()
            }),
            json!({"type": "update", "digest": "ab", "size": 3, "protocol": "cd"})
        );
        assert_eq!(
            wire(&part(b"abc")),
            json!({"type": "updatePart", "data": "YWJj"})
        );
        assert_eq!(wire(&FromGateway::UpdateEnd), json!({"type": "updateEnd"}));
        assert_eq!(
            wire(&FromGateway::UpdateRefused {
                reason: "platform".into()
            }),
            json!({"type": "updateRefused", "reason": "platform"})
        );
        assert_eq!(
            wire(&FromEnvironment::UpdateWanted {
                os: "linux".into(),
                arch: "x86_64".into(),
                libc: "gnu".into()
            }),
            json!({"type": "updateWanted", "os": "linux", "arch": "x86_64", "libc": "gnu"})
        );
    }

    #[test]
    fn a_reader_ignores_fields_it_does_not_know() {
        assert_eq!(
            from_gateway(br#"{"type":"versionRefused","protocol":"p","later":1}"#),
            Some(FromGateway::VersionRefused {
                protocol: "p".into()
            })
        );
        assert_eq!(
            from_environment(
                br#"{"type":"updateWanted","os":"linux","arch":"x86_64","libc":"gnu","more":true}"#
            ),
            Some(FromEnvironment::UpdateWanted {
                os: "linux".into(),
                arch: "x86_64".into(),
                libc: "gnu".into()
            })
        );
    }

    #[test]
    fn a_lease_frame_is_not_one_of_these() {
        assert_eq!(from_gateway(br#"{"type":"keepalive"}"#), None);
        assert_eq!(
            from_environment(br#"{"type":"hello","build":"1","protocol":"p","workspace":"/w"}"#),
            None
        );
    }

    #[test]
    fn a_part_carries_its_bytes_back() {
        let FromGateway::UpdatePart { data } = part(&[0, 1, 2, 255]) else {
            unreachable!()
        };
        assert_eq!(part_bytes(&data), Some(vec![0, 1, 2, 255]));
        assert_eq!(part_bytes("not base64!"), None);
    }

    #[test]
    fn a_full_part_fits_a_lease_frame() {
        encode(&part(&vec![7; UPDATE_PART_BYTES])).unwrap();
    }
}
