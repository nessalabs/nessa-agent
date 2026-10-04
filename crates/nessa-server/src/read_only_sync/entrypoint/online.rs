//! JSON presentation of typed connection, driver and durable cache evidence.
//! `causes` presents typed owner codes without adapter diagnostics.
use super::{
    output::{catalogue_progress, record_progress},
    CommandError,
};
use crate::conversation::application::ConversationTranscriptState;
use crate::read_only_sync::application::{
    driver::{CatalogueRun, RecordDriverCause, RecordDriverError, RecordRun},
    CacheError, CachedProgress, GatewayAttempt,
};
use nessa_sdk::application::agent_execution::sessions::CommittedStatus;
use nessa_sync::replication::catalogue::{CatalogueError, CatalogueProgress};
use serde_json::{json, Value};
use std::io::Write;
mod causes;
pub(crate) use causes::{cache_failure, gateway_failure};
use causes::{catalogue_failure, core_failure};

/// `device` carries the enrollment evidence the run was admitted under, and
/// any recheck after a refusal; its fields join the report.
pub(crate) fn write_records(
    attempt: &GatewayAttempt<Result<RecordRun, RecordDriverError>>,
    saved: Result<Option<(CachedProgress, CommittedStatus)>, CacheError>,
    cache_refusal: Option<CacheError>,
    device: Value,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let (check, work, cause, freshness_failure) = match &attempt.result {
        Some(Ok(run)) => (
            Some(json!({"head":run.checked_head.to_string(),"checkedAtMs":run.checked_at_ms})),
            Some(
                json!({"downloaded":run.downloaded.to_string(),"pages":run.pages,"complete":run.complete}),
            ),
            None,
            None,
        ),
        Some(Err(error)) => (
            error.captured_check.map(
                |value| json!({"head":value.head.to_string(),"checkedAtMs":value.checked_at_ms}),
            ),
            None,
            Some(match &error.cause {
                RecordDriverCause::Core(error) => {
                    json!({"owner":"core","cause":core_failure(error)})
                }
                RecordDriverCause::Cache(error) => {
                    json!({"owner":"cache","cause":cache_failure(error)})
                }
            }),
            error.freshness_failure.as_ref().map(cache_failure),
        ),
        None => (None, None, None, None),
    };
    let saved_ok = saved.is_ok();
    let durable = match saved {
        Ok(Some((progress, status))) => {
            json!({"progress":record_progress(&progress),"status":ConversationTranscriptState::from(status.view_state())})
        }
        Ok(None) => json!({"progress":null,"status":"notLoaded"}),
        Err(error) => json!({"failure":cache_failure(&error)}),
    };
    let successful = matches!(attempt.result, Some(Ok(_)))
        && attempt.outcome.failure.is_none()
        && saved_ok
        && cache_refusal.is_none();
    write(
        joined(
            json!({"operation":"records","successful":successful,"connectionCheck":"performed","connectionOperation":attempt.outcome.operation.to_string(), "capturedCheck":check,"work":work,"durable":durable,"transportFailure":attempt.outcome.failure.map(gateway_failure),"driverFailure":cause,"cacheRefusal":cache_refusal.as_ref().map(cache_failure),"freshnessFailure":freshness_failure}),
            device,
        ),
        output,
    )?;
    if successful {
        Ok(())
    } else {
        Err(CommandError::OnlineRefused)
    }
}
pub(crate) fn write_catalogue(
    attempt: &GatewayAttempt<Result<CatalogueRun, CatalogueError>>,
    saved: Result<Option<CatalogueProgress>, CacheError>,
    cache_refusal: Option<CacheError>,
    device: Value,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let work = attempt
        .result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|run| json!({"pages":run.pages,"complete":run.complete}));
    let saved_ok = saved.is_ok();
    let durable = match saved {
        Ok(progress) => json!({"progress":progress.as_ref().map(catalogue_progress)}),
        Err(error) => json!({"failure":cache_failure(&error)}),
    };
    let successful = matches!(attempt.result, Some(Ok(_)))
        && attempt.outcome.failure.is_none()
        && saved_ok
        && cache_refusal.is_none();
    write(
        joined(
            json!({"operation":"catalogue","successful":successful,"connectionCheck":"performed","connectionOperation":attempt.outcome.operation.to_string(),"work":work,"durable":durable,"transportFailure":attempt.outcome.failure.map(gateway_failure),"driverFailure":attempt.result.as_ref().and_then(|result|result.as_ref().err()).map(|error|json!({"owner":"core","cause":catalogue_failure(error)})),"cacheRefusal":cache_refusal.as_ref().map(cache_failure)}),
            device,
        ),
        output,
    )?;
    if successful {
        Ok(())
    } else {
        Err(CommandError::OnlineRefused)
    }
}
fn joined(mut report: Value, device: Value) -> Value {
    if let (Some(report), Value::Object(device)) = (report.as_object_mut(), device) {
        report.extend(device);
    }
    report
}
pub(crate) fn write(value: Value, output: &mut dyn Write) -> Result<(), CommandError> {
    serde_json::to_writer(&mut *output, &value).map_err(|_| CommandError::Output)?;
    output.write_all(b"\n").map_err(|_| CommandError::Output)
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/entrypoint/online.rs"]
mod tests;
