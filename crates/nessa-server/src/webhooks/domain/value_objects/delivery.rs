use uuid::Uuid;

/// Five POSTs at most, with retry delays 1, 2, 4 and 8 seconds. No retry
/// extends a POST's own deadline.
pub const MAX_ATTEMPTS: u8 = 5;

/// A saved delivery state. The store appends each transition with its audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Sending,
    Retry { due_at_ms: u64 },
    Terminal(Outcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    NotSent,
    TimedOut,
    Unknown,
}

/// A transport observation, without response text, endpoint or key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Success,
    Retryable,
    Rejected,
    TimedOut,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryError {
    NotReady,
    WrongEpoch,
    InvalidTransition,
    TimeOverflow,
}

/// Immutable delivery value. A transition returns the replacement to save;
/// returning it is not a storage acknowledgement or a network effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    id: Uuid,
    epoch: u64,
    phase: Phase,
    attempts: u8,
    timestamp_ms: Option<u64>,
}

impl Delivery {
    pub fn queued(id: Uuid, epoch: u64) -> Self {
        Self {
            id,
            epoch,
            phase: Phase::Queued,
            attempts: 0,
            timestamp_ms: None,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn attempts(&self) -> u8 {
        self.attempts
    }
    pub fn timestamp_ms(&self) -> Option<u64> {
        self.timestamp_ms
    }

    /// Call while holding the current grant and registration permits. This
    /// checks the pinned revision, not permission: Cedar owns permission.
    /// Save the returned Sending transition before any POST byte.
    pub fn admit(&self, current_epoch: u64, now_ms: u64) -> Result<Self, DeliveryError> {
        if current_epoch != self.epoch {
            return Err(DeliveryError::WrongEpoch);
        }
        match self.phase {
            Phase::Queued => {}
            Phase::Retry { due_at_ms } if due_at_ms <= now_ms => {}
            _ => return Err(DeliveryError::NotReady),
        }
        if self.attempts >= MAX_ATTEMPTS {
            return Err(DeliveryError::InvalidTransition);
        }
        Ok(Self {
            phase: Phase::Sending,
            attempts: self.attempts + 1,
            timestamp_ms: Some(self.timestamp_ms.unwrap_or(now_ms)),
            ..self.clone()
        })
    }

    /// Resolve a live attempt. Retryable errors can have produced remote
    /// effects; recipients deduplicate by delivery id. This is not exactly once.
    pub fn observed(&self, observation: Observation, now_ms: u64) -> Result<Self, DeliveryError> {
        if self.phase != Phase::Sending {
            return Err(DeliveryError::InvalidTransition);
        }
        let phase = match observation {
            Observation::Success => Phase::Terminal(Outcome::Sent),
            Observation::Rejected => Phase::Terminal(Outcome::NotSent),
            Observation::TimedOut => Phase::Terminal(Outcome::TimedOut),
            Observation::Unknown => Phase::Terminal(Outcome::Unknown),
            Observation::Retryable if self.attempts < MAX_ATTEMPTS => {
                let delay = 1000u64 << (self.attempts - 1);
                Phase::Retry {
                    due_at_ms: now_ms
                        .checked_add(delay)
                        .ok_or(DeliveryError::TimeOverflow)?,
                }
            }
            Observation::Retryable => Phase::Terminal(Outcome::NotSent),
        };
        Ok(Self {
            phase,
            ..self.clone()
        })
    }

    /// Restart has no observation for Sending. It records Unknown and does
    /// not infer either remote success or a safe retry from a lost response.
    pub fn recovered(&self) -> Self {
        match self.phase {
            Phase::Sending => Self {
                phase: Phase::Terminal(Outcome::Unknown),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    /// Refuse work before dispatch (grant removed, epoch changed or credential
    /// stopped). An already Sending attempt resolves from its own observation.
    pub fn refused(&self) -> Result<Self, DeliveryError> {
        match self.phase {
            Phase::Queued | Phase::Retry { .. } => Ok(Self {
                phase: Phase::Terminal(Outcome::NotSent),
                ..self.clone()
            }),
            _ => Err(DeliveryError::InvalidTransition),
        }
    }
}
