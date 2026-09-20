use crate::cli::entrypoint::parse;
use crate::composition::CompositionRoot;
use crate::core::{error, logging, Launch, RunError};
#[cfg(unix)]
use crate::core::{launch::Managed, log_file};

/// Bootstrap logging, runtime, and the HTTP/WebSocket server.
pub fn run() -> std::process::ExitCode {
    logging::init();
    // What started this process is resolved once, here, and handed down. It
    // decides whether a failure may exit zero to stop launchd relaunching this
    // service, and whether the record that failure leaves behind may be written
    // or removed — one fact, not three sniffed out separately further in.
    let command = match parse(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(command) => command,
        Err(message) => {
            return error::report(RunError::Authentication(message), &Launch::Standalone)
        }
    };
    let launch = Launch::from_system(&command);
    #[cfg(unix)]
    if let Some(managed) = launch.managed() {
        bound_log(managed);
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

    match runtime.block_on(CompositionRoot::run(command, &launch)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(failure) => error::report(failure, &launch),
    }
}

/// Keep `gateway.log` within its size bound, before this run writes its first
/// line into it.
///
/// Only the service launchd redirects into that file has one to bound. A log
/// that cannot be opened or rolled does not stop a server from starting: the
/// bound not holding is worth saying, and is not worth refusing to run over.
/// The line saying so is the first one in the log it could not bound.
#[cfg(unix)]
fn bound_log(managed: &Managed) {
    if let Err(error) = log_file::bound(&managed.logs().join(log_file::GATEWAY_LOG)) {
        tracing::warn!(%error, "could not keep the gateway log within its size bound");
    }
}
