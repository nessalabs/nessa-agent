use crate::{
    attachments::domain::{TicketFingerprint, UploadTicket},
    conversation::domain::ConversationId,
};
use nessa_auth::domain::OrganizationId;

/// How many tickets may be outstanding at once. The narrower bounds sit below
/// the wider ones so one conversation cannot use up its organization's
/// tickets, nor one organization everybody's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TicketLimits {
    pub total: usize,
    pub per_organization: usize,
    pub per_conversation: usize,
}

/// Which bound a new ticket would have passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookFull {
    Total,
    Organization,
    Conversation,
}

/// What presenting a secret found. Every outcome that names a ticket has
/// already removed it from the book.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a redeemed or expired ticket has left the book; its evidence must be carried on"]
pub enum Redemption {
    /// The ticket was outstanding and in time. It is now used.
    Usable(UploadTicket),
    /// The ticket was outstanding but its time had passed.
    Expired(UploadTicket),
    /// Never issued, already used, replaced, withdrawn, or expired and swept.
    Unknown,
}

/// A ticket entered the book, and possibly pushed its own earlier copy out.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a replaced ticket has left the book; its replacement must be recorded"]
pub struct Issued {
    /// The ticket this same request was issued before. It no longer works.
    pub replaced: Option<UploadTicket>,
}

/// Every outstanding upload ticket, found by the fingerprint of its secret.
///
/// Single use, expiry, and the bounds on outstanding tickets are decided here
/// together: a ticket leaves the book exactly once (redeemed, expired, replaced
/// by the same request made again, or withdrawn with its conversation's files)
/// and the caller of each receives it so the evidence is not lost.
///
/// One request holds one ticket. A `begin` repeated with the same caller,
/// action identifier, conversation, and file does not add a ticket: it replaces
/// the earlier one. The secret is kept nowhere, only its fingerprint, so the
/// first ticket cannot be handed out a second time; a caller repeats a `begin`
/// because the answer never reached it, and then the first ticket was never
/// known to anyone and replacing it loses nothing.
#[derive(Debug)]
pub struct TicketBook {
    limits: TicketLimits,
    outstanding: Vec<(TicketFingerprint, UploadTicket)>,
}
impl TicketBook {
    pub fn new(limits: TicketLimits) -> Self {
        Self {
            limits,
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
    /// not. A conversation that lets go of its files also gives up the uploads
    /// it had been permitted and not begun. It may be permitted new ones
    /// afterwards: closing is not final, and what is held then is let go by
    /// the next close.
    #[must_use = "withdrawn tickets have left the book; why they left must be recorded"]
    pub fn withdraw(
        &mut self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
    ) -> Vec<UploadTicket> {
        let (withdrawn, kept) =
            std::mem::take(&mut self.outstanding)
                .into_iter()
                .partition(|(_, ticket)| {
                    ticket.organization_id() == organization_id
                        && ticket.conversation_id() == conversation_id
                });
        self.outstanding = kept;
        withdrawn.into_iter().map(|(_, ticket)| ticket).collect()
    }
    /// Record a new ticket, replacing the one this same request holds already.
    /// Expired tickets still count until [`Self::expire`] has returned them, so
    /// their evidence cannot be displaced by new work. A refusal changes nothing.
    pub fn issue(
        &mut self,
        fingerprint: TicketFingerprint,
        ticket: UploadTicket,
    ) -> Result<Issued, BookFull> {
        let repeated = self
            .outstanding
            .iter()
            .position(|(_, known)| known.repeats(&ticket));
        // The ticket being replaced makes room for its replacement.
        let others = |matches: &dyn Fn(&UploadTicket) -> bool| {
            self.outstanding
                .iter()
                .enumerate()
                .filter(|(index, (_, known))| Some(*index) != repeated && matches(known))
                .count()
        };
        if others(&|_| true) >= self.limits.total {
            return Err(BookFull::Total);
        }
        if others(&|known| known.organization_id() == ticket.organization_id())
            >= self.limits.per_organization
        {
            return Err(BookFull::Organization);
        }
        if others(&|known| {
            known.organization_id() == ticket.organization_id()
                && known.conversation_id() == ticket.conversation_id()
        }) >= self.limits.per_conversation
        {
            return Err(BookFull::Conversation);
        }
        let replaced = repeated.map(|index| self.outstanding.remove(index).1);
        self.outstanding.push((fingerprint, ticket));
        Ok(Issued { replaced })
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
    /// Take back a ticket that was never handed out, by its own fingerprint.
    /// Nobody ever held its secret, so nothing consequential is undone.
    pub fn withdraw_unissued(&mut self, fingerprint: &TicketFingerprint) {
        self.outstanding
            .retain(|(known, _)| !known.matches(fingerprint));
    }
}
