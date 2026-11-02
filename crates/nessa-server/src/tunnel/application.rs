//! What the gateway knows about its web tunnel, whichever provider carries
//! it (`docs/design/auth/tunnel.md`): the saved setting, and the status its
//! service publishes. A provider is a shepherd service that reports the
//! tunnel's public address once it is ready; the bridge, the ingress, the
//! listener's edge and every reader know nothing of which provider it is.
//!
//! ```text
//! config.json native.tunnel + the token file ──▶ TunnelSetting ──▶ composition picks the provider's service
//! shepherd ServiceSupervisor (failure kind) + LaunchRefusal ──▶ composition::tunnel ──▶ watch<TunnelStatus>
//!   ──▶ pairing.tunnel, server.reach
//! ```
use nessa_protocol::tunnel::TunnelAddress;
use sha2::{Digest, Sha256};
use std::{fmt, net::SocketAddr};
use tokio::sync::watch;

/// Where the tunnel stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TunnelStatus {
    /// Not ready yet (or starting again).
    Starting,
    /// Ready, at this public address.
    Ready(TunnelAddress),
    /// The last attempt failed for this reason. It is tried again.
    Failed(TunnelFailure),
}

/// Why the tunnel is not up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TunnelFailure {
    /// Its program could not be made ready, or nothing can run: the wire's
    /// word for why (`unsupported`, `download`, `unverified`, `storage`,
    /// `token`, `ingress`).
    Unavailable(&'static str),
    /// The program could not be started, or it ended.
    Start,
    /// It started but was not ready in time, or said it never would be.
    NotReady,
}

impl TunnelFailure {
    /// The wire's word for it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable(reason) => reason,
            Self::Start => "start",
            Self::NotReady => "not_ready",
        }
    }
}

/// Why the provider's last launch was refused, when it was: the provider
/// decides it, and the status reads it from here, never from a failure's
/// wording. Emptied by a launch that goes ahead.
pub(crate) type LaunchRefusal = std::sync::Arc<std::sync::Mutex<Option<TunnelFailure>>>;

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

    /// Which token this is, for audit evidence: the first 8 bytes of its
    /// SHA-256, in hex. Telling two tokens apart needs no more, and it
    /// cannot be used in the token's place.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.0.as_bytes())[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
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

/// Why a device tunnel secret was revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretRevocation {
    /// The device was unpaired (`pairing.cancel` of an active enrollment).
    Unpaired,
    /// Its credential was revoked (`credential.revoke`).
    CredentialRevoked,
}

/// A tunnel secret could not be issued or revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// The audit record was not stored. The secret was not handed out, or
    /// on revoke the credential is already ended and a retry records the audit.
    Audit,
    /// The secret file could not be read or written.
    Storage,
}

/// The paired devices' tunnel secrets. Issuing and revoking are audited.
/// `accepts` and `authorize_lookup` consider only secrets whose credentials
/// are still active.
pub trait DeviceTunnelSecrets: Send + Sync {
    /// The secret for `credential_id`, creating it on the first call.
    /// A later call returns the same secret and writes no second audit.
    fn issue(
        &self,
        credential_id: &str,
        device_key: &[u8; 32],
        initiator: &str,
    ) -> Result<[u8; 32], SecretError>;

    /// Forget `credential_id`'s secret and record the revocation.
    /// A credential that was never issued is already done.
    fn revoke(
        &self,
        credential_id: &str,
        cause: SecretRevocation,
        initiator: &str,
    ) -> Result<(), SecretError>;

    /// Whether `presented` is an active paired device's secret.
    /// The compare does not stop at the first match.
    fn accepts(&self, presented: &[u8]) -> bool;

    /// Whether `mac` is `tunnel_lookup_mac` for an active secret, `gateway` and `nonce`.
    fn authorize_lookup(
        &self,
        gateway: &nessa_protocol::relay::GatewayKey,
        nonce: &[u8; 32],
        mac: &[u8; 32],
    ) -> bool;
}

/// Secrets kept in memory. Pairing tests use it; the gateway uses the file store.
#[derive(Default)]
pub struct MemoryDeviceSecrets {
    rows: std::sync::Mutex<Vec<MemorySecret>>,
}

struct MemorySecret {
    credential_id: String,
    secret: [u8; 32],
}

impl MemoryDeviceSecrets {
    /// Nothing issued.
    pub fn new() -> Self {
        Self::default()
    }
}

impl DeviceTunnelSecrets for MemoryDeviceSecrets {
    fn issue(
        &self,
        credential_id: &str,
        _device_key: &[u8; 32],
        _initiator: &str,
    ) -> Result<[u8; 32], SecretError> {
        let mut rows = self.rows.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(row) = rows.iter().find(|row| row.credential_id == credential_id) {
            return Ok(row.secret);
        }
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(|_| SecretError::Storage)?;
        rows.push(MemorySecret {
            credential_id: credential_id.to_owned(),
            secret,
        });
        Ok(secret)
    }

    fn revoke(
        &self,
        credential_id: &str,
        _cause: SecretRevocation,
        _initiator: &str,
    ) -> Result<(), SecretError> {
        let mut rows = self.rows.lock().unwrap_or_else(|error| error.into_inner());
        rows.retain(|row| row.credential_id != credential_id);
        Ok(())
    }

    fn accepts(&self, presented: &[u8]) -> bool {
        let rows = self.rows.lock().unwrap_or_else(|error| error.into_inner());
        any_match(
            presented,
            &rows.iter().map(|row| row.secret).collect::<Vec<_>>(),
        )
    }

    fn authorize_lookup(
        &self,
        gateway: &nessa_protocol::relay::GatewayKey,
        nonce: &[u8; 32],
        mac: &[u8; 32],
    ) -> bool {
        let rows = self.rows.lock().unwrap_or_else(|error| error.into_inner());
        any_mac(
            &rows.iter().map(|row| row.secret).collect::<Vec<_>>(),
            gateway,
            nonce,
            mac,
        )
    }
}

/// True when `presented` equals some secret. Every secret is compared, and
/// a wrong length matches none. An empty list still runs one compare.
pub(crate) fn any_match(presented: &[u8], secrets: &[[u8; 32]]) -> bool {
    use subtle::{Choice, ConstantTimeEq};
    let mut candidate = [0; 32];
    let length_ok = Choice::from(u8::from(presented.len() == 32));
    if presented.len() == 32 {
        candidate.copy_from_slice(presented);
    }
    let mut matched = Choice::from(0);
    if secrets.is_empty() {
        let _ = candidate.ct_eq(&[0; 32]);
        return false;
    }
    for secret in secrets {
        matched |= candidate.ct_eq(secret);
    }
    bool::from(matched & length_ok)
}

/// True when `mac` is the lookup mac of some secret. Every secret is checked.
pub(crate) fn any_mac(
    secrets: &[[u8; 32]],
    gateway: &nessa_protocol::relay::GatewayKey,
    nonce: &[u8; 32],
    mac: &[u8; 32],
) -> bool {
    use nessa_protocol::relay::tunnel_lookup_mac;
    use subtle::{Choice, ConstantTimeEq};
    let dummy = [0; 32];
    let list: &[[u8; 32]] = if secrets.is_empty() {
        std::slice::from_ref(&dummy)
    } else {
        secrets
    };
    let mut matched = Choice::from(0);
    for secret in list {
        matched |= mac.ct_eq(&tunnel_lookup_mac(secret, gateway, nonce));
    }
    bool::from(matched & Choice::from(u8::from(!secrets.is_empty())))
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
            TunnelStatus::Ready(address) => Some(address.clone()),
            _ => None,
        }
    }
}
