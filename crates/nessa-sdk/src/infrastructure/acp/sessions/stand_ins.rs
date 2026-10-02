//! What every stand-in for an MCP server is given for one provider open: the
//! host's grant for that SDK session.
#![deny(missing_docs)]

use crate::domain::agent_execution::sessions::SessionId;
use std::{fmt, sync::Arc};

/// The host's side of keying MCP stand-ins by session. For each provider
/// open of an SDK session, the binding asks for a grant and holds it for
/// that provider session's life — through every restart of its process —
/// then drops it. What the grant's environment says is the host's own; the
/// SDK passes it to every MCP server process of that open and nowhere else.
pub trait StandInGrants: Send + Sync {
    /// A grant for one open of `session`.
    fn grant(&self, session: &SessionId) -> StandInGrant;
}

/// One open's grant: the environment its MCP server processes get, and what
/// the host revokes when the grant is dropped.
pub struct StandInGrant {
    environment: Arc<[(String, String)]>,
    _held: Box<dyn Send + Sync>,
}
impl StandInGrant {
    /// A grant giving each MCP server process `environment`, holding `held`
    /// until it is dropped.
    pub fn new(environment: Vec<(String, String)>, held: Box<dyn Send + Sync>) -> Self {
        Self {
            environment: environment.into(),
            _held: held,
        }
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
            .finish_non_exhaustive()
    }
}

/// Where a binding's MCP stand-ins get their per-open environment: from a
/// host's [`StandInGrants`], or nowhere. Outside the context fingerprint,
/// like credentials, so a fresh grant on each open never changes it.
#[derive(Clone, Default)]
pub struct StandInSessions {
    grants: Option<Arc<dyn StandInGrants>>,
    environment: Arc<[(String, String)]>,
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
        }
    }
    /// For one open of `session` (none for an open that names no session):
    /// these sessions with that open's environment, and the grant to hold
    /// while the provider session lives.
    pub(crate) fn opened(&self, session: Option<&SessionId>) -> (Self, Option<StandInGrant>) {
        let grant = self
            .grants
            .as_ref()
            .zip(session)
            .map(|(grants, session)| grants.grant(session));
        let environment = grant
            .as_ref()
            .map_or_else(|| Arc::from([]), |grant| grant.environment.clone());
        (
            Self {
                grants: self.grants.clone(),
                environment,
            },
            grant,
        )
    }
    /// What every MCP server process of this open is given.
    pub(crate) fn environment(&self) -> &[(String, String)] {
        &self.environment
    }
}
impl fmt::Debug for StandInSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StandInSessions")
            .field("granted", &self.grants.is_some())
            .field("variables", &self.environment.len())
            .finish()
    }
}
