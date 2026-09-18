use tracing_subscriber::EnvFilter;

/// Install the process-global tracing subscriber. Call once at startup, before any log line.
pub fn init() {
    // The SDK is where an agent actually runs, so its warnings and errors are
    // the operator's to see: a provider refusing to start, an audit sink
    // rejecting evidence, cleanup it could not confirm. Filtered to this crate
    // alone, all of that was dropped before it reached a terminal, and a
    // gateway that could not open a conversation said so only as a code. Its
    // info and debug lines stay off, because those are a developer's.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("nessa_server=info,nessa_sdk=warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();
}
