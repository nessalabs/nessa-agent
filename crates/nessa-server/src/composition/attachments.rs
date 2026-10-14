//! Builds the attachment store once and hands out the four things made from
//! it: the service the socket and the upload route share, where a surface
//! reads back a held file, what conversations ask, and where the agent
//! adapter reads image bytes.
use crate::{
    attachments::{
        application::{
            AttachmentDependencies, AttachmentLimits, AttachmentService, HeldReads, ImageNormalizer,
        },
        infrastructure::{
            ConversationHolds, DurableAttachmentAudit, LocalAttachmentStore, OsTicketSecrets,
            RepositoryOwnership, StoredUserImages,
        },
    },
    conversation::application::{
        AccessAction, ConversationAccessPolicyEvaluator, ConversationAttachments,
        ConversationRepository,
    },
    core::RunError,
};
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::providers::UserImageSource;
use std::{path::Path, sync::Arc};

/// One store, seen four ways.
pub(super) struct Attachments {
    pub service: AttachmentService,
    /// Where a surface reads back a file a conversation holds.
    pub reads: HeldReads,
    pub conversations: Arc<dyn ConversationAttachments>,
    pub images: Arc<dyn UserImageSource>,
}

/// Compose attachments under `root`, a private directory beside `conversations/`.
///
/// `conversations` is the conversation context's own repository: beginning an
/// upload asks it who owns a conversation, and never opens an agent to find out.
/// `access` is the conversation access policy that answers: an upload acts on
/// the conversation, and a read back of a held file is `artifact.read`.
/// `normalizer` is how an uploaded image becomes the image that is kept.
pub(super) fn attachments(
    root: &Path,
    conversations: Arc<dyn ConversationRepository>,
    access: Arc<dyn ConversationAccessPolicyEvaluator>,
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
    let ownership = Arc::new(RepositoryOwnership::new(
        conversations.clone(),
        access.clone(),
        AccessAction::Act,
    ));
    let service = AttachmentService::new(
        AttachmentDependencies {
            store: store.clone(),
            audit,
            ownership,
            secrets: Arc::new(OsTicketSecrets),
            normalizer,
            clock,
        },
        AttachmentLimits::default(),
    );
    let reads = HeldReads::new(
        store.clone(),
        Arc::new(RepositoryOwnership::new(
            conversations,
            access,
            AccessAction::ArtifactRead,
        )),
    );
    Ok(Attachments {
        conversations: Arc::new(ConversationHolds::new(service.clone(), reads.clone())),
        reads,
        images: Arc::new(StoredUserImages::new(store)),
        service,
    })
}
