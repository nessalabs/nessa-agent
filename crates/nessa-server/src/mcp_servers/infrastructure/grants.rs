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
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

/// The grants live now, by the digest of each one's token.
#[derive(Clone)]
pub struct ConversationGrants {
    inner: Arc<Grants>,
}

struct Grants {
    servers: McpServers,
    live: Mutex<HashMap<TokenDigest, McpOwner>>,
    next: AtomicU64,
}

impl ConversationGrants {
    /// Grants whose revocation ends their sessions on `servers`.
    pub fn new(servers: McpServers) -> Self {
        Self {
            inner: Arc::new(Grants {
                servers,
                live: Mutex::default(),
                next: AtomicU64::new(0),
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
        if let Err(error) = getrandom::fill(&mut bytes) {
            // Without a token, this open's stand-ins are refused: its MCP
            // servers fail to start, and nothing else does.
            tracing::error!(%error, "no MCP session token could be made; this session's MCP servers are off");
            return StandInGrant::new(Vec::new(), Box::new(()));
        }
        let token = session_token(bytes);
        let digest = TokenDigest::of(&token);
        let grant = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .live
            .lock()
            .expect("grants")
            .insert(digest, McpOwner::new(session.clone(), grant));
        StandInGrant::new(
            vec![(SESSION_VARIABLE.to_owned(), token)],
            Box::new(Revoke {
                grants: self.inner.clone(),
                digest,
                grant,
            }),
        )
    }
}

/// Revokes one grant when dropped: its token is no longer let through, and
/// the sessions opened with it end at once.
struct Revoke {
    grants: Arc<Grants>,
    digest: TokenDigest,
    grant: u64,
}
impl Drop for Revoke {
    fn drop(&mut self) {
        self.grants
            .live
            .lock()
            .expect("grants")
            .remove(&self.digest);
        self.grants.servers.close_granted(self.grant);
    }
}
