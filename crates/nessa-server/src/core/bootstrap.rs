use std::process::Termination;

use crate::composition::CompositionRoot;
use crate::core::logging;
#[cfg(unix)]
use crate::{core::log_file, env::Environment};

/// Bootstrap logging, runtime, and the HTTP/WebSocket server.
pub fn run() -> std::process::ExitCode {
    logging::init();
    #[cfg(unix)]
    bound_log();
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
        Err(error) => error.report(),
    }
}

/// Keep `gateway.log` within its size bound, before this run writes its first
/// line into it.
///
/// A log that cannot be resolved, opened or rolled does not stop a server from
/// starting — the bound not holding is worth saying, and is not worth refusing
/// to run over. The line saying so is the first one in the new log.
#[cfg(unix)]
fn bound_log() {
    let logs = match Environment::log_directory_from_system() {
        Ok(Some(logs)) => logs,
        // No data root at all, or a stage this build will refuse further down
        // with a sentence of its own.
        Ok(None) | Err(_) => return,
    };
    if let Err(error) = log_file::bound(&logs.join(log_file::GATEWAY_LOG)) {
        tracing::warn!(%error, "could not keep the gateway log within its size bound");
    }
}
