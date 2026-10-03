//! The tokens the gateway issues to open conversations' harnesses: one for
//! each provider open of an SDK session (a conversation's), revoked when that
//! provider session ends. A stand-in's hello names one, and the session it
//! opens is that conversation's.
use crate::mcp_servers::domain::{session_token, TokenDigest, SESSION_VARIABLE};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::acp::sessions::{StandInGrant, StandInGrants};
use nessa_sdk::infrastructure::mcp::{McpOwner, McpServers};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Where tokens' random bytes come from.
pub trait TokenSource: Send + Sync {
    /// Fill `bytes` with fresh random bytes, or say why not.
    fn fill(&self, bytes: &mut [u8; 32]) -> Result<(), String>;
}

/// The operating system's random source.
pub struct OsTokens;
impl TokenSource for OsTokens {
    fn fill(&self, bytes: &mut [u8; 32]) -> Result<(), String> {
        getrandom::fill(bytes).map_err(|error| error.to_string())
    }
}

/// The grants live now, by the digest of each one's token.
#[derive(Clone)]
pub struct ConversationGrants {
    inner: Arc<Grants>,
}

struct Grants {
    servers: McpServers,
    tokens: Arc<dyn TokenSource>,
    live: Mutex<HashMap<TokenDigest, McpOwner>>,
}

impl ConversationGrants {
    /// Grants whose tokens come from `tokens`, and whose revocation closes
    /// their sessions on `servers`.
    pub fn new(servers: McpServers, tokens: Arc<dyn TokenSource>) -> Self {
        Self {
            inner: Arc::new(Grants {
                servers,
                tokens,
                live: Mutex::default(),
            }),
        }
    }

    /// Whose a stand-in naming `token` is: the open it was issued for, while
    /// that grant lives. `None` for a token never issued, or revoked.
    pub fn owner(&self, token: &str) -> Option<McpOwner> {
        let live = self.inner.live.lock().expect("grants");
        live.get(&TokenDigest::of(token)).cloned()
    }

    /// How many grants are live now.
    #[cfg(all(test, unix))]
    pub(crate) fn live(&self) -> usize {
        self.inner.live.lock().expect("grants").len()
    }
}

impl StandInGrants for ConversationGrants {
    fn grant(&self, session: &SessionId) -> StandInGrant {
        let mut bytes = [0; 32];
        if let Err(error) = self.inner.tokens.fill(&mut bytes) {
            // Without a token, this open's stand-ins are refused: its MCP
            // servers fail to start, and nothing else does.
            tracing::error!(%error, "no MCP session token could be made; this session's MCP servers are off");
            return StandInGrant::new(Vec::new(), Box::new(()));
        }
        let token = session_token(bytes);
        let digest = TokenDigest::of(&token);
        let owner = McpOwner::new(session.clone());
        self.inner
            .live
            .lock()
            .expect("grants")
            .insert(digest, owner.clone());
        // What this open's stand-ins forward, for the binding to attach to
        // the tool calls its harness reports (#435).
        let forwarded = owner.forwarded();
        StandInGrant::new(
            vec![(SESSION_VARIABLE.to_owned(), token)],
            Box::new(Revoke {
                grants: self.inner.clone(),
                digest,
                owner,
            }),
        )
        .with_forwarded(forwarded)
    }
}

/// Revokes one grant when dropped: its token is no longer let through, no
/// session opens under it, and the sessions opened with it are closed.
struct Revoke {
    grants: Arc<Grants>,
    digest: TokenDigest,
    owner: McpOwner,
}
impl Drop for Revoke {
    fn drop(&mut self) {
        self.grants
            .live
            .lock()
            .expect("grants")
            .remove(&self.digest);
        self.grants.servers.revoke(&self.owner);
    }
}
