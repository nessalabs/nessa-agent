//! Test-support-only bridge from one proved native device to a binary Head.
use super::dispatch::use_case;
use crate::conversation::application::{
    PassiveRead, RecordHead, RecordReadError, RecordReadLease, RecordReadOperation, RecordReadValue,
};
use crate::product::state::ProductRouteState;
use nessa_auth::{
    adapters::pairing::NativeTransport,
    application::{
        ports::{AccessError, CredentialEvidence, CredentialVerifier},
        session::{AuthenticateSession, AuthenticatedSession, ReadCurrentSession},
    },
};
use nessa_protocol::{
    conversation::{domain::ConversationId, read_scope::ReadRefusal},
    pairing::socket::DeadlineStream,
    product::{generated::RecordScope, passive_read::decode_scope},
};
use nessa_sdk::infrastructure::session_storage::physical_record_schema;
use nessa_sync::replication::{
    application::{Access, RecordReadView, RecordSource, ScopeAuthorizer, SourceError},
    domain::{Page, PageRequest, Scope, SourceHead},
    infrastructure::session::{
        serve_operation_capability_record_session, RecordOperation, RecordOperationAuthorizer,
        SessionLimits,
    },
};
use serde::Deserialize;
use std::{cell::Cell, io::ErrorKind, time::Duration};
use tokio::runtime::Handle;

const AUTH_BUDGET: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Opening {
    credential: String,
    receiver: String,
    epoch: u64,
    scope: RecordScope,
}

pub(in crate::product) fn serve(
    runtime: &Handle,
    state: &ProductRouteState,
    verifier: &dyn CredentialVerifier,
    mut transport: NativeTransport<DeadlineStream>,
    opening: &[u8],
) -> Result<(), ErrorKind> {
    let opening: Opening = serde_json::from_slice(opening).map_err(|_| ErrorKind::InvalidData)?;
    let evidence = CredentialEvidence::new(opening.credential.into_bytes())
        .map_err(|_| ErrorKind::PermissionDenied)?;
    let session = runtime
        .block_on(async {
            tokio::time::timeout(
                AUTH_BUDGET,
                AuthenticateSession {
                    policy: state.policy.as_ref(),
                    verifier,
                    access: state.access.as_ref(),
                    clock: state.clock.as_ref(),
                }
                .execute(&evidence, &state.audience),
            )
            .await
        })
        .map_err(|_| ErrorKind::TimedOut)?
        .map_err(|_| ErrorKind::PermissionDenied)?;
    let scope = decode_scope(&opening.scope).map_err(|_| ErrorKind::InvalidData)?;
    let conversation =
        ConversationId::new(scope.stream().as_str()).map_err(|_| ErrorKind::InvalidData)?;
    let taken = Cell::new(false);
    let probe = Probe {
        runtime,
        state,
        session: &session,
        conversation: &conversation,
        receiver: &opening.receiver,
        epoch: opening.epoch,
        scope: &scope,
        taken: &taken,
    };
    // The bounded Bind already checks Head policy before the physical read
    // and before release. Its source matches the selector to the physical
    // identity; the selector is not itself an origin assertion.
    let mut scope_policy = probe;
    let mut operations = probe;
    serve_operation_capability_record_session(
        &mut transport,
        1,
        SessionLimits {
            control_bytes: 2048,
            bindings: 1,
        },
        &mut scope_policy,
        &mut operations,
        |_| Ok(probe),
    )
    .map_err(|_| ErrorKind::InvalidData)
}

#[derive(Clone, Copy)]
struct Probe<'a> {
    runtime: &'a Handle,
    state: &'a ProductRouteState,
    session: &'a AuthenticatedSession,
    conversation: &'a ConversationId,
    receiver: &'a str,
    epoch: u64,
    scope: &'a Scope,
    taken: &'a Cell<bool>,
}

impl Probe<'_> {
    fn read(&self) -> Result<RecordHead, SourceError> {
        let permit = self
            .state
            .admission
            .record_reads()
            .clone()
            .try_acquire_owned()
            .map_err(|_| SourceError::Unavailable)?;
        let use_case = use_case(self.state).map_err(|_| SourceError::Unavailable)?;
        let result = self
            .runtime
            .block_on(async {
                tokio::time::timeout(
                    AUTH_BUDGET,
                    use_case.execute(
                        self.session,
                        self.conversation,
                        self.receiver,
                        self.epoch,
                        RecordReadOperation::Head,
                        RecordReadLease::new(permit),
                    ),
                )
                .await
            })
            .map_err(|_| SourceError::Unavailable)?
            .map_err(read_error)?;
        match result.value {
            RecordReadValue::Head(head) if head.scope == *self.scope => Ok(head),
            _ => Err(SourceError::IdentityChanged),
        }
    }
}

