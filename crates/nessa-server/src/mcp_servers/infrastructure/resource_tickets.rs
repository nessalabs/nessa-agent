//! Where an MCP App resource's bytes wait for their one redemption over
//! `GET /mcp-resources` (#348).
//!
//! ```text
//! conversation service ──issue──▶ ResourceTicketStore ◀──redeem── GET /mcp-resources ──▶ McpAppAudit
//!                                   │ held: digest ─▶ bytes, the reading call's record, deadline
//!       McpAppAudit ◀── audit_ticket_ends ◀── TicketEvents ◀┘ each unredeemed end
//! ```
//!
//! Arrows are calls. The store keeps each ticket's [`ResourceTicketDigest`],
//! never the ticket, and finds a redemption by the digest of what it
//! presented. A ticket ends exactly once
//! (`every_ticket_ends_once_whatever_ends_it`): redeemed, expired, released
//! with its app or conversation, or dropped with the store. Each end frees
//! the held bytes, and each is audited against the `mcp.readResource` call
//! that read the bytes ([`HeldResource::record`]):
//! - its issue by the conversation service (`TicketIssued`), naming the
//!   ticket by [`ResourceTicketDigest::of`] the ticket `issue` answered;
//! - its redemption by the route, which records [`Redemption::audit_record`]
//!   (`TicketRedeemed`) and awaits it before it serves the bytes;
//! - every unredeemed end through [`TicketEvents`] (`TicketExpired`,
//!   initiated by the system), which composition points at
//!   [`audit_ticket_ends`].
//!
//! Expiry is found on every call the store answers, and by
//! [`ResourceTicketStore::sweep_periodically`] between them, so an
//! unredeemed ticket's bytes are let go of, and its end reported, within one
//! sweep period of its deadline even when nothing else happens.
use crate::conversation::application::{
    HeldResource, McpAppAudit, McpAppAuditPhase, McpAppAuditRecord, McpAppInitiator, McpAppRef,
    ResourceTickets, TicketRefusal, MAX_HELD_RESOURCE_BYTES, MAX_HELD_TICKETS,
    RESOURCE_TICKET_LIFETIME_MS,
};
use crate::conversation::domain::ConversationId;
use crate::mcp_servers::domain::{resource_ticket, ResourceTicketDigest};
use crate::mcp_servers::infrastructure::TokenSource;
use nessa_auth::application::ports::Clock;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError, Weak},
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// How a ticket ended unredeemed. Each ticket ends once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketEnd {
    /// Its lifetime passed.
    Expired,
    /// Its app's mount was let go of first (`release_app`).
    AppReleased,
    /// Its conversation was let go of first (`release_conversation`).
    ConversationReleased,
    /// The store itself was dropped, at shutdown.
    StoreDropped,
}

/// One ticket's unredeemed end: the reading call's record, which ticket,
/// and how it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketEvent {
    pub record: McpAppAuditRecord,
    pub ticket_digest: ResourceTicketDigest,
    pub end: TicketEnd,
}
impl TicketEvent {
    /// The audit step this end is: `TicketExpired`, whichever way the ticket
    /// ended unredeemed. Which way that was is [`TicketEvent::end`].
    pub fn phase(&self) -> McpAppAuditPhase {
        McpAppAuditPhase::TicketExpired {
            ticket_digest: self.ticket_digest.to_hex(),
        }
    }

    /// The reading call's record of this end: [`Self::phase`], taken by the
    /// gateway itself — a deadline, a cleanup, or a shutdown, never the app.
    pub fn audit_record(&self) -> McpAppAuditRecord {
        McpAppAuditRecord {
            phase: self.phase(),
            initiator: McpAppInitiator::System,
            ..self.record.clone()
        }
    }
}

/// A ticket redeemed: the bytes it held, which the route serves once their
/// redemption is on record, and which ticket it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redemption {
    pub resource: HeldResource,
    pub ticket_digest: ResourceTicketDigest,
}
impl Redemption {
    /// The reading call's record of the redemption: `TicketRedeemed`, taken
    /// by the app that read the resource, on behalf of its caller.
    pub fn audit_record(&self) -> McpAppAuditRecord {
        self.resource.audit_record(
            McpAppAuditPhase::TicketRedeemed {
                ticket_digest: self.ticket_digest.to_hex(),
            },
            self.resource.record.initiator.clone(),
        )
    }
}

/// Told of each ticket's unredeemed end, so it can be audited. A redemption
/// is not told here: the route records it itself, before serving.
///
/// Called once per such end, synchronously, after the store has let go of
/// the ticket and outside its lock. It must not block: an implementation
/// that has to await an audit store hands the event on (the channel's
/// implementation below). The store keeps no record of what it reported;
/// the receiver is the evidence's only holder from then on.
pub trait TicketEvents: Send + Sync {
    fn ticket_ended(&self, event: TicketEvent);
}

