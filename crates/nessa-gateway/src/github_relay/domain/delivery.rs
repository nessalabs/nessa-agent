/// The immutable account/repository binding selected by the local owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    generation: uuid::Uuid,
    user_id: String,
    repository_id: String,
    repository_name: String,
}
impl Binding {
    pub fn new(
        user_id: String,
        repository_id: String,
        repository_name: String,
        generation: uuid::Uuid,
    ) -> Result<Self, Failure> {
        let identity = |value: &str| {
            value
                .parse::<i64>()
                .is_ok_and(|id| id > 0 && id <= 9_007_199_254_740_991 && id.to_string() == value)
        };
        if !identity(&user_id)
            || !identity(&repository_id)
            || !Self::repository_name_valid(&repository_name)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(Self {
            generation,
            user_id,
            repository_id,
            repository_name,
        })
    }
    pub fn repository_name_valid(value: &str) -> bool {
        let parts = value.split('/').collect::<Vec<_>>();
        parts.len() == 2
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.len() <= 100
                    && *part != "."
                    && *part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
    }
    pub fn generation(&self) -> uuid::Uuid {
        self.generation
    }
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }
    pub fn repository_name(&self) -> &str {
        &self.repository_name
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub sequence: i64,
    pub delivery_id: String,
    pub event: String,
    pub payload: String,
    pub received_at: String,
    pub repository_id: String,
}
pub struct Page {
    pub user_id: String,
    pub events: Vec<Event>,
    pub next: i64,
    pub has_more: bool,
    pub ticket: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    UnconfiguredStage,
    CredentialsUnavailable,
    IdentityChanged,
    StageMismatch,
    CatchUpPending,
    Forbidden,
    Offline,
    RateLimited,
    InvalidResponse,
    /// Protected storage is sealed; wait for the app to unlock it.
    VaultLocked,
    Storage,
    Capacity,
    CursorChanged,
    Gap(i64),
}
impl std::fmt::Display for Failure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gap(through) => write!(out, "GitHub catch-up gap through {through}; reconnect with --accept-gap to acknowledge it"),
            other => write!(out, "GitHub relay: {other:?}"),
        }
    }
}
impl std::error::Error for Failure {}
impl Page {
    /// Response correlation and monotonicity precede durable effects.
    pub fn validate(&self, binding: &Binding, after: i64) -> Result<(), Failure> {
        if self.user_id != binding.user_id() {
            return Err(Failure::IdentityChanged);
        }
        if after < 0
            || self.events.len() > super::contract::PAGE_SIZE
            || self.ticket.is_empty()
            || self.ticket.len() > 1024
        {
            return Err(Failure::InvalidResponse);
        }
        if self.has_more && self.events.is_empty() {
            return Err(Failure::InvalidResponse);
        }
        if self
            .events
            .iter()
            .map(|event| event.payload.len())
            .sum::<usize>()
            > super::contract::PAYLOAD_BYTES
        {
            return Err(Failure::InvalidResponse);
        }
        let mut previous = after;
        for event in &self.events {
            if event.repository_id != binding.repository_id()
                || event.sequence <= previous
                || event.sequence > 9_007_199_254_740_991
                || event.delivery_id.len() != 36
                || uuid::Uuid::parse_str(&event.delivery_id).is_err()
                || event.payload.len() > super::contract::PAYLOAD_BYTES
                || event.event.is_empty()
                || event.event.len() > 64
            {
                return Err(Failure::InvalidResponse);
            }
            previous = event.sequence;
        }
        if self.next != previous {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }
}
