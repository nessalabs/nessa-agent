//! Reaching the gateway from anywhere through a web tunnel
//! (`docs/design/auth/tunnel.md`): a provider's program forwards WebSockets
//! to a loopback ingress, whose bytes are the native TLS the listener
//! already serves.
//!
//! ```text
//! application: the setting, the status and the view the product reads
//! infrastructure::ingress: the loopback way in, and the listener's tunnel source
//! infrastructure::cloudflare: the Cloudflare provider (cloudflared), a helper service
//! ```
//!
//! Only `infrastructure::cloudflare` knows which provider carries the
//! tunnel; another provider is another helper service beside it.
// Only a Unix gateway runs a tunnel today (native linking is Unix only).
#[cfg_attr(not(unix), allow(dead_code))]
pub mod application;
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) mod infrastructure;
