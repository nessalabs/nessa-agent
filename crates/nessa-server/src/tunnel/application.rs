//! What the gateway knows about its web tunnel, whichever provider carries
//! it (`docs/design/auth/tunnel.md`): the saved setting, and the status its
//! service publishes. A provider is a [`Service`] that reports the tunnel's
//! public address once it is ready; the bridge, the ingress, the listener's
//! edge and every reader know nothing of which provider it is.
//!
//! ```text
//! config.json native.tunnel + the token file ──▶ TunnelSetting ──▶ composition picks the provider's Service
//! ServiceSupervisor<TunnelAddress> ──▶ watch<TunnelStatus> ──▶ pairing.tunnel, server.reach
//! ```
//!
//! [`Service`]: crate::services::application::Service
use crate::services::application::ServiceStatus;
use nessa_protocol::tunnel::TunnelAddress;
use sha2::{Digest, Sha256};
use std::{fmt, net::SocketAddr};
use tokio::sync::watch;

/// The tunnel's status: its public address once ready.
pub(crate) type TunnelStatus = ServiceStatus<TunnelAddress>;

/// Where the tunnel's provider forwards each connection to, when
/// `config.json` names nowhere else: a loopback port beside the native
/// listener's. A named Cloudflare tunnel is pointed at it in Cloudflare's
/// dashboard, so it stays the same across starts.
pub(crate) const DEFAULT_TUNNEL_INGRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 47651);

/// How the tunnel is carried, as Settings names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TunnelMode {
    /// A free quick tunnel, at a new random address each time its program
    /// starts.
    Quick,
    /// One the person made with their provider, at their own hostname.
    Named,
}

impl TunnelMode {
    /// The mode's wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Named => "named",
        }
    }
}

/// The tunnel as saved and turned on.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TunnelSetting {
    /// The loopback address the provider forwards to.
    pub(crate) ingress: SocketAddr,
    pub(crate) provider: TunnelProvider,
}

/// Which provider carries the tunnel, and how.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum TunnelProvider {
    Cloudflare(CloudflareTunnel),
}

/// A Cloudflare tunnel: a quick one with a new random address each start, or
/// one the person made in Cloudflare's dashboard, at their own hostname.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum CloudflareTunnel {
    Quick,
    Named {
        address: TunnelAddress,
        token: TunnelToken,
    },
}

/// A named tunnel's token: what lets `cloudflared` run it. Never shown,
/// logged or sent back.
#[derive(Clone, PartialEq, Eq)]
pub struct TunnelToken(String);

impl TunnelToken {
    /// The longest token kept. Cloudflare's are a few hundred bytes.
    pub const MAX_BYTES: usize = 4096;

    /// A token: printable ASCII with no spaces, within [`Self::MAX_BYTES`].
    pub fn new(token: String) -> Option<Self> {
        (!token.is_empty()
            && token.len() <= Self::MAX_BYTES
            && token.bytes().all(|byte| byte.is_ascii_graphic()))
        .then_some(Self(token))
    }

    /// The token itself, for the file `cloudflared` reads it from.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Its SHA-256, as lowercase hex: enough to tell two tokens apart
    /// without keeping either.
    pub(crate) fn digest(&self) -> String {
        Sha256::digest(self.0.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl fmt::Debug for TunnelToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TunnelToken(..)")
    }
}

impl fmt::Debug for TunnelSetting {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TunnelSetting")
            .field("ingress", &self.ingress)
            .field("mode", &self.mode())
            .finish()
    }
}

impl TunnelSetting {
    /// Its mode.
    pub(crate) fn mode(&self) -> TunnelMode {
        match &self.provider {
            TunnelProvider::Cloudflare(CloudflareTunnel::Quick) => TunnelMode::Quick,
            TunnelProvider::Cloudflare(CloudflareTunnel::Named { .. }) => TunnelMode::Named,
        }
    }
    /// What tells two settings apart, the token only by its digest: a run
    /// compares the one it serves with the one just saved to know whether
    /// to start again.
    pub(crate) fn identity(&self) -> String {
        let provider = match &self.provider {
            TunnelProvider::Cloudflare(CloudflareTunnel::Quick) => "cloudflare quick".to_owned(),
            TunnelProvider::Cloudflare(CloudflareTunnel::Named { address, token }) => {
                format!("cloudflare named {} {}", address.as_str(), token.digest())
            }
        };
        format!("{provider} ingress {}", self.ingress)
    }
}

/// The running tunnel as the product reads it: its live status.
#[derive(Clone)]
pub(crate) struct TunnelView {
    pub(crate) status: watch::Receiver<TunnelStatus>,
}

impl TunnelView {
    /// The address a reader can open now, when the tunnel is ready.
    pub(crate) fn address(&self) -> Option<TunnelAddress> {
        match &*self.status.borrow() {
            ServiceStatus::Ready(address) => Some(address.clone()),
            _ => None,
        }
    }
}
