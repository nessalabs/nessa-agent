//! Who may copy next on a weak or metered link, and at what quote.
//!
//! The table is `docs/design/weak-link-scheduling.md`.
//! This module does not read a clock, a socket, or the cache. The caller
//! applies a returned wait and then says `pace_elapsed` or `backoff_elapsed`.
//! Payload bytes are the only unit: the quote, the reserve, `kept`, and the
//! pace wait all count them.
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use std::num::NonZeroU64;

/// How the bytes travel. It does not change who is admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Direct,
    Relay,
}

/// Whether a person is looking at the open conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presence {
    Foreground,
    Background,
    Sleep,
}

/// Whether the open conversation's subscription is still up.
/// Down is set after `follow` has ended. It does not keep that loop running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Subscription {
    Up,
    Down,
}

/// Whether copied bytes spend an allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Billing {
    Unmetered,
    /// Payload bytes still allowed. Reserved at admit, reconciled at settle.
    Metered {
        remaining: u64,
    },
}

/// One thing the caller might copy. Identity is the conversation, or the
/// conversation and the file's digest for one artifact range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Interest {
    OpenTranscript(ConversationId),
    Catalogue,
    OlderHistory(ConversationId),
    Artifact {
        conversation: ConversationId,
        digest: Sha256Digest,
    },
}

/// A ceiling known before the read, or the caller has not classified the interest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quote {
    Known(u64),
    Unknown,
}

/// One candidate in the single list the caller submits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) interest: Interest,
    pub(crate) quote: Quote,
}

/// Why nothing was copied. Order when several facts hold: unreachable, then
/// reconnect, then sleep, then backpressure, then the quote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Deferral {
    Unreachable,
    Reconnect,
    Sleep,
    Backpressure,
    UnknownQuote,
    Allowance,
}

/// What `admit` decided. `InFlight` means the quantum already admitted is
/// still the one running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Copy this interest. `reserved` is the payload allowance held until settle.
    Go {
        interest: Interest,
        reserved: u64,
    },
    Defer(Deferral),
    /// No candidate was admissible. Nothing is copied.
    None,
    InFlight,
}

/// What `finish` or `cancel` did. Both reconcile the allowance the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settle {
    /// No quantum was in flight, so nothing was charged.
    Idle,
    /// `wait_ms` is `ceil(kept / rate)` seconds when the pace is limited and
    /// `kept` is not zero, and zero otherwise. The pace stays closed until
    /// `pace_elapsed` when `wait_ms` is not zero.
    Settled {
        kept: u64,
        wait_ms: u64,
        reconnecting: bool,
    },
}

/// How a link starts. Events after `new` are methods on [`LinkSchedule`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkSpec {
    pub(crate) route: Route,
    pub(crate) presence: Presence,
    pub(crate) billing: Billing,
    pub(crate) pace_bytes_per_second: Option<NonZeroU64>,
    pub(crate) open: Option<ConversationId>,
    pub(crate) subscription: Subscription,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Flight {
    reserved: u64,
    /// Allowance before this quantum reserved any of it. Unmetered is zero.
    before: u64,
}

/// The schedule. Fields stay private; each event returns a new value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkSchedule {
    route: Route,
    presence: Presence,
    source_reachable: bool,
    reconnecting: bool,
    /// A loss reported while a quantum is in flight. Applied at settle.
    loss_pending: bool,
    pace_bytes_per_second: Option<NonZeroU64>,
    pace_closed: bool,
    billing: Billing,
    open: Option<ConversationId>,
    subscription: Subscription,
    flight: Option<Flight>,
}

impl LinkSchedule {
    pub(crate) fn new(spec: LinkSpec) -> Self {
        Self {
            route: spec.route,
            presence: spec.presence,
            source_reachable: true,
            reconnecting: false,
            loss_pending: false,
            pace_bytes_per_second: spec.pace_bytes_per_second,
            pace_closed: false,
            billing: spec.billing,
            open: spec.open,
            subscription: spec.subscription,
            flight: None,
        }
    }

    pub(crate) fn route(&self) -> Route {
        self.route
    }

    pub(crate) fn remaining(&self) -> Option<u64> {
        match self.billing {
            Billing::Metered { remaining } => Some(remaining),
            Billing::Unmetered => None,
        }
    }

    pub(crate) fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    /// Set the metered remainder. Refused while a quantum is in flight, or
    /// when the link is unmetered: the caller still holds the schedule.
    pub(crate) fn with_allowance(mut self, remaining: u64) -> Result<Self, Self> {
        match (&mut self.billing, self.flight.is_some()) {
            (Billing::Metered { remaining: slot }, false) => {
                *slot = remaining;
                Ok(self)
            }
            _ => Err(self),
        }
    }

    pub(crate) fn subscription(mut self, subscription: Subscription) -> Self {
        self.subscription = subscription;
        self
    }

