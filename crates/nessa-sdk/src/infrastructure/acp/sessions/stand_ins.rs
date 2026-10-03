//! What every stand-in for an MCP server is given for one provider open: the
//! host's grant for that SDK session — the environment its MCP server
//! processes get, and the results its stand-ins forward, which the ACP worker
//! attaches to the tool calls the harness reports.
#![deny(missing_docs)]

use super::ForwardedResults;
use crate::domain::agent_execution::sessions::SessionId;
use std::{fmt, sync::Arc};

/// The host's side of keying MCP stand-ins by session. For each provider
/// open of an SDK session, the binding asks for a grant and holds it for
/// that provider session's life — through every restart of its process —
/// then drops it. What the grant's environment says is the host's own; the
/// SDK puts it in every `mcpServers` entry of that open, which the harness
/// gives the MCP server process it starts.
pub trait StandInGrants: Send + Sync {
    /// A grant for one open of `session`. Called on the open's task, so it
    /// must not block; it may be called from several opens at once.
    fn grant(&self, session: &SessionId) -> StandInGrant;
}

/// One open's grant: the environment its MCP server processes get, the
/// results its stand-ins forward when it is an MCP owner's
/// ([`McpOwner::stand_in_grant`](crate::infrastructure::mcp::McpOwner::stand_in_grant)),
/// and what the host revokes when the grant is
/// dropped — which happens on whichever task drops the provider session's
/// last handle, so `held`'s `Drop` must not block either.
pub struct StandInGrant {
    environment: Arc<[(String, String)]>,
    forwarded: Option<ForwardedResults>,
    _held: Box<dyn Send + Sync>,
}
impl StandInGrant {
    /// A grant giving each MCP server process `environment`, holding `held`
    /// until it is dropped.
    pub fn new(environment: Vec<(String, String)>, held: Box<dyn Send + Sync>) -> Self {
        Self {
            environment: environment.into(),
            forwarded: None,
            _held: held,
        }
    }
    /// This grant, with the results its stand-ins forward to their harness:
    /// those of the owner it is granted as, which only that owner gives
    /// ([`McpOwner::stand_in_grant`](crate::infrastructure::mcp::McpOwner::stand_in_grant)).
    /// The binding attaches each to the tool call the harness reports it
    /// under, on that call's completed update. Without them, nothing is
    /// attached.
    pub(crate) fn with_forwarded(self, forwarded: ForwardedResults) -> Self {
        Self {
            forwarded: Some(forwarded),
            ..self
        }
    }
    /// The results this grant's stand-ins forward, when it was built with
    /// them: for a host to check that a grant carries its owner's
    /// ([`McpOwner::forwarded`](crate::infrastructure::mcp::McpOwner::forwarded)).
    pub fn forwarded(&self) -> Option<&ForwardedResults> {
        self.forwarded.as_ref()
    }
    /// What each MCP server process of the open is given.
    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }
}
impl fmt::Debug for StandInGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Its values are the host's secrets: never printed.
        f.debug_struct("StandInGrant")
            .field("variables", &self.environment.len())
            .field("forwarded", &self.forwarded)
            .finish_non_exhaustive()
    }
}

/// Where a binding's MCP stand-ins get their per-open environment, and where
/// the results they forward are taken from: a host's [`StandInGrants`], or
/// nowhere. Outside the context fingerprint, like credentials, so a fresh
/// grant on each open never changes it.
#[derive(Clone, Default)]
pub struct StandInSessions {
    grants: Option<Arc<dyn StandInGrants>>,
    environment: Arc<[(String, String)]>,
    forwarded: Option<ForwardedResults>,
}
impl StandInSessions {
    /// No grants: MCP server processes get no per-open environment.
    pub fn none() -> Self {
        Self::default()
    }
    /// Grants from `grants`, one for each provider open of an SDK session.
    pub fn granted_by(grants: Arc<dyn StandInGrants>) -> Self {
        Self {
            grants: Some(grants),
            environment: Arc::new([]),
            forwarded: None,
        }
    }
    /// For one open of `session` (none for an open that names no session):
    /// these sessions with that open's environment and forwarded results, and
    /// the grant to hold while the provider session lives. The ACP binding asks this itself; a
    /// host calls it only to check what an open's entries carry.
    ///
    /// It is not inert: it asks the host's [`StandInGrants`] for a real grant
    /// — the gateway's mints a token it lets through — which lasts until the
    /// returned [`StandInGrant`] is dropped. After that, the returned
    /// sessions still carry its environment, but the host no longer honours
    /// it.
    pub fn opened(&self, session: Option<&SessionId>) -> (Self, Option<StandInGrant>) {
        let grant = self
            .grants
            .as_ref()
            .zip(session)
            .map(|(grants, session)| grants.grant(session));
        let environment = grant
            .as_ref()
            .map_or_else(|| Arc::from([]), |grant| grant.environment.clone());
        let forwarded = grant.as_ref().and_then(|grant| grant.forwarded.clone());
        (
            Self {
                grants: self.grants.clone(),
                environment,
                forwarded,
            },
            grant,
        )
    }
    /// What every MCP server process of this open is given.
    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }
    /// The results this open's stand-ins forwarded, when its grant has them.
    pub(crate) fn forwarded(&self) -> Option<&ForwardedResults> {
        self.forwarded.as_ref()
    }
}
impl fmt::Debug for StandInSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StandInSessions")
            .field("granted", &self.grants.is_some())
            .field("variables", &self.environment.len())
            .field("forwarded", &self.forwarded.is_some())
            .finish()
    }
}
