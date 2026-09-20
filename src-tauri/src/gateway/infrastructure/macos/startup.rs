//! Why a launchd-managed gateway is not starting, in one sentence for a person.
//!
//! Readiness has a deadline because a healthy server can be slow. A server that
//! exits on startup is not slow, and waiting out its deadline reports the
//! readiness contract ("did not advertise the expected runtime identity")
//! instead of the reason. Two pieces of evidence already exist at that moment:
//! what `launchctl print` says the service's last exit was, and the tail of the
//! log the plist itself redirects the process's stderr to. This module reads
//! both and names the causes it can recognise.
//!
//! The sentence is for the panel. Everything the sentence was inferred from —
//! the exit status, the log tail, the readiness message — belongs in the app's
//! own log, and is carried separately for that purpose.
use nessa_local_storage::OpenMode;
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
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

/// Name the cause when the evidence names it, and say plainly that we cannot
/// when it does not. An unrecognised failure still carries its log tail to the
/// app log, which is the whole of what diagnosing this by hand recovered.
pub(super) fn diagnose(
    last_exit: &LastExit,
    tail: &str,
    port: u16,
    readiness: &str,
) -> StartupFailure {
    let cause = recognise(last_exit, tail, port);
    let sentence = match &cause {
        Some(cause) => format!("Nessa's background service is not starting: {cause}"),
        None if tail.is_empty() => {
            "Nessa's background service is not starting, and it exited without reporting why."
                .into()
        }
        None => "Nessa's background service is not starting.".into(),
    };
    let mut detail = format!(
        "gateway did not start: launchd reports {}; inferred cause: {}; {readiness}",
        last_exit.describe(),
        cause.as_deref().unwrap_or("none recognised")
    );
    if !tail.is_empty() {
        detail.push_str("\ngateway log tail:\n");
        detail.push_str(tail);
    }
    StartupFailure { sentence, detail }
}
/// The server's own fatal messages are the vocabulary here — `RunError`'s
/// `Display` is what reaches this log — plus the exec failures that happen
/// before the server has a chance to say anything at all.
fn recognise(last_exit: &LastExit, tail: &str, port: u16) -> Option<String> {
    let tail = tail.to_ascii_lowercase();
    // `LocalStoreError` says "invalid" for a registry whose contents this build
    // cannot make sense of, which is the schema version among other things. The
    // rest of that family — locked, not initialized — is still the registry,
    // and saying so is better than naming a version that is not the problem.
    if tail.contains("credential registry is invalid") {
        return Some("its credential registry is not one this version of Nessa can read.".into());
    }
    if tail.contains("credential registry") {
        return Some("its credential registry could not be read.".into());
    }
    if tail.contains("port already in use") || tail.contains("address already in use") {
        return Some(format!("port {port} is already in use."));
    }
    if exec_failed(last_exit, &tail) {
        return Some("its background program could not be launched.".into());
    }
    None
}
/// A shell reports 126 for a program it cannot execute and 127 for one it
/// cannot find, and launchd's spawn failures surface the same way; dyld and the
/// code-signing kill say so in the log before any of our own code runs.
fn exec_failed(last_exit: &LastExit, lowercased_tail: &str) -> bool {
    matches!(last_exit, LastExit::Code(126 | 127))
        || lowercased_tail.contains("dyld")
        || lowercased_tail.contains("library not loaded")
        || lowercased_tail.contains("code signature")
        // A missing file is only evidence about the program itself when the
        // program never got far enough to report a failure of its own.
        || (!lowercased_tail.contains("nessa failed")
            && lowercased_tail.contains("no such file or directory"))
}

#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/startup.rs"]
mod tests;