/// Each end, onto a channel whose receiver turns it into an audit record
/// ([`audit_ticket_ends`]). Unbounded so that no end is ever dropped for
/// want of room: each ticket sends at most one event, so the channel holds
/// at most one per ticket issued and not yet audited. A receiver that is
/// gone loses the evidence, and says so.
impl TicketEvents for UnboundedSender<TicketEvent> {
    fn ticket_ended(&self, event: TicketEvent) {
        if let Err(lost) = self.send(event) {
            tracing::error!(
                ticket_digest = %lost.0.ticket_digest.to_hex(),
                call_id = %lost.0.record.call_id,
                end = ?lost.0.end,
                "an MCP App resource ticket's end could not be audited: nothing receives it"
            );
        }
    }
}

/// Record each unredeemed end `events` receives, as
/// [`TicketEvent::audit_record`], in the order they ended, until every
/// sender is gone — the store's, which lives as long as the store.
///
/// A record that cannot be committed is logged, by the ticket's digest and
/// its call, never the ticket, and the next is tried: the held bytes are
/// already let go of, and the log is all that is left to say it.
pub async fn audit_ticket_ends(
    mut events: UnboundedReceiver<TicketEvent>,
    audit: Arc<dyn McpAppAudit>,
) {
    while let Some(event) = events.recv().await {
        if let Err(error) = audit.record(event.audit_record()).await {
            tracing::error!(
                ticket_digest = %event.ticket_digest.to_hex(),
                call_id = %event.record.call_id,
                end = ?event.end,
                ?error,
                "an MCP App resource ticket's end could not be audited"
            );
        }
    }
}

/// [`ResourceTickets`] in memory. Shared: the conversation service issues
/// and releases, the route redeems, and both hold the same `Arc`.
pub struct ResourceTicketStore {
    clock: Arc<dyn Clock>,
    randomness: Arc<dyn TokenSource>,
    events: Arc<dyn TicketEvents>,
    held: Mutex<Held>,
}

/// One held resource, until its deadline.
struct Ticket {
    resource: HeldResource,
    /// Unix milliseconds from which it is refused.
    expires_at: u64,
}

#[derive(Default)]
struct Held {
    tickets: HashMap<ResourceTicketDigest, Ticket>,
    /// Every ticket issued and not yet past its deadline, in issue order —
    /// which is deadline order, the lifetime being one constant — including
    /// some already ended otherwise, which the sweep passes over.
    deadlines: VecDeque<(u64, ResourceTicketDigest)>,
    /// What each conversation holds now: its bytes, and its tickets. A
    /// conversation that holds nothing has no entry.
    bytes: HashMap<ConversationId, (usize, usize)>,
}

impl Held {
    /// Let go of the ticket `digest`, if it is held.
    fn take(&mut self, digest: &ResourceTicketDigest) -> Option<Ticket> {
        let ticket = self.tickets.remove(digest)?;
        let conversation = ticket.resource.conversation_id();
        if let Some((bytes, tickets)) = self.bytes.get_mut(conversation) {
            *bytes -= ticket.resource.bytes.len();
            *tickets -= 1;
            if *tickets == 0 {
                self.bytes.remove(conversation);
            }
        }
        Some(ticket)
    }

    /// Let go of every ticket whose deadline is `now` or earlier.
    ///
    /// The deadlines are looked at in issue order. Should the wall clock step
    /// back, a later ticket's earlier deadline waits behind an earlier one's,
    /// at most a lifetime plus the step; it is refused on time all the same,
    /// because a redemption checks its own deadline.
    fn sweep(&mut self, now: u64, ended: &mut Vec<TicketEvent>) {
        while let Some(&(deadline, digest)) = self.deadlines.front() {
            if deadline > now {
                break;
            }
            self.deadlines.pop_front();
            if let Some(ticket) = self.take(&digest) {
                ended.push(event(ticket, digest, TicketEnd::Expired));
            }
        }
    }

    /// Let go of every ticket `which` selects, as having ended by `end`.
    fn release(
        &mut self,
        which: impl Fn(&HeldResource) -> bool,
        end: TicketEnd,
        ended: &mut Vec<TicketEvent>,
    ) {
        let digests: Vec<_> = self
            .tickets
            .iter()
            .filter(|(_, ticket)| which(&ticket.resource))
            .map(|(digest, _)| *digest)
            .collect();
        for digest in digests {
            if let Some(ticket) = self.take(&digest) {
                ended.push(event(ticket, digest, end));
            }
        }
    }
}

fn event(ticket: Ticket, ticket_digest: ResourceTicketDigest, end: TicketEnd) -> TicketEvent {
    TicketEvent {
        record: ticket.resource.record,
        ticket_digest,
        end,
    }
}

impl ResourceTicketStore {
    /// A store whose deadlines are read from `clock`, whose tickets are drawn
    /// from `randomness`, and whose tickets' ends are told to `events`.
    pub fn new(
        clock: Arc<dyn Clock>,
        randomness: Arc<dyn TokenSource>,
        events: Arc<dyn TicketEvents>,
    ) -> Self {
        Self {
            clock,
            randomness,
            events,
            held: Mutex::default(),
        }
    }

