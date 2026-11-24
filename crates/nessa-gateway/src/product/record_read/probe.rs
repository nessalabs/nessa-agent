//! Test-support-only bridge from one proved native device to bounded record reads.
use super::dispatch::use_case;
use crate::conversation::application::{
    PassiveRead, RecordReadError, RecordReadLease, RecordReadOperation, RecordReadValue,
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
    product::{
        generated::{
            product_method::{CONVERSATION_RECORDS_HEAD, CONVERSATION_RECORDS_PAGE},
            RecordScope, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES,
            MAX_RECORD_PAGE_RECORDS,
        },
        passive_read::decode_scope,
    },
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
use std::{cell::Cell, io::ErrorKind, sync::Arc, time::Duration};
use tokio::{runtime::Handle, sync::OwnedSemaphorePermit};

const AUTH_BUDGET: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Opening {
    credential: String,
    receiver: String,
    epoch: u64,
    scope: RecordScope,
    #[serde(default)]
    page: bool,
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
    let uncertain = Cell::new(false);
    let probe = Probe {
        runtime,
        state,
        session: &session,
        conversation: &conversation,
        receiver: &opening.receiver,
        epoch: opening.epoch,
        scope: &scope,
        taken: &taken,
        page: opening.page,
        uncertain: &uncertain,
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
        |_| {
            Ok(View {
                probe,
                permit: None,
            })
        },
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
    page: bool,
    uncertain: &'a Cell<bool>,
}

// The binary codec keeps this view through bounded encoding, then drops it
// before beginning a response write (the existing Product R64 handoff).
struct View<'a> {
    probe: Probe<'a>,
    permit: Option<Arc<OwnedSemaphorePermit>>,
}

impl View<'_> {
    fn read(&mut self, operation: RecordReadOperation) -> Result<RecordReadValue, SourceError> {
        let probe = self.probe;
        if probe.uncertain.get() {
            return Err(SourceError::Unavailable);
        }
        if self.permit.is_none() {
            self.permit = Some(Arc::new(
                probe
                    .state
                    .admission
                    .record_reads()
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| SourceError::Unavailable)?,
            ));
        }
        let permit = self
            .permit
            .as_ref()
            .expect("view permit was acquired")
            .clone();
        let use_case = use_case(probe.state).map_err(|_| SourceError::Unavailable)?;
        let result = probe.runtime.block_on(async {
            tokio::time::timeout(
                AUTH_BUDGET,
                use_case.execute(
                    probe.session,
                    probe.conversation,
                    probe.receiver,
                    probe.epoch,
                    operation,
                    RecordReadLease::new(permit),
                ),
            )
            .await
        });
        match result {
            Ok(Ok(result)) => Ok(result.value),
            Ok(Err(error)) => Err(read_error(error)),
            Err(_) => {
                // The worker can still own its lease. Do not admit a new read
                // on this connection even if global capacity remains.
                probe.uncertain.set(true);
                Err(SourceError::Unavailable)
            }
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
        if self.uncertain.get() {
            return Access::Unverifiable;
        }
        let (method, read) = match operation {
            // #121 asks Head independently for binding as well as HEAD_CAP.
            RecordOperation::Bind => return Access::Allowed(scope.clone()),
            RecordOperation::Head => (CONVERSATION_RECORDS_HEAD, PassiveRead::RecordHead),
            RecordOperation::Authorize | RecordOperation::Page if self.page => {
                (CONVERSATION_RECORDS_PAGE, PassiveRead::RecordPage)
            }
            RecordOperation::Authorize | RecordOperation::Page => return Access::Denied,
        };
        let Ok(use_case) = use_case(self.state) else {
            return Access::Unverifiable;
        };
        match self.runtime.block_on(async {
            tokio::time::timeout(AUTH_BUDGET, async {
                crate::product::owner_phone::admit_method(self.state, self.session, method)
                    .await
                    .map_err(|_| ReadRefusal::Forbidden)?;
                use_case
                    .admission
                    .execute(
                        self.session,
                        self.conversation,
                        self.receiver,
                        self.epoch,
                        read,
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

impl RecordSource for View<'_> {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        if scope != self.probe.scope {
            return Err(SourceError::IdentityChanged);
        }
        match self.read(RecordReadOperation::Head)? {
            RecordReadValue::Head(head) if head.scope == *scope => Ok(head.head),
            _ => Err(SourceError::IdentityChanged),
        }
    }

    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        if !self.probe.page || self.probe.taken.replace(true) {
            return Err(SourceError::Denied);
        }
        if request.scope != *self.probe.scope {
            return Err(SourceError::IdentityChanged);
        }
        if request.max_records > MAX_RECORD_PAGE_RECORDS
            || request.max_payload_bytes > MAX_RECORD_PAGE_PAYLOAD_BYTES
            || request.max_record_bytes > MAX_PHYSICAL_RECORD_PAYLOAD_BYTES
        {
            return Err(SourceError::InvalidRequest);
        }
        match self.read(RecordReadOperation::Page(request.clone()))? {
            RecordReadValue::Page(page) if page.request == *request => Ok(page),
            _ => Err(SourceError::IdentityChanged),
        }
    }
}

impl RecordReadView for View<'_> {
    fn view_head(&mut self, scope: &Scope) -> Result<SourceHead, SourceError> {
        // Head-only keeps its one-HEAD allowance. Page mode spends its one
        // allowance on Page, whose capability response also reads Head.
        if !self.probe.page && self.probe.taken.replace(true) {
            return Err(SourceError::Denied);
        }
        if self.probe.scope.schema() != &physical_record_schema() {
            return Err(SourceError::IdentityChanged);
        }
        Ok(SourceHead::new(scope.clone(), self.head(scope)?, 0, 0))
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
