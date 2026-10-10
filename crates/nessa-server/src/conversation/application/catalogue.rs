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
/// `reader` narrows a page and a resolve to what that reader was granted
/// (`read_grants`); the head is the owner's, so a grant change moves it.
pub trait ConversationCatalogue: Send + Sync {
    fn head(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
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
