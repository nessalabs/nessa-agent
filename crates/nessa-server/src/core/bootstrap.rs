use std::process::Termination;

use crate::composition::CompositionRoot;
use crate::core::logging;

/// Bootstrap logging, runtime, and the HTTP/WebSocket server.
pub fn run() -> std::process::ExitCode {
    logging::init();
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

    match runtime.block_on(CompositionRoot::serve()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => error.report(),
    }
}
