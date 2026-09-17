pub const IDLE_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Time evidence for an opaque browser session's rolling idle deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifetime {
    created_at: u64,
    renewed_at: u64,
    idle_expires_at: u64,
}
impl Lifetime {
    pub fn new(now: u64) -> Option<Self> {
        Self::restore(now, now, now.checked_add(IDLE_SECONDS)?)
    }
    pub fn restore(created_at: u64, renewed_at: u64, idle_expires_at: u64) -> Option<Self> {
        let expected = renewed_at.checked_add(IDLE_SECONDS)?;
        if created_at > renewed_at || idle_expires_at <= renewed_at || idle_expires_at != expected {
            return None;
        }
        Some(Self {
            created_at,
            renewed_at,
            idle_expires_at,
        })
    }
    pub fn renew(&self, now: u64) -> Option<Self> {
        if now < self.renewed_at || now >= self.idle_expires_at {
            return None;
        }
        Self::restore(self.created_at, now, now.checked_add(IDLE_SECONDS)?)
    }
    pub fn is_active_at(&self, now: u64) -> bool {
        now < self.idle_expires_at
    }
    pub fn created_at(&self) -> u64 {
        self.created_at
    }
    pub fn renewed_at(&self) -> u64 {
        self.renewed_at
    }
    pub fn idle_expires_at(&self) -> u64 {
        self.idle_expires_at
    }
}

#[cfg(test)]
#[path = "../../../../tests/browser_session/lifetime.rs"]
mod tests;
