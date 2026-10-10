//! Physical catalogue reads after trusted owner admission.

use crate::conversation::{
    application::{
        CatalogueReadError, CatalogueReadOperation, CatalogueReadValue, ConversationCaller,
        ConversationCatalogue,
    },
    infrastructure::{CatalogueWorkerError, NessaCatalogueSource},
};
#[cfg(test)]
use nessa_protocol::conversation::domain::conversation_catalogue_schema;
use nessa_protocol::conversation::read_scope::{
    check_catalogue_scope_identity, passive_read_selector, validate_catalogue_selector,
    CatalogueReadScope,
};
use nessa_sync::replication::{
    catalogue::{CatalogueSource, CatalogueSourceError},
    domain::Id,
};
use std::sync::Arc;

fn caller(admitted: &CatalogueReadScope) -> ConversationCaller {
    ConversationCaller {
        organization_id: admitted.organization_id.clone(),
        principal_id: admitted.owner_id.clone(),
        surface_id: admitted.receiver_id.clone(),
        action_id: "catalogue-read".into(),
    }
}

/// Compare all non-physical scope facts before the metadata port is called.
/// Incarnation is verified by the metadata adapter in its read transaction.
pub(super) fn trusted_scope(
    admitted: &CatalogueReadScope,
    origin: &Id,
    operation: &CatalogueReadOperation,
) -> Result<(), CatalogueReadError> {
    if let Some(scope) = operation.scope() {
        validate_catalogue_selector(admitted, scope).map_err(CatalogueReadError::Admission)?;
        if scope.origin() != origin {
            return Err(CatalogueReadError::IdentityChanged);
        }
        check_catalogue_scope_identity(&admitted.organization_id, &admitted.owner_id, scope)
            .map_err(source_error)?;
    }
    Ok(())
}

/// Run outside a Tokio enter so dropping the synchronous source joins its
/// internal worker before the outer executor releases the admitted work lease.
pub(super) fn execute(
    catalogue: Arc<dyn ConversationCatalogue>,
    admitted: CatalogueReadScope,
    origin: Id,
    operation: CatalogueReadOperation,
) -> Result<CatalogueReadValue, CatalogueReadError> {
    trusted_scope(&admitted, &origin, &operation)?;
    let caller = caller(&admitted);
    let (mut source, captured_head) = match operation.scope() {
        Some(scope) => (
            NessaCatalogueSource::new(catalogue, caller, scope.clone()).map_err(worker_error)?,
            None,
        ),
        None => {
            let (receiver, epoch) =
                passive_read_selector(&admitted.receiver_id, admitted.access_epoch)
                    .map_err(CatalogueReadError::Admission)?;
            let (source, head) =
                NessaCatalogueSource::discover(catalogue, caller, receiver, origin, epoch)
                    .map_err(worker_error)?;
            (source, Some(head))
        }
    };
    let scope = source.scope().clone();
    let result = (|| match operation {
        CatalogueReadOperation::Head => Ok(CatalogueReadValue::Head {
            scope,
            head: captured_head.expect("head operation discovers its source"),
        }),
        CatalogueReadOperation::Manifest(request) => {
            let page = source.manifest(&request).map_err(source_error)?;
            Ok(CatalogueReadValue::Manifest(page))
        }
        CatalogueReadOperation::Resolve {
            pass,
            descriptor,
            max_payload_bytes,
        } => {
            let resolved = source
                .resolve(&pass, &descriptor.key.id, max_payload_bytes)
                .map_err(source_error)?;
            Ok(CatalogueReadValue::Resolve(resolved))
        }
    })();
    let drain = source.finish().map_err(worker_error);
    match (result, drain) {
        (Err(operation), Err(CatalogueReadError::WorkerPanicked)) => Err(
            CatalogueReadError::OperationAndWorkerPanicked(Box::new(operation)),
        ),
        (_, Err(error)) => Err(error),
        (result, Ok(())) => result,
    }
}

fn worker_error(error: CatalogueWorkerError) -> CatalogueReadError {
    match error {
        CatalogueWorkerError::Source(error) => source_error(error),
        CatalogueWorkerError::WorkerPanicked => CatalogueReadError::WorkerPanicked,
    }
}