    /// The resource `ticket` holds, handed over once: it is let go of here,
    /// and its redemption is the caller's to record before it serves the
    /// bytes ([`Redemption::audit_record`]). `None` for anything else —
    /// a ticket never issued, already redeemed, past its deadline, released,
    /// or not a ticket at all — which the route answers alike.
    ///
    /// Found by the digest of what was presented, so no comparison with a
    /// held ticket's own bytes ever runs.
    pub fn redeem(&self, ticket: &[u8]) -> Option<Redemption> {
        let digest = ResourceTicketDigest::of(ticket);
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        let redeemed = {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            match held.take(&digest) {
                Some(ticket) if ticket.expires_at > now => Some(Redemption {
                    resource: ticket.resource,
                    ticket_digest: digest,
                }),
                Some(ticket) => {
                    ended.push(event(ticket, digest, TicketEnd::Expired));
                    None
                }
                None => None,
            }
        };
        self.report(ended);
        redeemed
    }

    /// Let go of every ticket past its deadline now.
    pub fn sweep(&self) {
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        self.lock().sweep(now, &mut ended);
        self.report(ended);
    }

    /// Sweep `store` every `period` for as long as anything else holds it.
    /// Holds it only while sweeping, so the store's drop is never waited on;
    /// returns at the first period after it.
    pub async fn sweep_periodically(store: Weak<Self>, period: Duration) {
        let mut ticks = tokio::time::interval(period);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            let Some(store) = store.upgrade() else {
                return;
            };
            store.sweep();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        // A panic elsewhere must not keep held bytes, or their ends, from
        // ever being let go of.
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn report(&self, ended: Vec<TicketEvent>) {
        for event in ended {
            self.events.ticket_ended(event);
        }
    }

    /// The digests held now, for a test that the tickets themselves are not.
    #[cfg(test)]
    pub(crate) fn held_digests(&self) -> Vec<ResourceTicketDigest> {
        self.lock().tickets.keys().copied().collect()
    }

    /// What `conversation` holds now, in bytes.
    #[cfg(test)]
    pub(crate) fn held_bytes(&self, conversation: &ConversationId) -> usize {
        self.lock()
            .bytes
            .get(conversation)
            .map_or(0, |(bytes, _)| *bytes)
    }
}

impl ResourceTickets for ResourceTicketStore {
    fn issue(&self, resource: HeldResource) -> Result<String, TicketRefusal> {
        let mut bytes = [0; 32];
        if let Err(error) = self.randomness.fill(&mut bytes) {
            tracing::error!(%error, "no MCP App resource ticket could be made");
            return Err(TicketRefusal::Unavailable);
        }
        let ticket = resource_ticket(bytes);
        let digest = ResourceTicketDigest::of(&ticket);
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        let issued = {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            let (holding, tickets) = held
                .bytes
                .get(resource.conversation_id())
                .copied()
                .unwrap_or_default();
            if holding.saturating_add(resource.bytes.len()) > MAX_HELD_RESOURCE_BYTES
                || tickets >= MAX_HELD_TICKETS
            {
                Err(TicketRefusal::Capacity)
            } else if held.tickets.contains_key(&digest) {
                // The same 256 bits twice is a random source that is not one.
                tracing::error!("an MCP App resource ticket was drawn twice; none is issued");
                Err(TicketRefusal::Unavailable)
            } else {
                let expires_at = now.saturating_add(RESOURCE_TICKET_LIFETIME_MS);
                let (bytes, tickets) = held
                    .bytes
                    .entry(resource.conversation_id().clone())
                    .or_default();
                *bytes += resource.bytes.len();
                *tickets += 1;
                held.deadlines.push_back((expires_at, digest));
                held.tickets.insert(
                    digest,
                    Ticket {
                        resource,
                        expires_at,
                    },
                );
                Ok(ticket)
            }
        };
        self.report(ended);
        issued
    }

    fn release_conversation(&self, conversation: &ConversationId) {
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            held.release(
                |resource| resource.conversation_id() == conversation,
                TicketEnd::ConversationReleased,
                &mut ended,
            );
        }
        self.report(ended);
    }

    fn release_app(&self, conversation: &ConversationId, app: &McpAppRef) {
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            held.release(
                |resource| resource.conversation_id() == conversation && resource.app() == app,
                TicketEnd::AppReleased,
                &mut ended,
            );
        }
        self.report(ended);
    }

    fn discard(&self, ticket: &str) {
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            // Taken, and not reported: it was never issued on record.
            held.take(&ResourceTicketDigest::of(ticket.as_bytes()));
        }
        self.report(ended);
    }
}

/// At shutdown every ticket still held ends unredeemed, and is reported so.
impl Drop for ResourceTicketStore {
    fn drop(&mut self) {
        let held = self.held.get_mut().unwrap_or_else(PoisonError::into_inner);
        let mut ended = Vec::new();
        held.release(|_| true, TicketEnd::StoreDropped, &mut ended);
        held.deadlines.clear();
        self.report(ended);
    }
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/resource_tickets.rs"]
mod tests;
