//! Why a launchd-managed gateway is not starting, in one sentence for a person.
//!
//! Readiness has a deadline because a healthy server can be slow. A server that
//! exits on startup is not slow, and waiting out its deadline reports the
//! readiness contract ("did not advertise the expected runtime identity")
//! instead of the reason.
//!
//! The reason comes from the server, deliberately. `RunError` chooses a process
//! exit code from `protocol/defaults/gateway-exit-codes.json`, launchd records
//! the exit code of the service it supervises, and `launchctl print` reports it
//! back. The log is prose written for a person and is never parsed: a message
//! can be reworded, a line can belong to an earlier run in the same append-only
//! file, and a healthy launch mentions the same subsystems a failing one does.
//!
//! There is one ending the exit code cannot carry. A server that would fail the
//! same way on every restart exits *successfully*, because
//! `KeepAlive: { SuccessfulExit: false }` is launchd's only exit condition and
//! zero is the only thing it reads as "stop" — so that status names nothing by
//! design. For that one case the server writes the reason down beside its log,
//! in the same table's vocabulary, and [`RecordedFailure`] reads it. It is
//! still a file rather than launchd's word, so it is corroborated against the
//! registration being reconciled before it decides anything.
//!
//! The sentence is for the panel. The exit status, the tail of the log the
//! plist already redirects the process's stderr to, and the readiness message
//! all belong in the app's own log, and are carried separately for that.
#[allow(unused_imports)]
pub(super) use super::super::startup_failure::{
    forget_recorded_failure, parse_record, recorded_failure, RecordedFailure,
};
use nessa_local_storage::OpenMode;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::LazyLock,
};

/// Bytes of the gateway log read back from the end. Startup failures report
/// themselves in the first moments of a run, so a short tail is enough and a
/// log that has grown for weeks is never read whole.
const TAIL_BYTES: u64 = 16_384;
/// Lines of that tail kept for the app log. Enough for a panic and its context.
const TAIL_LINES: usize = 12;

/// What launchd last reported about the service's own process.
///
/// `Unknown` is what an output we cannot read says, and it is the value that
/// keeps the caller waiting: a missing or ambiguous line is not evidence of
/// death.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LastExit {
    Unknown,
    NeverExited,
    Code(i32),
    /// A process that was signalled did not choose a code, and launchd prints
    /// no exit code for it at all — only the signal that ended it.
    Signal(String),
    Reason(String),
}
impl LastExit {
    /// Whether launchd has seen this service's process go away unsuccessfully.
    pub(super) fn is_failure(&self) -> bool {
        match self {
            Self::Code(code) => *code != 0,
            Self::Signal(_) | Self::Reason(_) => true,
            Self::Unknown | Self::NeverExited => false,
        }
    }
    fn describe(&self) -> String {
        match self {
            Self::Unknown => "unknown".into(),
            Self::NeverExited => "never exited".into(),
            Self::Code(code) => format!("exit code {code}"),
            Self::Signal(signal) => format!("terminating signal {signal}"),
            Self::Reason(reason) => format!("exit reason {reason}"),
        }
    }
}

