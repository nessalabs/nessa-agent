//! Owner-scoped current conversation metadata for bounded linked readers.
//! The repository owns revisions; this port exposes no receiver progress.

use super::{ConversationFuture, Reader};
use crate::conversation::domain::Conversation;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::{ConversationId, ConversationSummary};
use nessa_sync::replication::catalogue::ManifestRequest;

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
/// granted (`read_grants`). The owner's head moves on every catalogue change.
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
