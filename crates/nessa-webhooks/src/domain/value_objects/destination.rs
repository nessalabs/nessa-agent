use std::fmt;
use url::{Host, Url};

/// Upper bound before parsing or storing a registration URL.
pub const MAX_DESTINATION_BYTES: usize = 2048;

/// A syntactically valid webhook destination. DNS addresses must additionally
/// pass the network adapter's address policy at each POST.
#[derive(Clone, PartialEq, Eq)]
pub struct Destination(Url);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidDestination;

impl Destination {
    /// HTTPS on its default port, with a DNS hostname and no credentials,
    /// query or fragment. The URL stays out of diagnostic representations.
    pub fn parse(value: &str) -> Result<Self, InvalidDestination> {
        if value.len() > MAX_DESTINATION_BYTES || value.bytes().any(|b| b.is_ascii_control()) {
            return Err(InvalidDestination);
        }
        let url = Url::parse(value).map_err(|_| InvalidDestination)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port_or_known_default() != Some(443)
            || !matches!(url.host(), Some(Host::Domain(_)))
        {
            return Err(InvalidDestination);
        }
        Ok(Self(url))
    }

    /// Only for the transport. This can contain an endpoint path secret.
    pub fn expose_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Destination([REDACTED])")
    }
}