/// `launchctl print` is diagnostic output, and what it prints for a process
/// that is gone takes several shapes. Verified against launchd on macOS 26:
///
/// ```text
/// last exit code = 0
/// last exit code = (never exited)
/// last exit code = 1
/// last exit code = 78: EX_CONFIG          // sysexits values are annotated
/// last terminating signal = Segmentation fault: 11
/// last exit reason = JETSAM_REASON_MEMORY_IDLE_EXIT
/// ```
///
/// A signalled process has no exit code line at all, so the two are read
/// separately and the signal wins: a process that was killed did not choose a
/// code, and if launchd ever prints both, the signal is the later fact.
/// Disagreeing or unparsable lines of one kind still resolve to `Unknown`,
/// which never accelerates a failure.
pub(super) fn parse_last_exit(text: &str) -> LastExit {
    let mut exit: Option<LastExit> = None;
    let mut signal: Option<LastExit> = None;
    // Lines are trimmed, so a key whose value is empty has no trailing space
    // left to match: the prefixes stop at the `=` and the value is trimmed
    // after, which is what lets an empty value be seen as one.
    for line in text.lines().map(str::trim) {
        let (slot, parsed) = if let Some(value) = line.strip_prefix("last exit code =") {
            (&mut exit, parse_exit_value(value.trim(), false))
        } else if let Some(value) = line.strip_prefix("last exit status =") {
            (&mut exit, parse_exit_value(value.trim(), true))
        } else if let Some(value) = line.strip_prefix("last terminating signal =") {
            (
                &mut signal,
                parse_label(value.trim()).map_or(LastExit::Unknown, LastExit::Signal),
            )
        } else if let Some(value) = line.strip_prefix("last exit reason =") {
            (
                &mut exit,
                parse_label(value.trim()).map_or(LastExit::Unknown, LastExit::Reason),
            )
        } else {
            continue;
        };
        if slot.get_or_insert_with(|| parsed.clone()) != &parsed {
            *slot = Some(LastExit::Unknown);
        }
    }
    match signal {
        // An unreadable signal line is not licence to fall back to an exit
        // code it contradicts.
        Some(LastExit::Unknown) => LastExit::Unknown,
        Some(signal) => signal,
        None => exit.unwrap_or(LastExit::Unknown),
    }
}
fn parse_label(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 128).then(|| value.to_owned())
}
fn parse_exit_value(value: &str, wait_encoded: bool) -> LastExit {
    if value == "(never exited)" {
        return LastExit::NeverExited;
    }
    // launchd annotates the sysexits values — `78: EX_CONFIG` — and prints a
    // bare number otherwise. The number is the fact; the symbol after it is a
    // courtesy, and demanding the whole string be an integer threw away the
    // most common real failure there is.
    let digits = value
        .split_once(':')
        .map_or(value, |(number, _)| number)
        .trim();
    let Ok(raw) = digits.parse::<i32>() else {
        return LastExit::Unknown;
    };
    // A wait(2) status carries the exit code in its high byte; a status that is
    // not a clean multiple of 256 is a signal, which is a failure either way.
    if wait_encoded && raw >= 256 {
        LastExit::Code(raw >> 8)
    } else {
        LastExit::Code(raw)
    }
}

/// The last lines the gateway process wrote before launchd noticed it was gone.
///
/// A log we cannot open is not a diagnosis and not an error: the caller still
/// has the exit status, and says less rather than blaming the log.
pub(super) fn log_tail(path: &Path) -> String {
    let Ok(mut file) = nessa_local_storage::open(path, OpenMode::ReadNonblocking) else {
        return String::new();
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return String::new();
    };
    let start = length.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.take(TAIL_BYTES).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text
        .lines()
        // A seek into a longer file lands somewhere inside a line. That
        // fragment is a piece of a sentence, not a line the log wrote, so it
        // goes; a whole log is read from its first line.
        .skip(usize::from(start > 0))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// One sentence for the panel, and the evidence behind it for the app log.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct StartupFailure {
    pub sentence: String,
    pub detail: String,
}

/// Name the cause when the server named it, and say plainly that we cannot
/// when it did not. The log tail is carried to the app log and never read for
/// meaning: it is prose written for a person, and the exit code is the
/// contract.
pub(super) fn diagnose(
    last_exit: &LastExit,
    recorded: Option<&RecordedFailure>,
    tail: &str,
    port: u16,
    readiness: &str,
) -> StartupFailure {
    let cause = recognise(last_exit, recorded, port);
    let sentence = match &cause {
        Some(cause) => format!("Nessa's background service is not starting: {cause}"),
        None if tail.is_empty() => {
            "Nessa's background service is not starting, and it exited without reporting why."
                .into()
        }
        None => "Nessa's background service is not starting.".into(),
    };
    let mut detail = format!(
        "gateway did not start: launchd reports {}; reported cause: {}; {readiness}",
        last_exit.describe(),
        cause.as_deref().unwrap_or("none reported")
    );
    if let Some(recorded) = recorded {
        detail.push('\n');
        detail.push_str(&recorded.describe());
    }
    if !tail.is_empty() {
        detail.push_str("\ngateway log tail:\n");
        detail.push_str(tail);
    }
    StartupFailure { sentence, detail }
}

