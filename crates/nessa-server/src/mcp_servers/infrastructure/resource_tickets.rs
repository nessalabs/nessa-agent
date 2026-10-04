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
//!   ticket by [`ResourceTicketDigest::of`] the ticket `issue` answered,
//!   while the ticket is still pending: not redeemable, and its end not the
//!   store's to report;
//! - its redemption by the route, which records [`Redemption::audit_record`]
//!   (`TicketRedeemed`) and awaits it before it serves the bytes;
//! - every unredeemed end of an active ticket through [`TicketEvents`]
//!   (`TicketEnded`, by its cause and who caused it), which composition
//!   points at [`audit_ticket_ends`];
//! - a pending ticket's end by the conversation service, after its issue,
//!   from what [`ResourceTickets::activate`] answers.
//!
//! Expiry is found on every call the store answers, and by
//! [`ResourceTicketStore::sweep_periodically`] between them, so an
//! unredeemed ticket's bytes are let go of, and its end reported, within one
//! sweep period of its deadline even when nothing else happens.
use crate::conversation::application::{
    HeldResource, McpAppAudit, McpAppAuditPhase, McpAppAuditRecord, McpAppInitiator, McpAppRef,
    ResourceTickets, TicketEnd, TicketRefusal, MAX_HELD_RESOURCE_BYTES, MAX_HELD_TICKETS,
    RESOURCE_TICKET_LIFETIME_MS,
};
use crate::mcp_servers::domain::{resource_ticket, ResourceTicketDigest};
use crate::mcp_servers::infrastructure::TokenSource;
use nessa_auth::application::ports::Clock;
use nessa_protocol::conversation::domain::ConversationId;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError, Weak},
    time::Duration,
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};

/// One active ticket's unredeemed end: the reading call's record, which
/// ticket, how it ended, and who ended it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketEvent {
    pub record: McpAppAuditRecord,
    pub ticket_digest: ResourceTicketDigest,
    pub end: TicketEnd,
    pub by: McpAppInitiator,
}
impl TicketEvent {
    /// The reading call's record of this end: `TicketEnded` by its cause,
    /// taken by whoever ended it — the system for a deadline or a shutdown,
    /// the releaser or the person who ended the conversation otherwise.
    pub fn audit_record(&self) -> McpAppAuditRecord {
        McpAppAuditRecord {
            phase: McpAppAuditPhase::TicketEnded {
                ticket_digest: self.ticket_digest.to_hex(),
                cause: self.end,
            },
            initiator: self.by.clone(),
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
/// [`TicketEvent::audit_record`], in the order they ended — until every
/// sender is gone, or `stop` says the gateway is stopping, when every end
/// already sent is recorded before it returns.
///
/// A record that cannot be committed is logged, by the ticket's digest and
/// its call, never the ticket, and the next is tried: the held bytes are
/// already let go of, and the log is all that is left to say it.
pub async fn audit_ticket_ends(
    mut events: UnboundedReceiver<TicketEvent>,
    audit: Arc<dyn McpAppAudit>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        // Ends first: stopping is taken only once none is waiting, so every
        // end the conversations' ends already sent is recorded before it.
        tokio::select! {
            biased;
            event = events.recv() => match event {
                Some(event) => record_end(audit.as_ref(), event).await,
                None => return,
            },
            _ = &mut stop => return,
        }
    }
}

async fn record_end(audit: &dyn McpAppAudit, event: TicketEvent) {
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
    /// Issued, and its issue not yet on record: not redeemable, and its end
    /// not reported.
    pending: bool,
}

#[derive(Default)]
struct Held {
    tickets: HashMap<ResourceTicketDigest, Ticket>,
    /// Pending tickets that ended before they were made redeemable, how and
    /// by whom, for [`ResourceTickets::activate`] to answer — however long
    /// their issue took to record. Taken by `activate` or `discard`, which
    /// the issuer always reaches; at most one per app call running.
    ended_pending: HashMap<ResourceTicketDigest, (TicketEnd, McpAppInitiator)>,
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
            self.end(digest, TicketEnd::Expired, &McpAppInitiator::System, ended);
        }
    }

