//! The process-global tracing subscriber, and the one thing it decides:
//! whether its output is for a screen or for a file.
//!
//! The packaged gateway is started by launchd, which redirects this process's
//! stderr straight into `~/.nessa/logs/gateway.log`. Colour written there is
//! not colour, it is `\u{1b}[2m` and `\u{1b}[31m` wrapped around every field of
//! the one artefact a person can send us — harder to read, and harder to grep
//! for the line that matters. Colour is for a terminal, so it is asked for only
//! when stderr is one.
use std::io::IsTerminal;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Install the process-global tracing subscriber. Call once at startup, before any log line.
pub fn init() {
    subscriber(std::io::stderr, std::io::stderr().is_terminal(), filter()).init();
}

fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("nessa_server=info"))
}

/// The subscriber `init` installs, over any writer, so what it writes can be
/// read back in a test instead of guessed at.
fn subscriber<W>(
    writer: W,
    colour: bool,
    filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_ansi(colour)
        .with_writer(writer)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// A writer the test can read back, shared with the subscriber.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl Captured {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("captured log").clone()).expect("utf-8 log")
        }
    }
    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("captured log").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn written(colour: bool) -> String {
        let captured = Captured::default();
        let writer = captured.clone();
        tracing::subscriber::with_default(
            subscriber(
                move || writer.clone(),
                colour,
                EnvFilter::new("nessa_server=info"),
            ),
            || tracing::error!(detail = "registry is invalid", "nessa failed"),
        );
        captured.text()
    }

    /// The gateway's log is a file launchd opened, and a file is what this
    /// process's stderr is whenever it is not a terminal.
    #[test]
    fn a_log_that_is_not_a_terminal_has_no_escape_sequences_in_it() {
        let file = written(false);
        assert!(file.contains("nessa failed"), "{file}");
        assert!(file.contains("registry is invalid"), "{file}");
        assert!(!file.contains('\u{1b}'), "{file:?}");
        // A terminal still gets its colour: this is one decision, not the
        // removal of a feature.
        assert!(written(true).contains('\u{1b}'));
    }
}
