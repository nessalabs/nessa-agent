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
//! back. This module reads that number and nothing else for meaning. The log is
//! prose written for a person and is never parsed: a message can be reworded,
//! a line can belong to an earlier run in the same append-only file, and a
//! healthy launch mentions the same subsystems a failing one does.
//!
//! The sentence is for the panel. The exit status, the tail of the log the
//! plist already redirects the process's stderr to, and the readiness message
//! all belong in the app's own log, and are carried separately for that.
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
    Reason(String),
}
impl LastExit {
    /// Whether launchd has seen this service's process go away unsuccessfully.
    pub(super) fn is_failure(&self) -> bool {
        match self {
            Self::Code(code) => *code != 0,
            Self::Reason(_) => true,
            Self::Unknown | Self::NeverExited => false,
        }
    }
    fn describe(&self) -> String {
        match self {
            Self::Unknown => "unknown".into(),
            Self::NeverExited => "never exited".into(),
            Self::Code(code) => format!("exit code {code}"),
            Self::Reason(reason) => format!("exit reason {reason}"),
        }
    }
}

/// `launchctl print` is diagnostic output, and its keys have changed across
/// releases: current macOS prints `last exit code`, older ones the wait(2)
/// encoded `last exit status`, and a kernel-initiated exit prints
/// `last exit reason`. Disagreeing or unparsable lines resolve to `Unknown`,
/// which never accelerates a failure.
pub(super) fn parse_last_exit(text: &str) -> LastExit {
    let mut seen: Option<LastExit> = None;
    for line in text.lines().map(str::trim) {
        let parsed = if let Some(value) = line.strip_prefix("last exit code = ") {
            parse_exit_value(value.trim(), false)
        } else if let Some(value) = line.strip_prefix("last exit status = ") {
            parse_exit_value(value.trim(), true)
        } else if let Some(value) = line.strip_prefix("last exit reason = ") {
            let reason = value.trim();
            if reason.is_empty() || reason.len() > 128 {
                LastExit::Unknown
            } else {
                LastExit::Reason(reason.to_owned())
            }
        } else {
            continue;
        };
        if seen.get_or_insert_with(|| parsed.clone()) != &parsed {
            return LastExit::Unknown;
        }
    }
    seen.unwrap_or(LastExit::Unknown)
}
fn parse_exit_value(value: &str, wait_encoded: bool) -> LastExit {
    if value == "(never exited)" {
        return LastExit::NeverExited;
    }
    let Ok(raw) = value.parse::<i32>() else {
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
    tail: &str,
    port: u16,
    readiness: &str,
) -> StartupFailure {
    let cause = recognise(last_exit, port);
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
        _ => None,
    }
}

/// What the service's own exit code says, or what launchd says when the
/// program never ran to have an opinion.
fn recognise(last_exit: &LastExit, port: u16) -> Option<String> {
    let LastExit::Code(code) = last_exit else {
        // A signal or a kernel-initiated exit: the process was ended, it did
        // not choose a code, and there is nothing of its own to report.
        return None;
    };
    let code = u8::try_from(*code).ok()?;
    // A shell reports 126 for a program it cannot execute and 127 for one it
    // cannot find, and launchd's own spawn failures surface the same way.
    // These are not the server's codes; nothing of ours ran to emit one.
    if matches!(code, 126 | 127) {
        return Some(UNLAUNCHABLE.into());
    }
    CODES
        .codes
        .iter()
        .find(|(_, value)| **value == code)
        .and_then(|(reason, _)| sentence_for(reason, port))
}

#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/startup.rs"]
mod tests;
