use std::path::Path;

use crate::composition::CompositionRoot;
#[cfg(unix)]
use crate::core::log_file;
use crate::core::{error, logging};
use crate::env::Environment;

/// Bootstrap logging, runtime, and the HTTP/WebSocket server.
pub fn run() -> std::process::ExitCode {
    logging::init();
    // The stage's log directory is resolved before anything else is parsed:
    // the log this run is about to write into lives there, and so does the
    // record of why the last run gave up — both are needed even when the
    // reason this run ends is that its own configuration would not parse.
    let logs = Environment::log_directory_from_system().ok().flatten();
    #[cfg(unix)]
    if let Some(logs) = logs.as_deref() {
        bound_log(logs);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "failed to start async runtime");
            return std::process::ExitCode::FAILURE;
        }
    };

    match runtime.block_on(CompositionRoot::run(
        &std::env::args().skip(1).collect::<Vec<_>>(),
    )) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(failure) => error::report(failure, logs.as_deref()),
    }
}

/// Keep `gateway.log` within its size bound, before this run writes its first
/// line into it.
///
/// A log that cannot be opened or rolled does not stop a server from starting:
/// the bound not holding is worth saying, and is not worth refusing to run
/// over. The line saying so is the first one in the log it could not bound.
#[cfg(unix)]
fn bound_log(logs: &Path) {
    if let Err(error) = log_file::bound(&logs.join(log_file::GATEWAY_LOG)) {
        tracing::warn!(%error, "could not keep the gateway log within its size bound");
    }
}
