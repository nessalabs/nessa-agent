//! Current original-purpose facts and Cedar admission for product requests.
//! Classification is mandatory even without a native listener or read source.
use super::state::ProductRouteState;
use crate::conversation::application::{
    AdmitOwnerPhoneReader, OwnerPhoneOperation, OwnerPhoneReader,
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::{
    conversation::read_scope::ReadRefusal, product::generated::action_for_method,
};

/// One shared application factory owns classification and current original holding.
pub(super) async fn current(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<Option<OwnerPhoneReader>, &'static str> {
    AdmitOwnerPhoneReader {
        purposes: state.owner_phone_purpose.as_ref(),
        holding: state.owner_phone_receivers.as_deref(),
        clock: state.clock.as_ref(),
        gateway: &state.gateway,
        policy: state.owner_phone_policy.clone(),
        access_policy: state.conversation_access.clone(),
    }
    .execute(session)
    .await
    .map_err(refusal_code)
}

fn refusal_code(refusal: ReadRefusal) -> &'static str {
    match refusal {
        ReadRefusal::Unverifiable => "temporarily_unavailable",
        ReadRefusal::Unauthorized => "unauthorized",
        _ => "forbidden",
    }
}

/// Cedar receives the published method classification and canonical facts.
/// This decision does not replace coarse grants or authoritative target checks.
pub(super) async fn admit_method(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    method: &str,
) -> Result<Option<OwnerPhoneReader>, &'static str> {
    admit_operation(
        state,
        session,
        OwnerPhoneOperation::ProductMethod {
            method,
            published_action: action_for_method(method),
        },
    )
    .await
}

pub(super) async fn admit_creation(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    requested_mode: Option<&str>,
    default_mode: &str,
) -> Result<(), &'static str> {
    admit_operation(
        state,
        session,
        OwnerPhoneOperation::Creation {
            requested_mode,
            default_mode,
        },
    )
    .await
    .map(|_| ())
}

pub(super) async fn admit_submission_input(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    linked_files: usize,
) -> Result<(), &'static str> {
    admit_operation(
        state,
        session,
        OwnerPhoneOperation::SubmissionInput { linked_files },
    )
    .await
    .map(|_| ())
}

async fn admit_operation(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    operation: OwnerPhoneOperation<'_>,
) -> Result<Option<OwnerPhoneReader>, &'static str> {
    let Some(phone) = current(state, session).await? else {
        return Ok(None);
    };
    phone.admit_operation(operation).map_err(refusal_code)?;
    Ok(Some(phone))
}
