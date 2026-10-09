//! Stable sanitized JSON values preserve each typed refusal owner.
use crate::read_only_sync::application::{CacheError, GatewayError};
use nessa_protocol::product_contract::generated::ChangeWatchEndReason;
use nessa_sdk::application::agent_execution::sessions::StorageError;
use nessa_sync::replication::{
    application::{SourceError, StoreError, SyncError},
    catalogue::{
        CatalogueError, CatalogueProgressError, CatalogueSourceError, CatalogueStoreError,
        CatalogueValidationError,
    },
    domain::ValidationError,
};
use serde_json::{json, Value};
pub(crate) fn gateway_failure(error: GatewayError) -> Value {
    let (code, product) = match error {
        GatewayError::InvalidPolicy => ("invalidPolicy", None),
        GatewayError::InvalidRequest => ("invalidRequest", None),
        GatewayError::DriverPanicked => ("driverPanicked", None),
        GatewayError::Cancelled => ("cancelled", None),
        GatewayError::TimedOut => ("timedOut", None),
        GatewayError::Transport => ("transport", None),
        GatewayError::Closed(reason) => ("closed", reason.map(|value| value.as_str())),
        GatewayError::Protocol => ("protocol", None),
        GatewayError::Correlation => ("correlation", None),
        GatewayError::NativeHandshake => ("nativeHandshake", None),
        GatewayError::ResponseTooLarge => ("responseTooLarge", None),
        GatewayError::RequestTooLarge => ("requestTooLarge", None),
        GatewayError::EventCapacity => ("eventCapacity", None),
        GatewayError::InvalidCredential => ("invalidCredential", None),
        GatewayError::ProductRefused => ("productRefused", None),
        GatewayError::Authentication(reason) => ("authentication", Some(reason.as_str())),
        GatewayError::ScopeChanged => ("scopeChanged", None),
        GatewayError::Busy => ("busy", None),
        GatewayError::Record(reason) => ("record", Some(reason.as_str())),
        GatewayError::Catalogue(reason) => ("catalogue", Some(reason.as_str())),
        GatewayError::Watch(reason) => ("watch", Some(reason.as_str())),
    };
    json!({"code":code,"productCode":product})
}
/// The gateway's end of a watch, with its product reason.
pub(crate) fn watch_ended(reason: ChangeWatchEndReason) -> Value {
    json!({"code":"watchEnded","productCode":reason})
}
fn storage_failure(error: &StorageError) -> Value {
    let code = match error {
        StorageError::Closed => "closed",
        StorageError::ReadCapacity => "readCapacity",
        StorageError::ReadWorkerPanicked => "readWorkerPanicked",
        StorageError::DiagnosticLimit => "diagnosticLimit",
        StorageError::ShutdownFailures(causes) => {
            return json!({"code":"shutdownFailures", "read":storage_failure(causes.read()),"runtime":storage_failure(causes.runtime())})
        }
        StorageError::Busy => "busy",
        StorageError::Io(_) => "io",
        StorageError::Corrupt(_) => "corrupt",
        StorageError::IdentityMismatch => "identityMismatch",
        StorageError::ChangesRequired => "changesRequired",
        StorageError::Unresolved => "unresolved",
        StorageError::TooLarge => "tooLarge",
        StorageError::CommittedReadUnavailable => "committedReadUnavailable",
        StorageError::AnotherVersion { .. } => "corrupt",
    };
    json!({"code":code})
}
pub(crate) fn cache_failure(error: &CacheError) -> Value {
    let code = match error {
        CacheError::InvalidPolicy => "invalidPolicy",
        CacheError::Unavailable => "unavailable",
        CacheError::Uncertain => "uncertain",
        CacheError::Quota => "quota",
        CacheError::Corrupt => "corrupt",
        CacheError::OutdatedSchema => "outdatedSchema",
        CacheError::CatalogueProgress(cause) => {
            return json!({"code":"catalogueProgress", "cause":catalogue_progress_failure(cause)})
        }
        CacheError::CatalogueValidation(cause) => {
            return json!({"code":"catalogueValidation", "cause":{"code":catalogue_validation_code(cause)}})
        }
        CacheError::CatalogueMetadata => "catalogueMetadata",
        CacheError::ResetAttributionRequired => "resetAttributionRequired",
        CacheError::Transcript(cause) => {
            return json!({"code":"transcript","cause":storage_failure(cause)})
        }
        CacheError::TranscriptScope => "transcriptScope",
        CacheError::Stale => "stale",
        CacheError::ConflictingRecord => "conflictingRecord",
        CacheError::Fenced => "fenced",
        CacheError::Scope { saved, requested } => {
            return json!({"code":"scope", "saved":super::super::output::scope(saved),"requested":super::super::output::scope(requested)})
        }
    };
    json!({"code":code})
}
fn validation_code(error: &ValidationError) -> &'static str {
    match error {
        ValidationError::InvalidId => "invalidId",
        ValidationError::InvalidLimits => "invalidLimits",
        ValidationError::InvalidRange => "invalidRange",
        ValidationError::WrongRequest => "wrongRequest",
        ValidationError::EmptyPage => "emptyPage",
        ValidationError::BoundsExceeded => "boundsExceeded",
        ValidationError::OversizedRecord => "oversizedRecord",
        ValidationError::Noncontiguous => "noncontiguous",
        ValidationError::WrongRecord => "wrongRecord",
        ValidationError::PositionOverflow => "positionOverflow",
    }
}
pub(super) fn core_failure(error: &SyncError) -> Value {
    let code = match error {
        SyncError::Denied => "denied",
        SyncError::Unverifiable => "unverifiable",
        SyncError::WrongAccessScope => "wrongAccessScope",
        SyncError::SourceBehindCheckpoint => "sourceBehindCheckpoint",
        SyncError::Validation(cause) => {
            return json!({"code":"validation","cause":{"code":validation_code(cause)}})
        }
        SyncError::Source(cause) => {
            return json!({"code":"source","cause":{"code":match cause { SourceError::Unavailable=>"unavailable",SourceError::Pruned=>"pruned",SourceError::InvalidRequest=>"invalidRequest",SourceError::IdentityChanged=>"identityChanged",SourceError::OversizedRecord=>"oversizedRecord"}}})
        }
        SyncError::Store(cause) => {
            let code = match cause {
                StoreError::Failed => "failed",
                StoreError::Uncertain => "uncertain",
                StoreError::Stale => "stale",
                StoreError::ConflictingRecord => "conflictingRecord",
                StoreError::Fenced => "fenced",
                StoreError::ScopeMismatch { saved, requested } => {
                    return json!({"code":"store","cause":{"code":"scopeMismatch","saved":super::super::output::scope(saved),"requested":super::super::output::scope(requested)}})
                }
            };
            return json!({"code":"store","cause":{"code":code}});
        }
    };
    json!({"code":code})
}
fn catalogue_validation_code(error: &CatalogueValidationError) -> &'static str {
    match error {
        CatalogueValidationError::InvalidRequest => "invalidRequest",
        CatalogueValidationError::WrongRequest => "wrongRequest",
        CatalogueValidationError::InvalidOrder => "invalidOrder",
        CatalogueValidationError::NoProgress => "noProgress",
        CatalogueValidationError::WrongPayload => "wrongPayload",
        CatalogueValidationError::BoundsExceeded => "boundsExceeded",
        CatalogueValidationError::DeletionFence => "deletionFence",
    }
}
fn catalogue_progress_failure(error: &CatalogueProgressError) -> Value {
    let code = match error {
        CatalogueProgressError::InvalidProgress => "invalidProgress",
        CatalogueProgressError::WrongScope => "wrongScope",
        CatalogueProgressError::Stale => "stale",
        CatalogueProgressError::GenerationExhausted => "generationExhausted",
        CatalogueProgressError::InvalidPage(cause) => {
            return json!({"code":"invalidPage","cause":{"code":catalogue_validation_code(cause)}})
        }
    };
    json!({"code":code})
}
pub(super) fn catalogue_failure(error: &CatalogueError) -> Value {
    let code = match error {
        CatalogueError::Denied => "denied",
        CatalogueError::Unverifiable => "unverifiable",
        CatalogueError::WrongAccessScope => "wrongAccessScope",
        CatalogueError::Validation(cause) => {
            return json!({"code":"validation","cause":{"code":catalogue_validation_code(cause)}})
        }
        CatalogueError::Source(cause) => {
            return json!({"code":"source","cause":{"code":match cause { CatalogueSourceError::Unavailable=>"unavailable",CatalogueSourceError::IdentityChanged=>"identityChanged",CatalogueSourceError::InvalidRequest=>"invalidRequest",CatalogueSourceError::OversizedEntry=>"oversizedEntry"}}})
        }
        CatalogueError::Store(cause) => {
            return json!({"code":"store","cause":{"code":match cause {CatalogueStoreError::Failed=>"failed",CatalogueStoreError::Uncertain=>"uncertain",CatalogueStoreError::Stale=>"stale",CatalogueStoreError::ResetRequired=>"resetRequired",CatalogueStoreError::Fenced=>"fenced",CatalogueStoreError::Conflict=>"conflict"}}})
        }
    };
    json!({"code":code})
}
