//! A remote MCP server's identity and URL. The URL rules live here: HTTPS, or
//! HTTP only for an explicit loopback host, with no userinfo and no fragment
//! (`docs/adr/todo/392-remote-mcp-servers.md`).
#![deny(missing_docs)]

use super::McpError;
use crate::infrastructure::acp::sessions::{
    server_name_unacceptable, McpServerProblem, MAX_MCP_SERVER_NAME_BYTES,
};
use std::fmt;
use url::Url;
use uuid::Uuid;

/// Why a remote URL cannot be an MCP endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteUrlProblem {
    /// Not an absolute URL, or not `http`/`https`.
    Scheme,
    /// A username or password is present.
    Userinfo,
    /// A fragment is present.
    Fragment,
    /// HTTP was used for a host that is not loopback.
    NotLoopback,
}

impl fmt::Display for RemoteUrlProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scheme => f.write_str("the MCP URL must be https, or http on loopback"),
            Self::Userinfo => f.write_str("the MCP URL must not carry userinfo"),
            Self::Fragment => f.write_str("the MCP URL must not carry a fragment"),
            Self::NotLoopback => f.write_str("http is only allowed for a loopback MCP URL"),
        }
    }
}

/// An MCP endpoint URL this client will call. The string is the canonical
/// form the `url` crate produces, so two spellings of one endpoint compare
/// equal.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RemoteMcpUrl {
    canonical: String,
    origin: String,
}

impl fmt::Debug for RemoteMcpUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RemoteMcpUrl")
            .field(&self.canonical)
            .finish()
    }
}

impl RemoteMcpUrl {
    /// Parse `raw` into an endpoint this client will call.
    ///
    /// # Errors
    ///
    /// [`RemoteUrlProblem`] when the scheme, userinfo, fragment, or host is
    /// not allowed (`a_remote_url_is_https_or_loopback_http`).
    pub fn parse(raw: &str) -> Result<Self, RemoteUrlProblem> {
        let url = Url::parse(raw).map_err(|_| RemoteUrlProblem::Scheme)?;
        if url.fragment().is_some() {
            return Err(RemoteUrlProblem::Fragment);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(RemoteUrlProblem::Userinfo);
        }
        match url.scheme() {
            "https" => {}
            "http" if host_is_loopback(url.host_str()) => {}
            "http" => return Err(RemoteUrlProblem::NotLoopback),
            _ => return Err(RemoteUrlProblem::Scheme),
        }
        if url.host_str().is_none() {
            return Err(RemoteUrlProblem::Scheme);
        }
        let origin = url.origin().ascii_serialization();
        Ok(Self {
            canonical: url.to_string(),
            origin,
        })
    }

    /// The canonical URL.
    pub fn as_str(&self) -> &str {
        &self.canonical
    }

    /// `scheme://host[:port]`, for the same-origin check on a legacy endpoint.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Resolve `endpoint` against this URL and accept it only when the result
    /// has this origin. Relative paths and absolute URLs on this origin pass;
    /// any other origin is refused.
    pub fn same_origin(&self, endpoint: &str) -> Option<Self> {
        let base = Url::parse(&self.canonical).ok()?;
        let joined = base.join(endpoint).ok()?;
        let candidate = Self::parse(joined.as_str()).ok()?;
        (candidate.origin == self.origin).then_some(candidate)
    }
}

/// Loopback is the name `localhost`, an IPv4 address in `127.0.0.0/8`, or
/// IPv6 `::1`. The check is on the host the URL parsed, not a substring.
fn host_is_loopback(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host.eq_ignore_ascii_case("::1") {
        return true;
    }
    let mut parts = host.split('.');
    let Some(first) = parts.next().and_then(|part| part.parse::<u8>().ok()) else {
        return false;
    };
    let rest = parts.count();
    first == 127 && rest == 3 && host.split('.').all(|part| part.parse::<u8>().is_ok())
}

/// One configured remote MCP server: a durable id, the name a harness sees,
/// and the endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteMcpServer {
    id: Uuid,
    name: String,
    url: RemoteMcpUrl,
}

impl fmt::Debug for RemoteMcpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteMcpServer")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &self.url)
            .finish()
    }
}

impl RemoteMcpServer {
    /// `id`, shown as `name`, reached at `url`.
    ///
    /// # Errors
    ///
    /// [`McpError::InvalidConfiguration`] when `name` breaks the server-name
    /// rule ([`server_name_unacceptable`]) or `url` is not an endpoint
    /// ([`RemoteMcpUrl::parse`]).
    pub fn new(id: Uuid, name: impl Into<String>, url: &str) -> Result<Self, McpError> {
        let name = name.into();
        if server_name_unacceptable(&name) {
            return Err(McpError::InvalidConfiguration(McpServerProblem::Name {
                server: name,
            }));
        }
        let url = RemoteMcpUrl::parse(url).map_err(|_| {
            McpError::InvalidConfiguration(McpServerProblem::Url {
                server: name.clone(),
            })
        })?;
        Ok(Self { id, name, url })
    }

    /// The durable id. Rename does not change it.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The name a harness sees.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The endpoint.
    pub fn url(&self) -> &RemoteMcpUrl {
        &self.url
    }

    /// Why this server cannot be used, or `None`. The name rule is
    /// [`server_name_unacceptable`]; the URL was checked at construction, so
    /// a value this constructor returned has no problem.
    pub fn problem(&self) -> Option<McpServerProblem> {
        if self.name.is_empty() || self.name.len() > MAX_MCP_SERVER_NAME_BYTES {
            return Some(McpServerProblem::Name {
                server: self.name.clone(),
            });
        }
        server_name_unacceptable(&self.name).then(|| McpServerProblem::Name {
            server: self.name.clone(),
        })
    }
}
