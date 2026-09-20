use crate::{
    attachments::domain::{TicketFingerprint, UploadTicket},
    conversation::domain::ConversationId,
};
use nessa_auth::domain::OrganizationId;

/// The book already holds as many tickets as it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BookFull;

/// What presenting a secret found. Every outcome that names a ticket has
/// already removed it from the book.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a redeemed or expired ticket has left the book; its evidence must be carried on"]
pub enum Redemption {
    /// The ticket was outstanding and in time. It is now used.
    Usable(UploadTicket),
    /// The ticket was outstanding but its time had passed.
    Expired(UploadTicket),
    /// Never issued, already used, or already expired and swept.
    Unknown,
}

/// Every outstanding upload ticket, found by the fingerprint of its secret.
///
/// Single use, expiry, and the bound on outstanding tickets are decided here
/// together: a ticket leaves the book exactly once, by being redeemed, by
/// expiring, or by its conversation letting go of everything, and the caller
/// of each receives it so the evidence is not lost.
#[derive(Debug)]
pub struct TicketBook {
    capacity: usize,
    outstanding: Vec<(TicketFingerprint, UploadTicket)>,
}
impl TicketBook {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            outstanding: Vec::new(),
        }
    }
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }
    /// Remove and return every ticket whose time has passed, oldest first.
    #[must_use = "expired tickets have left the book; their expiry must be recorded"]
    pub fn expire(&mut self, now_ms: u64) -> Vec<UploadTicket> {
        let (kept, expired) = std::mem::take(&mut self.outstanding)
            .into_iter()
            .partition(|(_, ticket)| ticket.lifetime().is_usable_at(now_ms));
        self.outstanding = kept;
        expired.into_iter().map(|(_, ticket)| ticket).collect()
    }
    /// Remove and return every ticket issued for one conversation, in time or
    /// not. A conversation that lets go of its files also gives up what it was
    /// still allowed to add, so nothing can arrive after it has closed.
    #[must_use = "voided tickets have left the book; why they left must be recorded"]
    pub fn void(
        &mut self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
    ) -> Vec<UploadTicket> {
        let (voided, kept) =
            std::mem::take(&mut self.outstanding)
                .into_iter()
                .partition(|(_, ticket)| {
                    ticket.organization_id() == organization_id
                        && ticket.conversation_id() == conversation_id
                });
        self.outstanding = kept;
        voided.into_iter().map(|(_, ticket)| ticket).collect()
    }
    /// Record a new ticket. Expired tickets still count until [`Self::expire`]
    /// has returned them, so their evidence cannot be displaced by new work.
    pub fn issue(
        &mut self,
        fingerprint: TicketFingerprint,
        ticket: UploadTicket,
    ) -> Result<(), BookFull> {
        if self.outstanding.len() >= self.capacity {
            return Err(BookFull);
        }
        self.outstanding.push((fingerprint, ticket));
        Ok(())
    }
    /// Use a ticket. Every entry is compared, in constant time each, so how
    /// long this takes says nothing about which ticket matched or whether one did.
    pub fn redeem(&mut self, presented: &TicketFingerprint, now_ms: u64) -> Redemption {
        let mut found = None;
        for (index, (fingerprint, _)) in self.outstanding.iter().enumerate() {
            if fingerprint.matches(presented) {
                found = Some(index);
            }
        }
        let Some(index) = found else {
            return Redemption::Unknown;
        };
        let (_, ticket) = self.outstanding.remove(index);
        if ticket.lifetime().is_usable_at(now_ms) {
            Redemption::Usable(ticket)
        } else {
            Redemption::Expired(ticket)
        }
    }
}
