//! Builds the attachment store once and hands out the three things made from
//! it: the service the socket and the upload route share, what conversations
//! ask, and where the agent adapter reads image bytes.
use crate::{
    attachments::{
        application::{
            AttachmentDependencies, AttachmentLimits, AttachmentService, ImageNormalizer,
        },
        infrastructure::{
            ConversationHolds, DurableAttachmentAudit, LocalAttachmentStore, OsTicketSecrets,
            RepositoryOwnership, StoredUserImages,
        },
    },
    conversation::application::{ConversationAttachments, ConversationRepository},
    core::RunError,
};
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::providers::UserImageSource;
use std::{path::Path, sync::Arc};

/// One store, seen three ways.
pub(super) struct Attachments {
    pub service: AttachmentService,
    pub conversations: Arc<dyn ConversationAttachments>,
    pub images: Arc<dyn UserImageSource>,
}

/// Compose attachments under `root`, a private directory beside `conversations/`.
///
/// `conversations` is the conversation context's own repository: beginning an
/// upload asks it who owns a conversation, and never opens an agent to find out.
/// `normalizer` is how an uploaded image becomes the image that is kept.
pub(super) fn attachments(
    root: &Path,
    conversations: Arc<dyn ConversationRepository>,
    normalizer: Arc<dyn ImageNormalizer>,
    clock: Arc<dyn Clock>,
) -> Result<Attachments, RunError> {
    let store = Arc::new(
        LocalAttachmentStore::open(root.to_path_buf())
            .map_err(|error| RunError::Agent(format!("attachment storage: {error}")))?,
    );
    let audit = Arc::new(
        DurableAttachmentAudit::new(root.join("audit"), clock.clone())
            .map_err(|_| RunError::Agent("attachment audit storage is not private".into()))?,
    );
    let service = AttachmentService::new(
        AttachmentDependencies {
            store: store.clone(),
            audit,
            ownership: Arc::new(RepositoryOwnership::new(conversations)),
            secrets: Arc::new(OsTicketSecrets),
            normalizer,
            clock,
        },
        AttachmentLimits::default(),
    );
    Ok(Attachments {
        conversations: Arc::new(ConversationHolds::new(service.clone())),
        images: Arc::new(StoredUserImages::new(store)),
        service,
    })
}