impl ScopeAuthorizer for Probe<'_> {
    fn authorize(&mut self, scope: &Scope) -> Access {
        if scope != self.scope {
            return Access::IdentityChanged;
        }
        match self.runtime.block_on(async {
            tokio::time::timeout(
                AUTH_BUDGET,
                ReadCurrentSession {
                    access: self.state.access.as_ref(),
                    clock: self.state.clock.as_ref(),
                }
                .execute(self.session),
            )
            .await
        }) {
            Ok(Ok(_)) => Access::Allowed(scope.clone()),
            Ok(Err(AccessError::CredentialRevoked | AccessError::CredentialExpired)) => {
                Access::Denied
            }
            _ => Access::Unverifiable,
        }
    }
}

impl RecordOperationAuthorizer for Probe<'_> {
    fn authorize_operation(&mut self, scope: &Scope, operation: RecordOperation) -> Access {
        if scope != self.scope {
            return Access::IdentityChanged;
        }
        match operation {
            // #121 asks Head independently for binding as well as HEAD_CAP.
            RecordOperation::Bind => return Access::Allowed(scope.clone()),
            RecordOperation::Head => {}
            RecordOperation::Authorize | RecordOperation::Page => return Access::Denied,
        }
        let Ok(use_case) = use_case(self.state) else {
            return Access::Unverifiable;
        };
        match self.runtime.block_on(async {
            tokio::time::timeout(AUTH_BUDGET, async {
                crate::product::owner_phone::admit_method(
                    self.state,
                    self.session,
                    nessa_protocol::product::generated::product_method::CONVERSATION_RECORDS_HEAD,
                )
                .await
                .map_err(|_| ReadRefusal::Forbidden)?;
                use_case
                    .admission
                    .execute(
                        self.session,
                        self.conversation,
                        self.receiver,
                        self.epoch,
                        PassiveRead::RecordHead,
                    )
                    .await
            })
            .await
        }) {
            Ok(Ok(_)) => Access::Allowed(scope.clone()),
            Ok(Err(error)) => access_refused(error),
            Err(_) => Access::Unverifiable,
        }
    }
}

impl RecordSource for Probe<'_> {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        if scope != self.scope {
            return Err(SourceError::IdentityChanged);
        }
        Ok(self.read()?.head)
    }

    fn page(&mut self, _: &PageRequest) -> Result<Page, SourceError> {
        Err(SourceError::Denied)
    }
}

impl RecordReadView for Probe<'_> {
    fn view_head(&mut self, scope: &Scope) -> Result<SourceHead, SourceError> {
        // Only HEAD_CAP uses this allowance. Bind's advisory head is separate;
        // the original finite native worker deadline still owns transport waits.
        if self.taken.replace(true) {
            return Err(SourceError::Denied);
        }
        let position = self.head(scope)?;
        // The lab accepts only the Nessa physical immutable schema. Its HEAD
        // asks for no extra Sync Engine reader features and exposes no writer.
        if self.scope.schema() != &physical_record_schema() {
            return Err(SourceError::IdentityChanged);
        }
        Ok(SourceHead::new(scope.clone(), position, 0, 0))
    }
}

fn access_refused(error: ReadRefusal) -> Access {
    match error {
        ReadRefusal::StaleEpoch | ReadRefusal::WrongReceiver => Access::IdentityChanged,
        ReadRefusal::Unauthorized | ReadRefusal::Forbidden | ReadRefusal::WrongOwner => {
            Access::Denied
        }
        ReadRefusal::InvalidRequest | ReadRefusal::Unverifiable => Access::Unverifiable,
    }
}

fn read_error(error: RecordReadError) -> SourceError {
    match error {
        RecordReadError::Admission(reason) => match access_refused(reason) {
            Access::IdentityChanged => SourceError::IdentityChanged,
            Access::Denied => SourceError::Denied,
            _ => SourceError::Unavailable,
        },
        RecordReadError::Incompatible(error) => SourceError::Incompatible(error),
        RecordReadError::InvalidRequest => SourceError::InvalidRequest,
        RecordReadError::IdentityChanged => SourceError::IdentityChanged,
        RecordReadError::HistoryPruned => SourceError::Pruned,
        RecordReadError::RecordTooLarge => SourceError::OversizedRecord,
        RecordReadError::TemporarilyUnavailable
        | RecordReadError::WorkerPanicked
        | RecordReadError::ReadTimeout
        | RecordReadError::SourcePreparing => SourceError::Unavailable,
    }
}
