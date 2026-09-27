use crate::cli::entrypoint::{parse, Command};
use crate::composition::CompositionRoot;
use crate::core::{ending, logging, Launch, RunError};
#[cfg(unix)]
use crate::{core::log_file, env::Environment};

/// Bootstrap logging, runtime, and the HTTP/WebSocket server.
pub fn run() -> std::process::ExitCode {
    logging::init();
    #[cfg(unix)]
    bound_log();
    // What started this process is resolved once, here, and handed down. It
    // decides whether a failure may exit zero to stop launchd relaunching this
    // service, and whether the record that failure leaves behind may be written
    // or removed — one fact, not three sniffed out separately further in.
    let command = match parsed(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(command) => command,
        Err(failure) => {
            return ending::report(
                Err(failure),
                // Arguments this build cannot parse are not the ones the plist
                // holds, so there is no registration to answer for.
                &Launch::Standalone,
            );
        }
    };
    let launch = Launch::from_system(&command);
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

    ending::report(
        runtime.block_on(CompositionRoot::run(command, &launch)),
        &launch,
    )
}

/// The command these arguments name, or the one failure any invocation can end
/// in.
///
/// Parsing happens before anything is dispatched, so a command line this build
/// cannot run is the one thing that can go wrong whatever it was trying to do.
/// It used to be reported as `Authentication`, which said authentication setup
/// failed about an invocation that never reached authentication — and under
/// launchd, where this installation's own plist is what got it wrong, that sent
/// the host looking in the wrong place. A usage problem is its own fact.
///
/// A named function rather than three lines inline because this is the only
/// place the mapping exists, and a mapping nothing can name is one nothing can
/// test.
fn parsed(arguments: &[String]) -> Result<Command, RunError> {
    parse(arguments).map_err(RunError::Usage)
}

/// Keep `gateway.log` within its size bound, before this run writes its first
/// line into it.
///
/// Deliberately not gated on this being the launch the desktop host registered:
/// the run that most needs the bound is the one relaunching every five seconds,
/// and a process that fails *before* it can establish which registration it
/// answers for is exactly that. What makes this safe is not who we are but
/// whether our own stderr is that file, which `log_file` establishes by device
/// and inode; for every other process it is a stat and nothing else.
///
/// A log that cannot be opened or rolled does not stop a server from starting:
/// the bound not holding is worth saying, and is not worth refusing to run
/// over. The line saying so is the first one in the log it could not bound.
#[cfg(unix)]
fn bound_log() {
    let Ok(Some(logs)) = Environment::log_directory_from_system() else {
        return;
    };
    if let Err(error) = log_file::bound(&logs.join(log_file::GATEWAY_LOG)) {
        tracing::warn!(%error, "could not keep the gateway log within its size bound");
    }
}

#[cfg(test)]
#[path = "../../tests/core/bootstrap.rs"]
mod tests;