    pub(crate) fn relay(mut self) -> Self {
        self.route = Route::Relay;
        self
    }

    pub(crate) fn sleep(mut self) -> Self {
        self.presence = Presence::Sleep;
        self
    }

    /// Foreground or background. Sleep is [`Self::sleep`].
    pub(crate) fn wake(mut self, presence: Presence) -> Self {
        if presence != Presence::Sleep {
            self.presence = presence;
        }
        self
    }

    pub(crate) fn unreachable(mut self) -> Self {
        self.source_reachable = false;
        self
    }

    pub(crate) fn reachable(mut self) -> Self {
        self.source_reachable = true;
        self
    }

    pub(crate) fn connection_lost(mut self) -> Self {
        if self.flight.is_some() {
            self.loss_pending = true;
        } else {
            self.reconnecting = true;
        }
        self
    }

    pub(crate) fn backoff_elapsed(mut self) -> Self {
        self.reconnecting = false;
        self
    }

    pub(crate) fn pace_elapsed(mut self) -> Self {
        self.pace_closed = false;
        self
    }

    pub(crate) fn admit(mut self, candidates: &[Candidate]) -> (Self, Admit) {
        if self.flight.is_some() {
            return (self, Admit::InFlight);
        }
        if let Some(deferral) = self.blocked() {
            return (self, Admit::Defer(deferral));
        }
        let Some(winner) = winner(&self, candidates) else {
            return (self, Admit::None);
        };
        match self.billing {
            Billing::Metered { remaining } => match winner.quote {
                Quote::Unknown => (self, Admit::Defer(Deferral::UnknownQuote)),
                Quote::Known(quote) if quote > remaining => {
                    (self, Admit::Defer(Deferral::Allowance))
                }
                Quote::Known(quote) => {
                    self.billing = Billing::Metered {
                        remaining: remaining - quote,
                    };
                    self.flight = Some(Flight {
                        reserved: quote,
                        before: remaining,
                    });
                    (
                        self,
                        Admit::Go {
                            interest: winner.interest.clone(),
                            reserved: quote,
                        },
                    )
                }
            },
            Billing::Unmetered => {
                self.flight = Some(Flight {
                    reserved: 0,
                    before: 0,
                });
                (
                    self,
                    Admit::Go {
                        interest: winner.interest.clone(),
                        reserved: 0,
                    },
                )
            }
        }
    }

    pub(crate) fn finish(self, kept: u64) -> (Self, Settle) {
        self.settle(kept)
    }

    pub(crate) fn cancel(self, kept: u64) -> (Self, Settle) {
        self.settle(kept)
    }

    fn blocked(&self) -> Option<Deferral> {
        if !self.source_reachable {
            return Some(Deferral::Unreachable);
        }
        if self.reconnecting {
            return Some(Deferral::Reconnect);
        }
        if self.presence == Presence::Sleep {
            return Some(Deferral::Sleep);
        }
        if self.pace_closed {
            return Some(Deferral::Backpressure);
        }
        None
    }

    fn settle(mut self, kept: u64) -> (Self, Settle) {
        let Some(flight) = self.flight.take() else {
            return (self, Settle::Idle);
        };
        if let Billing::Metered { remaining } = &mut self.billing {
            *remaining = flight.before.saturating_sub(kept);
        }
        let wait_ms = match self.pace_bytes_per_second {
            Some(rate) if kept > 0 => {
                self.pace_closed = true;
                kept.div_ceil(rate.get()).saturating_mul(1_000)
            }
            _ => 0,
        };
        let reconnecting = self.loss_pending;
        self.loss_pending = false;
        if reconnecting {
            self.reconnecting = true;
        }
        (
            self,
            Settle::Settled {
                kept,
                wait_ms,
                reconnecting,
            },
        )
    }
}

/// Lowest rank first. Equal ranks keep candidate order.
fn winner<'a>(schedule: &LinkSchedule, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    let mut best: Option<(u8, &Candidate)> = None;
    for candidate in candidates {
        let Some(rank) = rank(schedule, &candidate.interest) else {
            continue;
        };
        match best {
            Some((kept, _)) if kept <= rank => {}
            _ => best = Some((rank, candidate)),
        }
    }
    best.map(|(_, candidate)| candidate)
}

fn rank(schedule: &LinkSchedule, interest: &Interest) -> Option<u8> {
    match interest {
        Interest::OpenTranscript(id) if schedule.subscription == Subscription::Up => {
            schedule.open.as_ref().filter(|open| *open == id).map(|_| 0)
        }
        Interest::Catalogue => Some(1),
        Interest::OlderHistory(id)
            if schedule.subscription == Subscription::Down
                && schedule.open.as_ref() == Some(id) =>
        {
            Some(0)
        }
        Interest::OlderHistory(_) => Some(2),
        Interest::Artifact { .. } => Some(3),
        Interest::OpenTranscript(_) => None,
    }
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/domain/link.rs"]
mod tests;
