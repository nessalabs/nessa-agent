//! What a gateway and its readers agree on to meet through a web tunnel
//! (`docs/design/auth/tunnel.md`): the public address a reader opens, the
//! one path on it, and the WebSocket limits both ends hold.
//!
//! ```text
//! reader ── wss://<host>/native ──▶ tunnel edge ──▶ the gateway's loopback ingress
//!   each binary message: the next bytes of the native connection, both ways
//! ```
//!
//! The address carries no trust. Whoever answers there must still complete
//! the native TLS handshake with the key the reader pinned at pairing.
use base64::Engine;
use std::{fmt, net::IpAddr, time::Duration};

/// The one path the ingress upgrades.
pub const TUNNEL_PATH: &str = "/native";
/// Header a paired device sends on every tunnel request. A client that
/// cannot set headers puts the same value in the path instead:
/// `/native/<secret>`.
pub const TUNNEL_SECRET_HEADER: &str = "x-nessa-tunnel";
/// Bytes of a device's tunnel secret.
pub const TUNNEL_SECRET_BYTES: usize = 32;

/// A paired device's tunnel secret. Debug never prints the bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TunnelSecret([u8; TUNNEL_SECRET_BYTES]);

impl TunnelSecret {
    /// A secret of exactly [`TUNNEL_SECRET_BYTES`] bytes.
    pub fn new(bytes: [u8; TUNNEL_SECRET_BYTES]) -> Self {
        Self(bytes)
    }

    /// The bytes to compare or send. Not for logs.
    pub fn expose(&self) -> &[u8; TUNNEL_SECRET_BYTES] {
        &self.0
    }

    /// The header and path value: unpadded URL-safe base64.
    pub fn encode(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0)
    }

    /// The inverse of [`Self::encode`]. Anything else is refused.
    pub fn decode(text: &str) -> Option<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .ok()?;
        let bytes: [u8; TUNNEL_SECRET_BYTES] = bytes.try_into().ok()?;
        Some(Self(bytes))
    }
}

impl fmt::Debug for TunnelSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TunnelSecret(..)")
    }
}
/// How often each end of a bridge pings the other, so a tunnel's idle limit
/// (Cloudflare closes a WebSocket silent for 100 s) never ends a quiet session.
pub const TUNNEL_PING: Duration = Duration::from_secs(20);
/// The largest WebSocket message either end accepts. A bridge sends at most
/// [`TUNNEL_CHUNK`] bytes at a time; anything larger is not ours.
pub const MAX_TUNNEL_MESSAGE: usize = 64 * 1024;
/// How many bytes a bridge reads from its socket before sending them on.
pub const TUNNEL_CHUNK: usize = 16 * 1024;
/// The longest public address, scheme included.
pub const MAX_TUNNEL_ADDRESS: usize = 270;

/// A tunnel's public origin: `https://host` or `https://host:port`, with no
/// path, query or credentials. `http://` is admitted only for a loopback
/// address, which is how a test stands in for the edge; anything that leaves
/// the machine goes over HTTPS.
#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TunnelAddress {
    origin: String,
    secure: bool,
    host: String,
    port: u16,
}

impl TunnelAddress {
    /// The origin as written: what the gateway hands out and Settings shows.
    pub fn as_str(&self) -> &str {
        &self.origin
    }
    /// Whether the WebSocket goes over TLS (`wss://`).
    pub fn secure(&self) -> bool {
        self.secure
    }
    /// The host to connect to and name in TLS, without brackets.
    pub fn host(&self) -> &str {
        &self.host
    }
    /// The port, the scheme's default when none is written.
    pub fn port(&self) -> u16 {
        self.port
    }
    /// The WebSocket URL a reader opens.
    pub fn websocket_url(&self) -> String {
        let scheme = if self.secure { "wss" } else { "ws" };
        let rest = self.origin.split_once("://").map_or("", |(_, rest)| rest);
        format!("{scheme}://{rest}{TUNNEL_PATH}")
    }
}

impl fmt::Debug for TunnelAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("TunnelAddress")
            .field(&self.origin)
            .finish()
    }
}

