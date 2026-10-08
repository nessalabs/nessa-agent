//! Bounded product wire conversion for authorized physical record reads.
//!
//! `nessa_protocol::product::record_read` decodes generated request DTOs, asks sync-engine's public validator
//! about a source page, and writes the response through a capped JSON writer.
//! It carries no credential, ownership, or stream-incarnation decision.

use super::super::passive_read::PUBLISHED_PASSIVE_READ_GRANTS;
use super::super::state::ProductRouteState;
use crate::conversation::application::{
    AdmitPassiveRead, ReadRecords, RecordReadError, RecordReadLease, RecordReadOperation,
    RecordReadValue,
};
use nessa_auth::application::{authorization::AuthorizeAction, session::AuthenticatedSession};
use nessa_protocol::conversation::{domain::ConversationId, read_scope::ReadRefusal};
use nessa_protocol::product::passive_read::{decode_epoch, ReadEncodeError, ReadWireError};
use nessa_protocol::product::record_read;

use nessa_protocol::product::generated::{
    ConversationRecordsHeadParams, ConversationRecordsPageParams,
};
use nessa_protocol::product_contract::generated::RecordReadErrorCode;
use nessa_protocol::protocol::RequestFrame;

/// Fresh admission, exact identity, one bounded source operation, then capped
/// wire encoding. On timeout the source thread retains its global permit.
pub(crate) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    lease: RecordReadLease,
) -> Result<(String, RecordReadLease), RecordReadErrorCode> {
    let request_id = frame.id;
    let params = match frame.method.as_str() {
        "conversation.recordsHead" => {
            let params: ConversationRecordsHeadParams = serde_json::from_value(frame.params)
                .map_err(|_| RecordReadErrorCode::InvalidRequest)?;
            let epoch = decode_epoch(&params.access_epoch)
                .map_err(|_| RecordReadErrorCode::InvalidRequest)?;
            (
                params.conversation_id,
                params.receiver_id,
                epoch,
                RecordReadOperation::Head,
            )
        }
        "conversation.recordsPage" => {
            let params: ConversationRecordsPageParams = serde_json::from_value(frame.params)
                .map_err(|_| RecordReadErrorCode::InvalidRequest)?;
            let page = record_read::decode_page_request(&params.request)
                .map_err(|_| RecordReadErrorCode::InvalidRequest)?;
            let receiver_id = page.scope.receiver().as_str().to_owned();
            let epoch = decode_epoch(&params.access_epoch)
                .map_err(|_| RecordReadErrorCode::InvalidRequest)?;
            (
                params.conversation_id,
                receiver_id,
                epoch,
                RecordReadOperation::Page(page),
            )
        }
        _ => return Err(RecordReadErrorCode::InvalidRequest),
    };
    let (conversation_id, receiver_id, epoch, operation) = params;
    let expected_page = match &operation {
        RecordReadOperation::Head => None,
        RecordReadOperation::Page(request) => Some(request.clone()),
    };
    let conversation =
        ConversationId::new(&conversation_id).map_err(|_| RecordReadErrorCode::InvalidRequest)?;
    let (receivers, repository) = state
        .passive_read
        .as_ref()
        .ok_or(RecordReadErrorCode::TemporarilyUnavailable)?;
    let source = state
        .record_source
        .as_ref()
        .ok_or(RecordReadErrorCode::TemporarilyUnavailable)?;
    let use_case = ReadRecords {
        admission: AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: state.access.as_ref(),
                clock: state.clock.as_ref(),
                policy: state.policy.as_ref(),
            },
            gateway: &state.gateway,
            receivers: receivers.as_ref(),
            conversations: repository.as_ref(),
            grants: &PUBLISHED_PASSIVE_READ_GRANTS,
        },
        source: source.as_ref(),
    };
    let response = use_case
        .execute(
            session,
            &conversation,
            &receiver_id,
            epoch,
            operation,
            lease,
        )
        .await
        .map_err(error_code)?;
    let text = match response.value {
        RecordReadValue::Head(head) => {
            if expected_page.is_some() {
                return Err(RecordReadErrorCode::Unverifiable);
            }
            record_read::encode_head(&request_id, &head.scope, head.head)
                .map_err(encode_error_code)?
        }
        RecordReadValue::Page(page) => {
            let request = expected_page.ok_or(RecordReadErrorCode::Unverifiable)?;
            record_read::encode_page(&request_id, &request, page).map_err(wire_error_code)?
        }
    };
    Ok((text, response.lease))
}

fn error_code(error: RecordReadError) -> RecordReadErrorCode {
    match error {
        RecordReadError::Admission(refusal) => refusal_code(refusal),
        RecordReadError::InvalidRequest => RecordReadErrorCode::InvalidRequest,
        RecordReadError::IdentityChanged => RecordReadErrorCode::IdentityChanged,
        RecordReadError::HistoryPruned => RecordReadErrorCode::HistoryPruned,
        RecordReadError::RecordTooLarge => RecordReadErrorCode::RecordTooLarge,
        RecordReadError::TemporarilyUnavailable | RecordReadError::WorkerPanicked => {
            RecordReadErrorCode::TemporarilyUnavailable
        }
        RecordReadError::SourcePreparing => RecordReadErrorCode::SourcePreparing,
        RecordReadError::ReadTimeout => RecordReadErrorCode::ReadTimeout,
    }
}

fn encode_error_code(error: ReadEncodeError) -> RecordReadErrorCode {
    match error {
        ReadEncodeError::ResponseTooLarge => RecordReadErrorCode::ResponseTooLarge,
        ReadEncodeError::InvalidPayload => RecordReadErrorCode::Unverifiable,
    }
}

fn wire_error_code(error: ReadWireError) -> RecordReadErrorCode {
    match error {
        ReadWireError::InvalidRequest => RecordReadErrorCode::InvalidRequest,
        ReadWireError::InvalidPage => RecordReadErrorCode::Unverifiable,
        ReadWireError::ResponseTooLarge => RecordReadErrorCode::ResponseTooLarge,
    }
}

/// The wire code a refused record read answers with. Record dispatch and
/// socket admission both answer through it.
pub(in crate::product) fn refusal_code(refusal: ReadRefusal) -> RecordReadErrorCode {
    match refusal {
        ReadRefusal::InvalidRequest => RecordReadErrorCode::InvalidRequest,
        ReadRefusal::Unauthorized => RecordReadErrorCode::Unauthorized,
        ReadRefusal::Forbidden => RecordReadErrorCode::Forbidden,
        ReadRefusal::WrongOwner => RecordReadErrorCode::WrongOwner,
        ReadRefusal::WrongReceiver => RecordReadErrorCode::WrongReceiver,
        ReadRefusal::StaleEpoch => RecordReadErrorCode::StaleEpoch,
        ReadRefusal::Unverifiable => RecordReadErrorCode::Unverifiable,
    }
}
