pub const IDLE_SECONDS: u64 = 30 * 24 * 60 * 60;
/// How far ahead of the clock a stored renewal may claim to have happened.
///
/// Journalled instants are only as truthful as whoever can write the journal,
/// and a wall clock can legitimately disagree with itself across a restart.
/// One hour matches the renewal interval: the finest granularity at which a
/// session's own time already matters.
pub const FUTURE_TOLERANCE_SECONDS: u64 = 60 * 60;

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
    /// Whether a clock reading of `now` can vouch for this renewal instant.
    ///
    /// Every extension of a session's life moves `renewed_at` forward, and
    /// `idle_expires_at` is always exactly `renewed_at + IDLE_SECONDS`, so a
    /// renewal the clock cannot account for is the one claim every forged or
    /// clock-damaged lifetime has to make.
    pub fn is_plausible_at(&self, now: u64) -> bool {
        self.renewed_at <= now.saturating_add(FUTURE_TOLERANCE_SECONDS)
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