fn source_error(error: CatalogueSourceError) -> CatalogueReadError {
    match error {
        CatalogueSourceError::Unavailable => CatalogueReadError::SourceUnavailable,
        CatalogueSourceError::IdentityChanged => CatalogueReadError::IdentityChanged,
        CatalogueSourceError::InvalidRequest => CatalogueReadError::InvalidRequest,
        CatalogueSourceError::OversizedEntry => CatalogueReadError::OversizedEntry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::application::{
        CatalogueHead, CataloguePage, CataloguePageRequest, CatalogueValue, ConversationError,
        ConversationFuture,
    };
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use nessa_protocol::conversation::domain::{conversation_catalogue_stream, ConversationId};
    use nessa_protocol::conversation::read_scope::ReadRefusal;
    use nessa_sync::replication::{
        catalogue::{CataloguePass, ManifestRequest},
        domain::Scope,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingCatalogue(AtomicUsize);
    impl ConversationCatalogue for CountingCatalogue {
        fn head(
            &self,
            _: &OrganizationId,
            _: &PrincipalId,
            _: &crate::conversation::application::Reader,
        ) -> ConversationFuture<'_, CatalogueHead> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(CatalogueHead {
                    incarnation: "incarnation".into(),
                    revision: 5,
                })
            })
        }
        fn page(&self, _: CataloguePageRequest) -> ConversationFuture<'_, CataloguePage> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn resolve(
            &self,
            _: &OrganizationId,
            _: &PrincipalId,
            _: &crate::conversation::application::Reader,
            _: &str,
            _: &ConversationId,
        ) -> ConversationFuture<'_, Option<CatalogueValue>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
    }
    fn admitted() -> CatalogueReadScope {
        CatalogueReadScope {
            receiver_id: "receiver".into(),
            organization_id: OrganizationId::new("org").unwrap(),
            owner_id: PrincipalId::new("owner").unwrap(),
            access_epoch: 7,
        }
    }
    fn id(value: &str) -> Id {
        Id::new(value).unwrap()
    }
    fn scope(parts: &[Id; 6]) -> Scope {
        Scope::new(
            parts[0].clone(),
            parts[1].clone(),
            parts[2].clone(),
            parts[3].clone(),
            parts[4].clone(),
            parts[5].clone(),
        )
    }

    #[test]
    fn foreign_trusted_identity_refuses_before_any_metadata_call() {
        let admitted = admitted();
        let origin = id("gateway");
        let catalogue = Arc::new(CountingCatalogue(AtomicUsize::new(0)));
        let expected = [
            id("receiver"),
            origin.clone(),
            conversation_catalogue_stream(&admitted.organization_id, &admitted.owner_id),
            id("incarnation"),
            conversation_catalogue_schema(),
            id("epoch-7"),
        ];
        for index in [0, 1, 2, 4, 5] {
            let mut parts = expected.clone();
            parts[index] = id("foreign");
            let operation = CatalogueReadOperation::Manifest(ManifestRequest {
                pass: CataloguePass {
                    scope: scope(&parts),
                    completed: 0,
                    boundary: 5,
                    cursor: None,
                    generation: 1,
                },
                max_entries: 1,
            });
            let expected_error = match index {
                0 => CatalogueReadError::Admission(ReadRefusal::WrongReceiver),
                2 => CatalogueReadError::Admission(ReadRefusal::WrongOwner),
                5 => CatalogueReadError::Admission(ReadRefusal::StaleEpoch),
                _ => CatalogueReadError::IdentityChanged,
            };
            assert!(
                matches!(execute(catalogue.clone(),admitted.clone(),origin.clone(),operation),Err(error) if error == expected_error)
            );
            assert_eq!(catalogue.0.load(Ordering::SeqCst), 0);
        }
        let valid = CatalogueReadOperation::Manifest(ManifestRequest {
            pass: CataloguePass {
                scope: scope(&expected),
                completed: 0,
                boundary: 5,
                cursor: None,
                generation: 1,
            },
            max_entries: 1,
        });
        assert!(matches!(
            execute(catalogue.clone(), admitted, origin, valid),
            Err(CatalogueReadError::SourceUnavailable)
        ));
        assert_eq!(catalogue.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn head_discovers_physical_identity_after_trusted_admission() {
        let catalogue = Arc::new(CountingCatalogue(AtomicUsize::new(0)));
        let admitted = admitted();
        let value = execute(
            catalogue.clone(),
            admitted.clone(),
            id("gateway"),
            CatalogueReadOperation::Head,
        )
        .unwrap();
        let CatalogueReadValue::Head { scope, head } = value else {
            panic!("head expected")
        };
        assert_eq!(head, 5);
        assert_eq!(scope.receiver().as_str(), admitted.receiver_id);
        assert_eq!(scope.incarnation().as_str(), "incarnation");
        assert_eq!(scope.access_epoch().as_str(), "epoch-7");
        assert_eq!(catalogue.0.load(Ordering::SeqCst), 1);
    }
}