    /// End the ticket `digest`, if it is held, as having ended by `end`,
    /// which `by` caused: reported if it was active, kept for its issuer to
    /// learn if it was pending.
    fn end(
        &mut self,
        digest: ResourceTicketDigest,
        end: TicketEnd,
        by: &McpAppInitiator,
        ended: &mut Vec<TicketEvent>,
    ) {
        let Some(ticket) = self.take(&digest) else {
            return;
        };
        if ticket.pending {
            self.ended_pending.insert(digest, (end, by.clone()));
        } else {
            ended.push(TicketEvent {
                record: ticket.resource.record,
                ticket_digest: digest,
                end,
                by: by.clone(),
            });
        }
    }

    /// End every ticket `which` selects, as having ended by `end`.
    fn release(
        &mut self,
        which: impl Fn(&HeldResource) -> bool,
        end: TicketEnd,
        by: &McpAppInitiator,
        ended: &mut Vec<TicketEvent>,
    ) {
        let digests: Vec<_> = self
            .tickets
            .iter()
            .filter(|(_, ticket)| which(&ticket.resource))
            .map(|(digest, _)| *digest)
            .collect();
        for digest in digests {
            self.end(digest, end, by, ended);
        }
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
            match held.tickets.get(&digest) {
                // Pending: not handed out yet, so not this caller's to spend.
                None | Some(Ticket { pending: true, .. }) => None,
                Some(ticket) if ticket.expires_at > now => {
                    held.take(&digest).map(|ticket| Redemption {
                        resource: ticket.resource,
                        ticket_digest: digest,
                    })
                }
                Some(_) => {
                    held.end(
                        digest,
                        TicketEnd::Expired,
                        &McpAppInitiator::System,
                        &mut ended,
                    );
                    None
                }
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
                        pending: true,
                    },
                );
                Ok(ticket)
            }
        };
        self.report(ended);
        issued
    }

    fn activate(&self, ticket: &str) -> Result<(), (TicketEnd, McpAppInitiator)> {
        let digest = ResourceTicketDigest::of(ticket.as_bytes());
        let mut held = self.lock();
        if let Some(ended) = held.ended_pending.remove(&digest) {
            return Err(ended);
        }
        match held.tickets.get_mut(&digest) {
            Some(ticket) => {
                ticket.pending = false;
                Ok(())
            }
            // Neither held nor ended while pending: past its deadline and
            // forgotten, which is an expiry the system caused.
            None => Err((TicketEnd::Expired, McpAppInitiator::System)),
        }
    }

    fn discard(&self, ticket: &str) {
        let digest = ResourceTicketDigest::of(ticket.as_bytes());
        let mut held = self.lock();
        // Never issued on record, so nothing of it is reported.
        held.ended_pending.remove(&digest);
        held.take(&digest);
    }

    fn release_conversation(&self, conversation: &ConversationId, by: &McpAppInitiator) {
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            held.release(
                |resource| resource.conversation_id() == conversation,
                TicketEnd::ConversationEnded,
                by,
                &mut ended,
            );
        }
        self.report(ended);
    }

    fn release_app(&self, conversation: &ConversationId, app: &McpAppRef, by: &McpAppInitiator) {
        let now = self.clock.unix_milliseconds();
        let mut ended = Vec::new();
        {
            let mut held = self.lock();
            held.sweep(now, &mut ended);
            held.release(
                |resource| resource.conversation_id() == conversation && resource.app() == app,
                TicketEnd::AppReleased,
                by,
                &mut ended,
            );
        }
        self.report(ended);
    }
}

/// At shutdown every ticket still held ends unredeemed, by the system, and
/// is reported so — once its conversation's end has not already let it go.
impl Drop for ResourceTicketStore {
    fn drop(&mut self) {
        let held = self.held.get_mut().unwrap_or_else(PoisonError::into_inner);
        let mut ended = Vec::new();
        held.release(
            |_| true,
            TicketEnd::ConversationEnded,
            &McpAppInitiator::System,
            &mut ended,
        );
        held.deadlines.clear();
        self.report(ended);
    }
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/resource_tickets.rs"]
mod tests;
