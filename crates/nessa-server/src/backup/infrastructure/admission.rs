//! The export hold is the conversation service's admission lock.

use async_trait::async_trait;

use crate::backup::application::{AdmissionFailure, AdmissionLease, MutationAdmission};
use crate::conversation::application::{ConversationService, ExportAdmission, ExportHoldFailure};

/// Holds `ConversationService::hold_for_export` for one cut.
pub struct ConversationAdmission {
    service: ConversationService,
}

impl ConversationAdmission {
    pub fn new(service: ConversationService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl MutationAdmission for ConversationAdmission {
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure> {
        match self.service.hold_for_export().await {
            Ok(hold) => Ok(Box::new(ServiceHold(hold))),
            Err(ExportHoldFailure::Retired) => Err(AdmissionFailure::Retired),
        }
    }
}

struct ServiceHold(ExportAdmission);

impl AdmissionLease for ServiceHold {
    fn retirement_signaled(&self) -> bool {
        self.0.retirement_signaled()
    }
}
