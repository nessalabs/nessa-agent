//! Bounded owner catalogue source execution using the shared passive worker owner.
//!
//! Trusted scope is checked before metadata I/O. Each outer worker owns one
//! admitted lease and joins the #259 source before returning it to the writer.

use super::operation;
use crate::conversation::application::{
    CatalogueEntryAccess, CatalogueReadError, CatalogueReadFuture, CatalogueReadOperation,
    CatalogueReadResponse, CatalogueReadSource, ConversationAccessPolicyEvaluator,
    ConversationCatalogue, RecordReadLease,
};
use crate::core::read_workers::{ReadWorkerError, ReadWorkers};
use nessa_auth::domain::PrincipalId;
use nessa_protocol::conversation::read_scope::CatalogueReadScope;
use nessa_sync::replication::domain::Id;
use std::sync::Arc;

pub struct NessaCatalogueReadSource {
    catalogue: Arc<dyn ConversationCatalogue>,
    /// Admits each entry a read returns (`catalogue.entry`).
    access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    origin: Id,
    workers: Arc<ReadWorkers>,
}

impl NessaCatalogueReadSource {
    pub fn new(
        catalogue: Arc<dyn ConversationCatalogue>,
        access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
        origin: Id,
    ) -> Self {
        Self {
            catalogue,
            access_policy,
            origin,
            workers: ReadWorkers::new(),
        }
    }
    pub async fn shutdown(&self) -> Result<(), CatalogueReadError> {
        self.workers.shutdown().await.map_err(worker_error)
    }
}

impl CatalogueReadSource for NessaCatalogueReadSource {
    fn read(
        &self,
        admitted: CatalogueReadScope,
        signed_in_as: PrincipalId,
        operation: CatalogueReadOperation,
        lease: RecordReadLease,
    ) -> CatalogueReadFuture<'_> {
        Box::pin(async move {
            operation::trusted_scope(&admitted, &self.origin, &operation)?;
            let catalogue = self.catalogue.clone();
            let entries = CatalogueEntryAccess::new(self.access_policy.clone(), signed_in_as);
            let origin = self.origin.clone();
            let workers = self.workers.clone();
            self.workers
                .run("nessa-catalogue-read", move || {
                    let value = operation::execute(catalogue, entries, admitted, origin, operation)
                        .map_err(|error| {
                            if matches!(
                                error,
                                CatalogueReadError::WorkerPanicked
                                    | CatalogueReadError::OperationAndWorkerPanicked(_)
                            ) {
                                workers.worker_panicked();
                            }
                            error
                        })?;
                    Ok(CatalogueReadResponse { value, lease })
                })
                .await
                .map_err(worker_error)?
        })
    }
}

fn worker_error(error: ReadWorkerError) -> CatalogueReadError {
    match error {
        ReadWorkerError::Unavailable => CatalogueReadError::SourceUnavailable,
        ReadWorkerError::WorkerPanicked => CatalogueReadError::WorkerPanicked,
    }
}
