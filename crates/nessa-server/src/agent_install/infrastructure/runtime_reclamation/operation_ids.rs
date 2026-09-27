use uuid::Uuid;

use crate::agent_install::{
    application::{
        ReclamationOperationIds, ReclamationPersistenceFailure, ReclamationPersistenceStage,
    },
    domain::ReclamationOperationId,
};

/// Generates unpredictable identities for durable runtime-removal attempts.
pub struct UuidReclamationOperationIds;

impl ReclamationOperationIds for UuidReclamationOperationIds {
    fn next(&self) -> Result<ReclamationOperationId, ReclamationPersistenceFailure> {
        ReclamationOperationId::new(Uuid::new_v4().to_string()).map_err(|error| {
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainAdmission,
                error.to_string(),
            )
        })
    }
}
