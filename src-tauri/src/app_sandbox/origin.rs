//! The sandbox proxy origin, shared by its handler and navigation policy.

/// The scheme the proxy is served on.
pub const SCHEME: &str = "nessa-sandbox";

/// The host the same scheme is served as on Windows, where WebView2 serves a
/// custom scheme as an `http` host named after it.
pub const HTTP_HOST: &str = "nessa-sandbox.localhost";
