//! Device enrollment's receiver port, answered by the conversation context's
//! receiver authority (`LocalReceiverAuthority`).
//!
//! The authority keeps receiver identity, epochs and their journal. This
//! adapter translates an enrollment stage into its calls: the owner who paired
//! is the principal, and the request text is derived from the stage's durable
//! correlation, so every retry of one stage names the same receipt.
use super::runtime::hex;
use crate::conversation::{
    domain::{PairedReceiver, ReceiverBinding},
    infrastructure::{LocalReceiverAuthority, ReceiverChangeError},
};
use crate::device_pairing::application::{PairingReceivers, ReceiverError, ReceiverRequest};
use nessa_auth::{application::pairing::ReceiverOutcome, domain::ResourceId};
use std::sync::Arc;

/// The receiver authority, as device enrollment asks for it.
pub struct ConversationReceivers(Arc<LocalReceiverAuthority>);
impl ConversationReceivers {
    /// Answer enrollment's receiver calls from `authority`.
    pub fn new(authority: Arc<LocalReceiverAuthority>) -> Self {
        Self(authority)
    }
}

/// The pair receipt's request text: one per stage, so a retry finds it.
fn stage_request(request: &ReceiverRequest) -> String {
    format!("pairing-stage-{}", hex(request.request().bytes()))
}
/// The fence's request text, derived from the same stage.
fn fence_request(request: &ReceiverRequest) -> String {
    format!("pairing-fence-{}", hex(request.request().bytes()))
}

/// The pairing a stage recorded, as the conversation domain names it.
fn paired(request: &ReceiverRequest, receiver: &ResourceId, paired_epoch: u64) -> PairedReceiver {
    PairedReceiver {
        receiver_id: receiver.as_str().to_owned(),
        credential_id: request.credential().clone(),
        organization_id: request.organization().clone(),
        owner_id: request.owner().clone(),
        paired_epoch,
    }
}

fn receiver_error(error: ReceiverChangeError) -> ReceiverError {
    match error {
        ReceiverChangeError::Unavailable => ReceiverError::Unavailable,
        ReceiverChangeError::Conflict => ReceiverError::Conflict,
        ReceiverChangeError::Missing => ReceiverError::Missing,
        ReceiverChangeError::Exhausted => ReceiverError::Exhausted,
    }
}

/// The authority's binding as Auth's receiver outcome for this stage. Auth's
/// constructor refuses a zero epoch or generation; the registry then checks
/// the outcome against the stage it was asked for.
fn outcome(
    request: &ReceiverRequest,
    binding: ReceiverBinding,
) -> Result<ReceiverOutcome, ReceiverError> {
    let receiver = ResourceId::new(binding.receiver_id).map_err(|_| ReceiverError::Conflict)?;
    ReceiverOutcome::new(
        binding.credential_id,
        request.request(),
        receiver,
        binding.access_epoch,
        request.generation(),
    )
    .map_err(|_| ReceiverError::Conflict)
}

impl PairingReceivers for ConversationReceivers {
    fn pair(&self, request: &ReceiverRequest) -> Result<ReceiverOutcome, ReceiverError> {
        let binding = self
            .0
            .pair_now(
                request.credential().clone(),
                request.organization().clone(),
                request.owner().clone(),
                request.owner().clone(),
                stage_request(request),
            )
            .map_err(receiver_error)?;
        outcome(request, binding)
    }

    fn paired(&self, request: &ReceiverRequest) -> Result<Option<ReceiverOutcome>, ReceiverError> {
        self.0
            .paired(request.owner(), &stage_request(request))
            .map_err(receiver_error)?
            .map(|binding| outcome(request, binding))
            .transpose()
    }

    fn holding(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<Option<u64>, ReceiverError> {
        self.0
            .holding(&paired(request, receiver, paired_epoch))
            .map(|current| current.map(|binding| binding.access_epoch))
            .map_err(receiver_error)
    }

    fn fence(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<ReceiverOutcome, ReceiverError> {
        let binding = self
            .0
            .fence(
                &paired(request, receiver, paired_epoch),
                fence_request(request),
            )
            .map_err(receiver_error)?;
        outcome(request, binding)
    }
}
