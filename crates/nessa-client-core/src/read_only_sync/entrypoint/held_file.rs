//! The `fetch` command's one JSON line: the file named, how far the fetch
//! got, and why it stopped when it did not keep the file.
use super::online::{gateway_failure, write};
use super::CommandError;
use crate::read_only_sync::application::held_file::{
    FetchCause, FetchFailure, FetchProgress, FileStep, PartCleanup,
};
use crate::read_only_sync::application::{GatewayError, HeldFileRefusal};
use crate::read_only_sync::domain::HeldFile;
use serde_json::{json, Value};
use std::io::Write;

/// `device` carries the enrollment evidence the run was admitted under, and
/// any recheck after a refusal; its fields join the report. `connected` says
/// whether the gateway was dialled: an output already there is answered
/// without it. A failure on this device's files is `LocalFile`; any other is
/// `OnlineRefused`.
pub(crate) fn write_fetch(
    file: &HeldFile,
    result: Result<FetchProgress, FetchFailure>,
    connected: bool,
    device: Value,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let (progress, failure) = match result {
        Ok(progress) => (progress, None),
        Err(failure) => (failure.progress, Some(failure.cause)),
    };
    let mut report = json!({
        "operation":"fetch","successful":failure.is_none(),
        "connectionCheck":if connected { "performed" } else { "notPerformed" },
        "file":{"conversationId":file.conversation().to_string(),"digest":file.digest().to_string(),
            "mimeType":file.media_type().as_str(),"size":file.size().to_string()},
        "resumedFrom":progress.resumed_from.to_string(),"reached":progress.reached.to_string(),
        "ranges":progress.ranges.to_string(),"busyRetries":progress.busy_retries.to_string(),
        "fetchFailure":failure.map(fetch_failure),
    });
    if let (Some(report), Value::Object(device)) = (report.as_object_mut(), device) {
        report.extend(device);
    }
    write(report, output)?;
    match failure {
        None => Ok(()),
        Some(cause) if cause.local() => Err(CommandError::LocalFile),
        Some(_) => Err(CommandError::OnlineRefused),
    }
}

fn fetch_failure(cause: FetchCause) -> Value {
    match cause {
        // An older gateway that has no `artifact.read`.
        FetchCause::Gateway(error @ GatewayError::HeldFile(HeldFileRefusal::UnknownMethod)) => {
            json!({"code":"unsupported","transportFailure":gateway_failure(error)})
        }
        FetchCause::Gateway(error) => {
            json!({"code":"gateway","transportFailure":gateway_failure(error)})
        }
        FetchCause::WrongRange => json!({"code":"wrongRange"}),
        FetchCause::Integrity { cleanup } => {
            let (part, remove, restart) = match cleanup {
                PartCleanup::Removed => ("removed", None, None),
                PartCleanup::Emptied { remove } => ("emptied", Some(remove), None),
                PartCleanup::Left { remove, restart } => ("left", Some(remove), Some(restart)),
            };
            json!({"code":"integrity","part":part,"removeFailure":remove.map(step),"restartFailure":restart.map(step)})
        }
        FetchCause::OutputExists => json!({"code":"outputExists"}),
        FetchCause::File(failed) => json!({"code":"file","step":step(failed)}),
        FetchCause::Published { step: failed } => json!({"code":"linked","step":step(failed)}),
    }
}

fn step(step: FileStep) -> &'static str {
    match step {
        FileStep::Open => "open",
        FileStep::Locked => "locked",
        FileStep::Read => "read",
        FileStep::Restart => "restart",
        FileStep::Write => "write",
        FileStep::Sync => "sync",
        FileStep::Link => "link",
        FileStep::SyncDirectory => "syncDirectory",
        FileStep::Remove => "remove",
        FileStep::Name => "name",
        FileStep::Changed => "changed",
    }
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/entrypoint/held_file.rs"]
mod tests;