impl From<TunnelAddress> for String {
    fn from(address: TunnelAddress) -> Self {
        address.origin
    }
}

impl TryFrom<String> for TunnelAddress {
    type Error = String;
    fn try_from(origin: String) -> Result<Self, Self::Error> {
        let refuse = || {
            format!(
                "tunnel must be https://host[:port], or http:// to a loopback address, at most {MAX_TUNNEL_ADDRESS} bytes"
            )
        };
        if origin.len() > MAX_TUNNEL_ADDRESS || !origin.is_ascii() {
            return Err(refuse());
        }
        let (secure, rest) = if let Some(rest) = origin.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = origin.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(refuse());
        };
        let (host, port) = authority(rest).ok_or_else(refuse)?;
        let port = port.unwrap_or(if secure { 443 } else { 80 });
        if port == 0 {
            return Err(refuse());
        }
        let literal = host.parse::<IpAddr>().ok();
        if !secure && !literal.is_some_and(|address| address.is_loopback()) {
            return Err(refuse());
        }
        if literal.is_none() && !dns_name(host) {
            return Err(refuse());
        }
        Ok(Self {
            host: host.to_owned(),
            origin,
            secure,
            port,
        })
    }
}

/// `host` or `host:port`, an IPv6 host in brackets, nothing else after it.
/// The host comes back without brackets.
fn authority(rest: &str) -> Option<(&str, Option<u16>)> {
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '\\']) {
        return None;
    }
    let (host, port) = match rest.strip_prefix('[') {
        Some(bracketed) => {
            let (v6, after) = bracketed.split_once(']')?;
            v6.parse::<std::net::Ipv6Addr>().ok()?;
            match after {
                "" => (v6, None),
                after => (v6, Some(after.strip_prefix(':')?)),
            }
        }
        None => match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        },
    };
    let port = match port {
        None => None,
        Some(port) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
            Some(port.parse().ok()?)
        }
        Some(_) => return None,
    };
    (!host.is_empty()).then_some((host, port))
}

/// A DNS name: dot-separated labels of letters, digits and inner hyphens.
fn dns_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::TunnelAddress;

    fn parse(origin: &str) -> Result<TunnelAddress, String> {
        TunnelAddress::try_from(origin.to_owned())
    }

    #[test]
    fn an_https_origin_or_a_loopback_http_one_is_a_tunnel() {
        let quick = parse("https://bright-river-1234.trycloudflare.com").unwrap();
        assert!(quick.secure());
        assert_eq!(quick.host(), "bright-river-1234.trycloudflare.com");
        assert_eq!(quick.port(), 443);
        assert_eq!(
            quick.websocket_url(),
            "wss://bright-river-1234.trycloudflare.com/native"
        );
        let named = parse("https://mac.example.com:8443").unwrap();
        assert_eq!(named.port(), 8443);
        let local = parse("http://127.0.0.1:40123").unwrap();
        assert!(!local.secure());
        assert_eq!(local.websocket_url(), "ws://127.0.0.1:40123/native");
        let v6 = parse("http://[::1]:40123").unwrap();
        assert_eq!(v6.host(), "::1");
        assert_eq!(v6.websocket_url(), "ws://[::1]:40123/native");
    }

    #[test]
    fn anything_else_is_refused() {
        for origin in [
            "",
            "trycloudflare.com",
            "ftp://mac.example.com",
            "http://mac.example.com",
            "http://192.0.2.1:80",
            "https://mac.example.com/native",
            "https://mac.example.com?x=1",
            "https://user@mac.example.com",
            "https://mac.example.com:",
            "https://mac.example.com:0",
            "https://mac.example.com:65536",
            "https://mac.example.com:+1",
            "https://-bad.example.com",
            "https://bad..example.com",
            "https://exa mple.com",
            "https://exämple.com",
            "https://[::1",
            "https://[::1]x",
        ] {
            assert!(parse(origin).is_err(), "{origin:?} is refused");
        }
        assert!(parse(&format!("https://{}.com", "a".repeat(270))).is_err());
    }
}