const UNLAUNCHABLE: &str = "its background program could not be launched.";
/// `protocol/defaults/gateway-exit-codes.json`, the same bytes the server
/// compiles in to choose the code it exits with. Reading the number is the
/// whole of how this host learns why the service stopped: the server says it
/// deliberately, rather than this host guessing from lines the server wrote
/// for a person to read.
const EXIT_CODES_JSON: &str =
    include_str!("../../../../../protocol/defaults/gateway-exit-codes.json");

#[derive(Debug, Deserialize)]
struct GatewayExitCodes {
    codes: BTreeMap<String, u8>,
}
static CODES: LazyLock<GatewayExitCodes> = LazyLock::new(|| {
    serde_json::from_str(EXIT_CODES_JSON).expect("bundled gateway-exit-codes.json must parse")
});

impl RecordedFailure {
    pub(super) fn sentence(&self, port: u16) -> Option<String> {
        sentence_for(self.reason(), port)
    }
}

/// The sentence a reason becomes. A reason the table names but this host has
/// no sentence for falls through to the generic message rather than showing
/// someone a name out of a JSON file.
fn sentence_for(reason: &str, port: u16) -> Option<String> {
    match reason {
        "credentialRegistryInvalid" => {
            Some("its credential registry is not one this version of Nessa can read.".into())
        }
        "credentialRegistry" => Some("its credential registry could not be read.".into()),
        "alreadyRunning" => Some("another Nessa is already running for this stage.".into()),
        "portInUse" => Some(format!("port {port} is already in use.")),
        "configuration" => Some("its configuration is not one it can start with.".into()),
        "runtime" => Some(
            "the prepared runtime it was registered with is missing or is not the one it expects."
                .into(),
        ),
        // Not a failure at all: the service was asked to stop and said so
        // rather than exiting zero, which would have told launchd to leave it
        // stopped. Only a reconciliation landing inside the restart throttle
        // ever sees it, and what it is seeing is a service on its way back.
        "stoppedOnRequest" => Some("it was stopped, and is starting again.".into()),
        _ => None,
    }
}

/// launchd's own report of a spawn it could not complete, verified against it:
/// a plist naming a program that is not there comes back as
/// `last exit code = 78: EX_CONFIG`, not as the 126 or 127 a shell would use.
/// The server never exits 78 — its own codes come from the shared table — so
/// seeing it means launchd never got our program running.
const SPAWN_FAILED: i32 = 78;

/// What the service's own exit code says, what launchd says when the program
/// never ran to have an opinion, or — when neither names a cause — what the
/// gateway wrote down on its way out.
///
/// The record is consulted last and only then, because an exit code is
/// launchd's own observation of this service's process while the file is
/// whatever was last left in a directory. It is the answer for exactly one
/// ending: the zero status a server exits with to stop being relaunched, which
/// names nothing by design.
fn recognise(
    last_exit: &LastExit,
    recorded: Option<&RecordedFailure>,
    port: u16,
) -> Option<String> {
    let named = |recorded: Option<&RecordedFailure>| {
        recorded.and_then(|recorded| sentence_for(recorded.reason(), port))
    };
    let code = match last_exit {
        LastExit::Code(code) => *code,
        // Killed rather than exited: by the code-signing enforcement that
        // refuses a runtime, by the kernel under memory pressure, or by its
        // own fault. Which of those it was belongs in the log with the signal
        // name; none of them is the program choosing to stop.
        LastExit::Signal(_) => return Some("its background program stopped abruptly.".into()),
        LastExit::Unknown | LastExit::NeverExited | LastExit::Reason(_) => return named(recorded),
    };
    // 126 and 127 are what a shell in front of the program would report; they
    // are kept because a wrapper can still produce them.
    if matches!(code, SPAWN_FAILED | 126 | 127) {
        return Some(UNLAUNCHABLE.into());
    }
    u8::try_from(code)
        .ok()
        .and_then(|code| CODES.codes.iter().find(|(_, value)| **value == code))
        .and_then(|(reason, _)| sentence_for(reason, port))
        .or_else(|| named(recorded))
}

#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/startup.rs"]
mod tests;
