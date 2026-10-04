//! Offline reads return retained evidence without a credential or source port.
use super::{CacheError, CachedCataloguePage, CachedProgress};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::view::ConversationView;
use nessa_sync::replication::{catalogue::EntryKey, domain::Id};
use uuid::Uuid;

pub(crate) enum SavedTranscript {
    NotLoaded,
    Deleted,
    Retained {
        progress: CachedProgress,
        view: Box<ConversationView>,
    },
}

pub(crate) trait SavedReads {
    fn catalogue_page(
        &mut self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
        after: Option<&EntryKey>,
        max_entries: usize,
    ) -> Result<Option<CachedCataloguePage>, CacheError>;

    fn transcript_view(
        &mut self,
        receiver: &Id,
        origin: &Id,
        conversation: &ConversationId,
        revision: Uuid,
    ) -> Result<SavedTranscript, CacheError>;
}
