//! Authenticated owner catalogue transport with bounded source-through-send ownership.
//!
//! `nessa_protocol::product::catalogue_read` maps generated DTOs. `ReadCatalogue` owns admission ordering; source
//! workers own physical I/O and sync-engine validates finite passes and values.

use super::super::state::ProductRouteState;
use crate::conversation::application::{
    AdmitPassiveRead, CatalogueReadError, CatalogueReadOperation, CatalogueReadValue,
    ReadCatalogue, ReadRefusal, RecordReadLease,
};
use nessa_auth::application::{authorization::AuthorizeAction, session::AuthenticatedSession};
use nessa_protocol::product::catalogue_read;
use nessa_protocol::product::generated::{
    ConversationCatalogueHeadParams, ConversationCatalogueManifestParams,
    ConversationCatalogueResolveParams,
};
use nessa_protocol::product::passive_read::{decode_epoch, ReadEncodeError};
use nessa_protocol::product_contract::generated::CatalogueReadErrorCode;
use nessa_protocol::protocol::RequestFrame;

pub(in crate::product) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    lease: RecordReadLease,
) -> Result<(String, RecordReadLease), CatalogueReadErrorCode> {
    let operation = match frame.method.as_str() {
        "conversation.catalogueHead" => {
            let params: ConversationCatalogueHeadParams = serde_json::from_value(frame.params)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            (
                params.receiver_id,
                decode_epoch(&params.access_epoch)
                    .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?,
                CatalogueReadOperation::Head,
            )
        }
        "conversation.catalogueManifest" => {
            let params: ConversationCatalogueManifestParams = serde_json::from_value(frame.params)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            let request = catalogue_read::decode_manifest(&params.request)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            let epoch = decode_epoch(&params.access_epoch)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            (
                request.pass.scope.receiver().as_str().to_owned(),
                epoch,
                CatalogueReadOperation::Manifest(request),
            )
        }
        "conversation.catalogueResolve" => {
            let params: ConversationCatalogueResolveParams =
                serde_json::from_value(frame.params)
                    .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            let pass = catalogue_read::decode_pass(&params.pass)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            let epoch = decode_epoch(&params.access_epoch)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            let descriptor = catalogue_read::decode_descriptor(&params.descriptor)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            let max_payload_bytes = usize::try_from(params.max_payload_bytes)
                .map_err(|_| CatalogueReadErrorCode::InvalidRequest)?;
            (
                pass.scope.receiver().as_str().to_owned(),
                epoch,
                CatalogueReadOperation::Resolve {
                    pass,
                    descriptor,
                    max_payload_bytes,
                },
            )
        }
        _ => return Err(CatalogueReadErrorCode::InvalidRequest),
    };
    let (receiver, epoch, operation) = operation;
    let requested = operation.clone();
    let (receivers, repository) = state
        .passive_read
        .as_ref()
        .ok_or(CatalogueReadErrorCode::SourceUnavailable)?;
    let source = state
        .catalogue_source
        .as_ref()
        .ok_or(CatalogueReadErrorCode::SourceUnavailable)?;
    let use_case = ReadCatalogue {
        admission: AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: state.access.as_ref(),
                clock: state.clock.as_ref(),
                policy: state.policy.as_ref(),
            },
            gateway: &state.gateway,
            receivers: receivers.as_ref(),
            conversations: repository.as_ref(),
        },
        source: source.as_ref(),
    };
    let response = use_case
        .execute(session, &receiver, epoch, operation, lease)
        .await
        .map_err(error_code)?;
    let encoded = encode_value(&frame.id, requested, response.value)?;
    Ok((encoded, response.lease))
}

pub(super) fn encode_value(
    request_id: &str,
    requested: CatalogueReadOperation,
    value: CatalogueReadValue,
) -> Result<String, CatalogueReadErrorCode> {
    match (requested, value) {
        (CatalogueReadOperation::Head, CatalogueReadValue::Head { scope, head }) => {
            catalogue_read::encode_head(request_id, &scope, head)
        }
        (CatalogueReadOperation::Manifest(request), CatalogueReadValue::Manifest(page)) => {
            catalogue_read::encode_manifest(request_id, &request, page)
        }
        (
            CatalogueReadOperation::Resolve {
                pass,
                descriptor,
                max_payload_bytes,
            },
            CatalogueReadValue::Resolve(resolved),
        ) => catalogue_read::encode_resolved(
            request_id,
            &pass,
            &descriptor,
            max_payload_bytes,
            resolved,
        ),
        _ => return Err(CatalogueReadErrorCode::Unverifiable),
    }
    .map_err(|error| match error {
        ReadEncodeError::ResponseTooLarge => CatalogueReadErrorCode::ResponseTooLarge,
        ReadEncodeError::InvalidPayload => CatalogueReadErrorCode::Unverifiable,
    })
}

fn error_code(error: CatalogueReadError) -> CatalogueReadErrorCode {
    match error {
        CatalogueReadError::Admission(refusal) => match refusal {
            ReadRefusal::InvalidRequest => CatalogueReadErrorCode::InvalidRequest,
            ReadRefusal::Unauthorized => CatalogueReadErrorCode::Unauthorized,
            ReadRefusal::Forbidden => CatalogueReadErrorCode::Forbidden,
            ReadRefusal::WrongOwner => CatalogueReadErrorCode::WrongOwner,
            ReadRefusal::WrongReceiver => CatalogueReadErrorCode::WrongReceiver,
            ReadRefusal::StaleEpoch => CatalogueReadErrorCode::StaleEpoch,
            ReadRefusal::Unverifiable => CatalogueReadErrorCode::Unverifiable,
        },
        CatalogueReadError::InvalidRequest => CatalogueReadErrorCode::InvalidRequest,
        CatalogueReadError::IdentityChanged => CatalogueReadErrorCode::IdentityChanged,
        CatalogueReadError::SourceUnavailable
        | CatalogueReadError::WorkerPanicked
        | CatalogueReadError::OperationAndWorkerPanicked(_) => {
            CatalogueReadErrorCode::SourceUnavailable
        }
        CatalogueReadError::OversizedEntry => CatalogueReadErrorCode::OversizedEntry,
    }
}

#[cfg(test)]
#[path = "../../../tests/product/catalogue_read/dispatch.rs"]
mod tests;
