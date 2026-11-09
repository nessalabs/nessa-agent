//! Composition of `fetch`: the output checked before anything is dialled,
//! then one held file fetched over the admitted connection into the files
//! beside its output, and the one JSON line.
//!
//! ```text
//! kept: output's name has no room for a part --> refused, nothing dialled
//!       output there? --is the file--> success, nothing dialled
//!                     --is not-------> refused, left alone
//!                     --absent-------> (status, connect) --> fetch
//! fetch: fetch_held_file(connection, pause, disk files) --> line
//!          --gateway refusal--> recheck asks the pinned status if it names authority
//! ```
//! Arrows are calls in order. `online` admits the run and supplies the
//! connection, the recheck and the enrollment evidence.
use crate::read_only_sync::application::held_file::{
    fetch_held_file, kept_output, FetchCause, FetchFailure, FetchProgress, FileStep,
    HeldFileSource, Pause,
};
use crate::read_only_sync::application::{device::asks_status, GatewayError};
use crate::read_only_sync::domain::HeldFile;
use crate::read_only_sync::entrypoint::held_file::write_fetch;
use crate::read_only_sync::entrypoint::CommandError;
use crate::read_only_sync::infrastructure::held_file::DiskFiles;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

/// The answer when `out` already decides the command: it is the file
/// (success), or it is something else, or it cannot be read, or it has no
/// room for a part beside it (refused). `None` when there is no output, so
/// the file is to be fetched. Nothing was dialled, so `recheck` is `null`.
pub(super) fn kept(
    file: &HeldFile,
    out: &Path,
    output: &mut dyn Write,
) -> Option<Result<(), CommandError>> {
    let result = match DiskFiles::new(out, file) {
        Ok(mut files) => kept_output(&mut files, file).transpose()?,
        Err(step) => Err(refused(step)),
    };
    Some(write_fetch(
        file,
        result,
        false,
        json!({"recheck":null}),
        output,
    ))
}

fn refused(step: FileStep) -> FetchFailure {
    FetchFailure {
        cause: FetchCause::File(step),
        progress: FetchProgress::default(),
    }
}

/// Fetch `file` into `out` over `source`. `recheck` asks the pinned status
/// again, and is called only for a gateway failure that names this device's
/// authority (`asks_status`); every other report carries `null`.
pub(super) fn fetch(
    source: &mut dyn HeldFileSource,
    pause: &dyn Pause,
    file: &HeldFile,
    out: &Path,
    recheck: &dyn Fn(GatewayError) -> Value,
    enrollment: Value,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let result = DiskFiles::new(out, file)
        .map_err(refused)
        .and_then(|mut files| fetch_held_file(source, pause, &mut files, file, &|| false));
    let recheck = match &result {
        Err(FetchFailure {
            cause: FetchCause::Gateway(failure),
            ..
        }) if asks_status(*failure) => recheck(*failure),
        _ => Value::Null,
    };
    write_fetch(
        file,
        result,
        true,
        json!({"enrollment":enrollment,"recheck":recheck}),
        output,
    )
}

#[cfg(test)]
#[path = "../../tests/composition/read_only_fetch.rs"]
mod tests;
