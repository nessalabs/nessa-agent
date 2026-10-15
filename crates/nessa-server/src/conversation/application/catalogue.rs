//! Owner-scoped current conversation metadata for bounded linked readers.
//! The repository owns revisions; this port exposes no receiver progress.
//!
//! ```text
//! paired catalogue read ──▶ ConversationCatalogue::page | resolve
//!                             rows narrowed in SQL to the receiver's
//!                             grants: an index, each row with the capabilities held
//!                       ──▶ CatalogueEntryAdmission::admits(descriptor)
//!                             ──▶ ConversationAccessPolicyEvaluator
//!                                 (catalogue.entry, a PairedReader)
//!                             ──▶ Shown | Dropped (logged) | Undecided
//! ```
//!
//! Arrows are calls. The index finds what a reader may be shown; the access
//! policy decides each entry it found, and an entry it refuses is left out.

use super::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    ConversationAccessPolicyEvaluator, ConversationFuture, Reader, RequestShape,
};
use crate::conversation::domain::{Capabilities, Conversation};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::{ConversationId, ConversationSummary};
use nessa_sync::replication::catalogue::ManifestRequest;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueHead {
    pub incarnation: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueKey {
    pub creation: u64,
    pub id: ConversationId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueDescriptor {
    pub key: CatalogueKey,
    pub revision: u64,
    pub deleted: bool,
    /// The capabilities of the share grant the reader holds on the conversation,
    /// read with the row; `None` for the owner, who holds none.
    pub held: Option<Capabilities>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CataloguePage {
    pub entries: Vec<CatalogueDescriptor>,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CataloguePageRequest {
    pub organization: OrganizationId,
    pub owner: PrincipalId,
    /// A paired device pages only the rows granted to it; the owner, all.
    pub reader: Reader,
    /// The actual finite-pass request; query fields are derived, not mirrored.
    pub manifest: ManifestRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueValue {
    pub descriptor: CatalogueDescriptor,
    pub conversation: Conversation,
    pub summary: Option<ConversationSummary>,
}

/// Each read is independently authorized by exact organization, principal and
/// database incarnation. The caller supplies its current authenticated owner;
/// a sync adapter additionally checks its exact scope and access epoch.
/// `reader` narrows a head, a page and a resolve to what that reader was
/// granted (`share_grants`). The owner's head moves on every catalogue change.
/// A paired reader's head is the latest revision among its granted rows and
/// its own grant changes, so a change to a row it may not read never moves
/// it, while a grant, a revoke or a change to a granted row does. In steady
/// state it never goes backwards: rows are never removed, a row's revision
/// only grows, and a revoke journals the revision it stamps on the row it
/// takes away. The values are still the owner's catalogue revisions, so the
/// gap between two of them counts the owner's other changes. A reader whose
/// progress was saved against the owner's head before this rule finds the
/// head below it once (`docs/design/auth/peer-gateways.md`, Known limits).
pub trait ConversationCatalogue: Send + Sync {
    fn head(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        reader: &Reader,
    ) -> ConversationFuture<'_, CatalogueHead>;
    fn page(&self, request: CataloguePageRequest) -> ConversationFuture<'_, CataloguePage>;
    fn resolve(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        reader: &Reader,
        incarnation: &str,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<CatalogueValue>>;
}

/// Who reads a paired catalogue, and the policy each entry it is shown is
/// admitted by (`catalogue.entry`). Built once per read source.
#[derive(Clone)]
pub struct CatalogueEntryAccess {
    policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    signed_in_as: PrincipalId,
}

impl CatalogueEntryAccess {
    /// A reader whose session signed in as `signed_in_as`, admitted by
    /// `policy`.
    pub fn new(
        policy: Arc<dyn ConversationAccessPolicyEvaluator>,
        signed_in_as: PrincipalId,
    ) -> Self {
        Self {
            policy,
            signed_in_as,
        }
    }

    /// The admission of the entries `receiver`, bound to `owner` in
    /// `organization`, is shown of that owner's catalogue, for one read.
    pub fn reading<'a>(
        &'a self,
        organization: &'a OrganizationId,
        owner: &'a PrincipalId,
        receiver: &'a str,
        reader: &'a Reader,
    ) -> CatalogueEntryAdmission<'a> {
        CatalogueEntryAdmission {
            access: self,
            organization,
            owner,
            receiver,
            reader,
            decided: HashMap::new(),
        }
    }
}

/// What the policy said of one catalogue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogueEntryAnswer {
    /// Admitted: the reader is shown it.
    Shown,
    /// Refused: it is left out, and that logged.
    Dropped,
    /// The policy could not decide: the read is unavailable.
    Undecided,
}

/// One read's admission of the entries a paired reader is shown.
///
/// Within one read the reader, the organization and the owner are fixed:
/// the rows are the owner's, as the store checks each record. An ask varies
/// by entry only in whether it was deleted and the capabilities held, so each
/// distinct pair is asked once and the answer reused: a page costs at most
/// a handful of evaluations however many entries it has.
pub struct CatalogueEntryAdmission<'a> {
    access: &'a CatalogueEntryAccess,
    organization: &'a OrganizationId,
    owner: &'a PrincipalId,
    receiver: &'a str,
    reader: &'a Reader,
    decided: HashMap<(bool, Option<Capabilities>), CatalogueEntryAnswer>,
}

impl CatalogueEntryAdmission<'_> {
    /// Whether the reader is shown `entry`. A refusal is logged at warn: the
    /// SQL index found the entry by the reader's grant, so the policy
    /// refusing it means the two disagree.
    pub fn admits(&mut self, entry: &CatalogueDescriptor) -> CatalogueEntryAnswer {
        let key = (entry.deleted, entry.held);
        let answer = match self.decided.get(&key) {
            Some(answer) => *answer,
            None => {
                let answer = self.decide(entry);
                self.decided.insert(key, answer);
                answer
            }
        };
        if answer == CatalogueEntryAnswer::Dropped {
            tracing::warn!(
                conversation = %entry.key.id,
                receiver = self.receiver,
                held = ?entry.held,
                deleted = entry.deleted,
                "the access policy refused a catalogue entry its grant index returned; it is left out"
            );
        }
        answer
    }

    fn decide(&self, entry: &CatalogueDescriptor) -> CatalogueEntryAnswer {
        let ask = AccessAsk {
            subject: if matches!(self.reader, Reader::OwnerPhone(_)) {
                AccessSubject::Person {
                    organization: self.organization,
                    principal: &self.access.signed_in_as,
                }
            } else {
                AccessSubject::PairedReader {
                    organization: self.organization,
                    signed_in_as: &self.access.signed_in_as,
                    binding_owner: self.owner,
                    receiver: self.receiver,
                }
            },
            action: if matches!(self.reader, Reader::OwnerPhone(_)) {
                AccessAction::Locate
            } else {
                AccessAction::CatalogueEntry
            },
            resource: AccessResource::Conversation {
                organization: self.organization,
                owner: self.owner,
                deleted: entry.deleted,
            },
            held: entry.held,
            request: RequestShape::default(),
            target: None,
        };
        match self.access.policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => CatalogueEntryAnswer::Shown,
            AccessDecision::Deny(AccessDenial::Unavailable) => CatalogueEntryAnswer::Undecided,
            AccessDecision::Deny(denial) => {
                tracing::debug!(?denial, "a catalogue entry was refused");
                CatalogueEntryAnswer::Dropped
            }
        }
    }
}
