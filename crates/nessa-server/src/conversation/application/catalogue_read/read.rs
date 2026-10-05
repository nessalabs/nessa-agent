//! Fresh owner admission followed by one bounded current catalogue operation.
//!
//! Admission owns credential, receiver, owner, and epoch. The injected source
//! owns physical catalogue identity and I/O; sync-engine owns pass validation.

use crate::conversation::application::{AdmitPassiveRead, RecordReadLease};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::read_scope::{
    passive_read_selector, validate_catalogue_selector, CatalogueReadScope, ReadRefusal,
};
use nessa_sync::replication::{
    catalogue::{
        validate_catalogue_pass, validate_manifest_request, CataloguePass, ManifestEntry,
        ManifestPage, ManifestRequest, ResolvedEntry, MAX_CATALOGUE_ENTRIES,
    },
    domain::Scope,
};
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogueReadError {
    Admission(ReadRefusal),
    InvalidRequest,
    IdentityChanged,
    SourceUnavailable,
    WorkerPanicked,
    OperationAndWorkerPanicked(Box<CatalogueReadError>),
    OversizedEntry,
}

#[derive(Clone)]
pub enum CatalogueReadOperation {
    Head,
    Manifest(ManifestRequest),
    Resolve {
        pass: CataloguePass,
        descriptor: ManifestEntry,
        max_payload_bytes: usize,
    },
}

impl CatalogueReadOperation {
    pub fn scope(&self) -> Option<&Scope> {
        match self {
            Self::Head => None,
            Self::Manifest(request) => Some(&request.pass.scope),
            Self::Resolve { pass, .. } => Some(&pass.scope),
        }
    }
}

pub enum CatalogueReadValue {
    Head { scope: Scope, head: u64 },
    Manifest(ManifestPage),
    Resolve(ResolvedEntry),
}

pub struct CatalogueReadResponse {
    pub value: CatalogueReadValue,
    pub lease: RecordReadLease,
}

pub type CatalogueReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CatalogueReadResponse, CatalogueReadError>> + Send + 'a>>;

/// Source reads consume trusted admitted owner facts. Its worker owns the lease
/// across caller cancellation and returns it after joining source work.
pub trait CatalogueReadSource: Send + Sync {
    fn read(
        &self,
        admitted: CatalogueReadScope,
        operation: CatalogueReadOperation,
        lease: RecordReadLease,
    ) -> CatalogueReadFuture<'_>;
}

pub struct ReadCatalogue<'a> {
    pub admission: AdmitPassiveRead<'a>,
    pub source: &'a dyn CatalogueReadSource,
}

impl ReadCatalogue<'_> {
    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
        receiver: &str,
        epoch: u64,
        operation: CatalogueReadOperation,
        lease: RecordReadLease,
    ) -> Result<CatalogueReadResponse, CatalogueReadError> {
        match &operation {
            CatalogueReadOperation::Head => {}
            CatalogueReadOperation::Manifest(request) => {
                validate_manifest_request(request, MAX_CATALOGUE_ENTRIES)
                    .map_err(|_| CatalogueReadError::InvalidRequest)?
            }
            CatalogueReadOperation::Resolve { pass, .. } => {
                validate_catalogue_pass(pass).map_err(|_| CatalogueReadError::InvalidRequest)?
            }
        }
        let admitted = self
            .admission
            .catalogue(session, receiver, epoch)
            .await
            .map_err(CatalogueReadError::Admission)?;
        passive_read_selector(&admitted.receiver_id, admitted.access_epoch)
            .map_err(CatalogueReadError::Admission)?;
        if let Some(scope) = operation.scope() {
            validate_catalogue_selector(&admitted, scope).map_err(CatalogueReadError::Admission)?;
        }
        let expected = operation.clone();
        let response = self.source.read(admitted.clone(), operation, lease).await?;
        let scope = match (&expected, &response.value) {
            (CatalogueReadOperation::Head, CatalogueReadValue::Head { scope, .. }) => scope,
            (CatalogueReadOperation::Manifest(request), CatalogueReadValue::Manifest(page))
                if &page.request == request =>
            {
                &page.request.pass.scope
            }
            (CatalogueReadOperation::Resolve { pass, .. }, CatalogueReadValue::Resolve(_)) => {
                &pass.scope
            }
            _ => return Err(CatalogueReadError::Admission(ReadRefusal::Unverifiable)),
        };
        validate_catalogue_selector(&admitted, scope).map_err(CatalogueReadError::Admission)?;
        Ok(response)
    }
}
