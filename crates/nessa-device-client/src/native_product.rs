//! Immutable foreign views over the single bounded physical product owner.
use crate::{
    serialization::{bounded_json, SerializationFailure},
    session::SessionProduct,
};
use nessa_client_core::product::{
    ProductCompletion, ProductEventBatch, ProductFailure, ProductSettlement, ProductTicket,
};
use nessa_protocol::{
    pairing::MAX_PROTECTED_RESPONSE_BYTES, product::generated::ProductSessionReady,
    product_contract::generated::SessionCloseReason,
};
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FormatResult},
    sync::Arc,
};

/// Original terminal close reason from the protocol owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeProductCloseReason {
    /// Same-named protocol reason.
    AuthenticationFailed,
    /// Same-named protocol reason.
    CredentialRevoked,
    /// Same-named protocol reason.
    CredentialExpired,
    /// Same-named protocol reason.
    AuthorizationLost,
    /// Same-named protocol reason.
    ProtocolIncompatible,
    /// Same-named protocol reason.
    HandshakeTimeout,
    /// Same-named protocol reason.
    TemporaryUnavailable,
    /// Same-named protocol reason.
    GatewayRestarting,
    /// Same-named protocol reason.
    GatewayOverloaded,
    /// Same-named protocol reason.
    ServerShutdown,
    /// Same-named protocol reason.
    TransportInterrupted,
}
impl From<SessionCloseReason> for NativeProductCloseReason {
    fn from(value: SessionCloseReason) -> Self {
        match value {
            SessionCloseReason::AuthenticationFailed => Self::AuthenticationFailed,
            SessionCloseReason::CredentialRevoked => Self::CredentialRevoked,
            SessionCloseReason::CredentialExpired => Self::CredentialExpired,
            SessionCloseReason::AuthorizationLost => Self::AuthorizationLost,
            SessionCloseReason::ProtocolIncompatible => Self::ProtocolIncompatible,
            SessionCloseReason::HandshakeTimeout => Self::HandshakeTimeout,
            SessionCloseReason::TemporaryUnavailable => Self::TemporaryUnavailable,
            SessionCloseReason::GatewayRestarting => Self::GatewayRestarting,
            SessionCloseReason::GatewayOverloaded => Self::GatewayOverloaded,
            SessionCloseReason::ServerShutdown => Self::ServerShutdown,
            SessionCloseReason::TransportInterrupted => Self::TransportInterrupted,
        }
    }
}
/// Original local product failure variant, distinct from a gateway Response.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeProductFailureKind {
    /// Same-named core failure.
    InvalidLimits,
    /// Same-named core failure.
    InvalidRequest,
    /// Same-named core failure.
    UnadvertisedMethod,
    /// Same-named core failure.
    Capacity,
    /// Same-named core failure.
    RequestTooLarge,
    /// Same-named core failure.
    ResponseTooLarge,
    /// Same-named core failure.
    TimedOut,
    /// Same-named core failure.
    Cancelled,
    /// Same-named core failure.
    Transport,
    /// Original optional close reason is retained separately.
    Closed,
    /// Same-named core failure.
    NativeHandshake,
    /// Same-named core failure.
    ProductRefused,
    /// Original close reason is retained separately.
    Authentication,
    /// Same-named core failure.
    Protocol,
    /// Same-named core failure.
    Correlation,
    /// Same-named core failure.
    WorkerPanicked,
    /// Same-named core failure.
    NotReady,
}
/// Immutable original failure with its optional typed protocol payload.
#[derive(Debug, uniffi::Object)]
pub struct NativeProductFailure {
    pub(crate) value: ProductFailure,
}
#[uniffi::export]
impl NativeProductFailure {
    /// Original local failure family.
    pub fn kind(&self) -> NativeProductFailureKind {
        match self.value {
            ProductFailure::InvalidLimits => NativeProductFailureKind::InvalidLimits,
            ProductFailure::InvalidRequest => NativeProductFailureKind::InvalidRequest,
            ProductFailure::UnadvertisedMethod => NativeProductFailureKind::UnadvertisedMethod,
            ProductFailure::Capacity => NativeProductFailureKind::Capacity,
            ProductFailure::RequestTooLarge => NativeProductFailureKind::RequestTooLarge,
            ProductFailure::ResponseTooLarge => NativeProductFailureKind::ResponseTooLarge,
            ProductFailure::TimedOut => NativeProductFailureKind::TimedOut,
            ProductFailure::Cancelled => NativeProductFailureKind::Cancelled,
            ProductFailure::Transport => NativeProductFailureKind::Transport,
            ProductFailure::Closed(_) => NativeProductFailureKind::Closed,
            ProductFailure::NativeHandshake => NativeProductFailureKind::NativeHandshake,
            ProductFailure::ProductRefused => NativeProductFailureKind::ProductRefused,
            ProductFailure::Authentication(_) => NativeProductFailureKind::Authentication,
            ProductFailure::Protocol => NativeProductFailureKind::Protocol,
            ProductFailure::Correlation => NativeProductFailureKind::Correlation,
            ProductFailure::WorkerPanicked => NativeProductFailureKind::WorkerPanicked,
            ProductFailure::NotReady => NativeProductFailureKind::NotReady,
        }
    }
    /// Exact reason when the original failure carried one; absent remains absent.
    pub fn close_reason(&self) -> Option<NativeProductCloseReason> {
        match self.value {
            ProductFailure::Closed(reason) => reason.map(Into::into),
            ProductFailure::Authentication(reason) => Some(reason.into()),
            _ => None,
        }
    }
}
/// Foreign product call refusal, carrying its original immutable typed cause.
#[derive(Debug, uniffi::Error)]
pub enum NativeProductCallError {
    /// Original product failure.
    Product {
        /// Retained typed cause; a gateway error Response is not this variant.
        failure: Arc<NativeProductFailure>,
    },
}
impl From<ProductFailure> for NativeProductCallError {
    fn from(value: ProductFailure) -> Self {
        Self::Product {
            failure: Arc::new(NativeProductFailure { value }),
        }
    }
}
impl Display for NativeProductCallError {
    fn fmt(&self, output: &mut Formatter<'_>) -> FormatResult {
        write!(output, "{self:?}")
    }
}
impl Error for NativeProductCallError {}
fn json<T: serde::Serialize>(value: &T) -> Result<String, NativeProductCallError> {
    bounded_json(value, MAX_PROTECTED_RESPONSE_BYTES).map_err(|failure| match failure {
        SerializationFailure::TooLarge => ProductFailure::ResponseTooLarge.into(),
        SerializationFailure::Invalid => ProductFailure::Protocol.into(),
    })
}
/// Actual immutable gateway Ready; its retained facts do not reopen admission.
#[derive(uniffi::Object)]
pub struct NativeProductReady {
    value: Arc<ProductSessionReady>,
}
#[uniffi::export]
impl NativeProductReady {
    /// Canonical full parsed Ready metadata, including original nullable expiry/grants.
    pub fn json(&self) -> Result<String, NativeProductCallError> {
        json(self.value.as_ref())
    }
    /// Actual protocol version.
    pub fn version(&self) -> u64 {
        self.value.version
    }
    /// Original public issued credential identity, not a bearer.
    pub fn credential_id(&self) -> String {
        self.value.credential_id.clone()
    }
    /// Actual advertised methods; no local policy list is substituted.
    pub fn methods(&self) -> Vec<String> {
        self.value.methods.clone()
    }
}
/// Physical dispatch stage retained independently of success/error response.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeProductCompletionKind {
    /// Original correlated success or gateway error frame.
    Response,
    /// The worker proved dispatch was not recorded.
    NotDispatched,
    /// Dispatch was recorded; no correlated result was captured.
    Uncertain,
}
/// Original settlement retaining the native ticket permit until its final handle.
#[derive(uniffi::Object)]
pub struct NativeProductCompletion {
    value: ProductSettlement,
}
#[uniffi::export]
impl NativeProductCompletion {
    /// Original native wire ID; never a new foreign correlation ID.
    pub fn id(&self) -> u64 {
        self.value.id()
    }
    /// Original dispatch/result meaning.
    pub fn kind(&self) -> NativeProductCompletionKind {
        match self.value.value() {
            ProductCompletion::Response(_) => NativeProductCompletionKind::Response,
            ProductCompletion::NotDispatched(_) => NativeProductCompletionKind::NotDispatched,
            ProductCompletion::Uncertain(_) => NativeProductCompletionKind::Uncertain,
        }
    }
    /// Original failure cause for unsent/uncertain variants.
    pub fn failure(&self) -> Option<Arc<NativeProductFailure>> {
        match self.value.value() {
            ProductCompletion::NotDispatched(value) | ProductCompletion::Uncertain(value) => {
                Some(Arc::new(NativeProductFailure { value: *value }))
            }
            ProductCompletion::Response(_) => None,
        }
    }
    /// Full canonical response including original payload/error/details/null meaning.
    pub fn response_json(&self) -> Result<Option<String>, NativeProductCallError> {
        match self.value.value() {
            ProductCompletion::Response(value) => json(value).map(Some),
            _ => Ok(None),
        }
    }
}
/// Original admitted ticket; dropping an observer does not cancel its worker.
#[derive(uniffi::Object)]
pub struct NativeProductTicket {
    value: ProductTicket,
}
#[uniffi::export]
impl NativeProductTicket {
    /// Native monotonic original wire ID.
    pub fn id(&self) -> u64 {
        self.value.id()
    }
    /// Original settlement if already captured.
    pub fn completion(&self) -> Option<Arc<NativeProductCompletion>> {
        self.value
            .completion()
            .map(|value| Arc::new(NativeProductCompletion { value }))
    }
    /// Blocking settlement wait, run off-UI independently of submission/close.
    pub fn wait(&self) -> Arc<NativeProductCompletion> {
        Arc::new(NativeProductCompletion {
            value: self.value.wait(),
        })
    }
}
/// Immutable delivered batch retaining both native count and byte permits.
#[derive(uniffi::Object)]
pub struct NativeProductEventBatch {
    value: ProductEventBatch,
}
#[uniffi::export]
impl NativeProductEventBatch {
    /// Bounded canonical copies of original ordered frames. Copies are caller history;
    /// the batch retains its native permits until its last foreign handle release.
    pub fn frames_json(&self) -> Result<Vec<String>, NativeProductCallError> {
        self.value.values().iter().map(json).collect()
    }
}
/// Escapable child handle with its independent physical owner, never its parent.
#[derive(uniffi::Object)]
pub struct NativeProductSession {
    pub(crate) value: SessionProduct,
}
#[uniffi::export]
impl NativeProductSession {
    /// Historical actual Ready when captured, even after terminal close.
    pub fn ready(&self) -> Option<Arc<NativeProductReady>> {
        self.value
            .value
            .ready()
            .map(|value| Arc::new(NativeProductReady { value }))
    }
    /// Original first terminal physical failure.
    pub fn failure(&self) -> Option<Arc<NativeProductFailure>> {
        self.value
            .value
            .failure()
            .map(|value| Arc::new(NativeProductFailure { value }))
    }
    /// Wait for actual opening independently of retained operations and submissions.
    pub fn wait_ready(&self) -> Result<Arc<NativeProductReady>, NativeProductCallError> {
        self.value
            .value
            .wait_ready()
            .map(|value| Arc::new(NativeProductReady { value }))
            .map_err(Into::into)
    }
    /// Admit through core bounds, actual Ready.methods and native ID allocation.
    pub fn request(
        &self,
        method: String,
        params_json: String,
    ) -> Result<Arc<NativeProductTicket>, NativeProductCallError> {
        self.value
            .value
            .request(&method, &params_json)
            .map(|value| Arc::new(NativeProductTicket { value }))
            .map_err(Into::into)
    }
    /// Wait at most this relative duration on the native clock; zero takes queued
    /// events immediately. Core owns event delivery and terminal close decisions.
    pub fn events(
        &self,
        timeout_ms: u64,
    ) -> Result<Arc<NativeProductEventBatch>, NativeProductCallError> {
        let deadline = self
            .value
            .clock
            .elapsed_ms()
            .checked_add(timeout_ms)
            .ok_or(ProductFailure::InvalidLimits)?;
        self.value
            .value
            .events(deadline)
            .map(|value| Arc::new(NativeProductEventBatch { value }))
            .map_err(Into::into)
    }
    /// Fence submission and interrupt I/O independently of a blocking waiter.
    pub fn request_close(&self) {
        self.value.value.request_close();
    }
    /// Join physical ownership off-UI, retaining its original terminal failure.
    pub fn close(&self) -> Result<(), NativeProductCallError> {
        self.value.value.close().map_err(Into::into)
    }
}
