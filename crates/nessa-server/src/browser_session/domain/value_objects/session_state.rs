use super::{BrowserSessionOrigin, Lifetime, RemovalReason};
use nessa_auth::domain::CredentialId;

const RENEWAL_INTERVAL_SECONDS: u64 = 60 * 60;

/// Immutable authoritative state stored under one opaque browser-session ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserSessionState {
    credential_id: CredentialId,
    lifetime: Lifetime,
    origin: BrowserSessionOrigin,
}

impl BrowserSessionState {
    pub fn new<O>(credential_id: CredentialId, origin: O, now: u64) -> Option<Self>
    where
        O: TryInto<BrowserSessionOrigin>,
    {
        Some(Self {
            lifetime: Lifetime::new(now)?,
            credential_id,
            origin: origin.try_into().ok()?,
        })
    }

    pub fn restore<O>(
        credential_id: CredentialId,
        origin: O,
        created_at: u64,
        renewed_at: u64,
        idle_expires_at: u64,
    ) -> Option<Self>
    where
        O: TryInto<BrowserSessionOrigin>,
    {
        Some(Self {
            lifetime: Lifetime::restore(created_at, renewed_at, idle_expires_at)?,
            credential_id,
            origin: origin.try_into().ok()?,
        })
    }

    pub fn renewed_for_check(&self, now: u64) -> Option<Self> {
        if !self.is_active_at(now) {
            return None;
        }
        if now.saturating_sub(self.renewed_at()) < RENEWAL_INTERVAL_SECONDS {
            return Some(self.clone());
        }
        let lifetime = self.lifetime.renew(now)?;
        Some(Self {
            credential_id: self.credential_id.clone(),
            origin: self.origin.clone(),
            lifetime,
        })
    }

    pub fn is_active_at(&self, now: u64) -> bool {
        self.lifetime.is_active_at(now)
    }

    /// Whether this session's renewal instant is one a clock at `now` can vouch for.
    pub fn is_plausible_at(&self, now: u64) -> bool {
        self.lifetime.is_plausible_at(now)
    }

    pub fn expiration_reason(&self, now: u64) -> Option<RemovalReason> {
        if self.is_active_at(now) {
            None
        } else {
            Some(RemovalReason::IdleExpired)
        }
    }

    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }

    pub fn created_at(&self) -> u64 {
        self.lifetime.created_at()
    }

    pub fn renewed_at(&self) -> u64 {
        self.lifetime.renewed_at()
    }

    pub fn idle_expires_at(&self) -> u64 {
        self.lifetime.idle_expires_at()
    }
}
