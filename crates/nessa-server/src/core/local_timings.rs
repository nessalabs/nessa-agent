//! Durations kept on this machine, and nowhere else.
//!
//! Two files in the stage log directory, beside `host.log` and `gateway.log`:
//! `launch-to-gateway-ready-ms` (the desktop host, process start to gateway
//! ready) and `message-to-first-token-ms` (this gateway, a send accepted to
//! the first reply text in a view). Each line is one duration in
//! milliseconds. `nessa doctor` reads the last [`SHOWN`] of each and prints
//! the latest and the median. Nothing in here is sent off the machine.
//!
//! ```text
//! desktop host ──append──▶ launch-to-gateway-ready-ms ──read──▶ nessa doctor
//! gateway      ──append──▶ message-to-first-token-ms  ──read──▶ nessa doctor
//! ```
//!
//! Arrows are local file writes and the doctor's local read. There is no
//! network hop.

use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// File the desktop host appends. Doctor reads the same name.
pub const LAUNCH_TO_GATEWAY_READY_FILE: &str = "launch-to-gateway-ready-ms";
/// File this gateway appends when a followed turn first shows reply text.
pub const MESSAGE_TO_FIRST_TOKEN_FILE: &str = "message-to-first-token-ms";
/// How many of the most recent samples doctor reports.
pub const SHOWN: usize = 8;
/// How many samples a file keeps, so it does not grow without a bound.
const STORED: usize = 32;

/// The last samples that parsed, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimingSamples {
    values: Vec<u64>,
}

impl TimingSamples {
    fn parse(text: &str, keep: usize) -> Self {
        let values: Vec<u64> = text
            .lines()
            .filter_map(|line| line.trim().parse::<u64>().ok())
            .collect();
        let skip = values.len().saturating_sub(keep);
        Self {
            values: values.into_iter().skip(skip).collect(),
        }
    }

    pub(crate) fn latest(&self) -> Option<u64> {
        self.values.last().copied()
    }

    /// Middle value, or the average of the two middle values when the count
    /// is even. The input order is left as it was.
    pub(crate) fn median_ms(&self) -> Option<f64> {
        if self.values.is_empty() {
            return None;
        }
        let mut sorted = self.values.clone();
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        if sorted.len() % 2 == 1 {
            Some(sorted[middle] as f64)
        } else {
            Some((sorted[middle - 1] as f64 + sorted[middle] as f64) / 2.0)
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }
}

/// Append `ms` and keep the last [`STORED`] integer lines.
pub(crate) fn append_sample(path: &Path, ms: u64) -> io::Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "timing file is a symlink",
        ));
    }
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut samples = TimingSamples::parse(&existing, STORED - 1);
    samples.values.push(ms);
    write_private(path, &samples.values)
}

/// What `nessa doctor` adds under `timings`. Missing files are "none yet".
/// A file that cannot be read is "unreadable" and does not fail the command.
pub fn doctor_timings(log_directory: Option<&Path>) -> Value {
    json!({
        "staysOnThisMachine": true,
        "launchToGatewayReady": series(log_directory, LAUNCH_TO_GATEWAY_READY_FILE),
        "messageToFirstToken": series(log_directory, MESSAGE_TO_FIRST_TOKEN_FILE),
    })
}

fn series(log_directory: Option<&Path>, name: &str) -> Value {
    let Some(directory) = log_directory else {
        return none_yet();
    };
    match read_series(&directory.join(name)) {
        Ok(samples) if samples.values.is_empty() => none_yet(),
        Ok(samples) => json!({
            "latestMs": samples.latest(),
            "medianMs": median_json(samples.median_ms()),
            "count": samples.len(),
        }),
        Err(Read::Missing) => none_yet(),
        Err(Read::Unreadable) => json!({
            "latestMs": Value::Null,
            "medianMs": Value::Null,
            "count": 0,
            "note": "unreadable",
        }),
    }
}

fn none_yet() -> Value {
    json!({
        "latestMs": Value::Null,
        "medianMs": Value::Null,
        "count": 0,
        "note": "none yet",
    })
}

fn median_json(median: Option<f64>) -> Value {
    match median {
        Some(value) if value.fract() == 0.0 && value >= 0.0 => json!(value as u64),
        Some(value) => json!(value),
        None => Value::Null,
    }
}

enum Read {
    Missing,
    Unreadable,
}

fn read_series(path: &Path) -> Result<TimingSamples, Read> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(TimingSamples::parse(&text, SHOWN)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Read::Missing),
        Err(_) => Err(Read::Unreadable),
    }
}

fn write_private(path: &Path, values: &[u64]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temporary)?;
        set_private(&file)?;
        for value in values {
            writeln!(file, "{value}")?;
        }
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    if let Ok(file) = OpenOptions::new().write(true).open(path) {
        let _ = set_private(&file);
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("timings");
    path.with_file_name(format!(".{name}.tmp"))
}

#[cfg(unix)]
fn set_private(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_even_count_averages_the_middle_pair_and_an_odd_count_takes_the_middle() {
        let even = TimingSamples::parse("4\n1\n3\n2\n", 8);
        assert_eq!(even.latest(), Some(2));
        assert_eq!(even.median_ms(), Some(2.5));
        let odd = TimingSamples::parse("9\n1\n5\n", 8);
        assert_eq!(odd.median_ms(), Some(5.0));
        assert_eq!(TimingSamples::parse("nope\n\n  ", 8).median_ms(), None);
    }

    #[test]
    fn doctor_reports_the_last_eight_and_says_when_there_are_none() {
        let directory = tempfile::tempdir().unwrap();
        let launch = directory.path().join(LAUNCH_TO_GATEWAY_READY_FILE);
        for ms in 1..=10 {
            append_sample(&launch, ms).unwrap();
        }
        let report = doctor_timings(Some(directory.path()));
        assert_eq!(report["staysOnThisMachine"], true);
        assert_eq!(report["launchToGatewayReady"]["latestMs"], 10);
        assert_eq!(report["launchToGatewayReady"]["count"], 8);
        assert_eq!(report["launchToGatewayReady"]["medianMs"], 6.5);
        assert!(report["launchToGatewayReady"].get("note").is_none());
        assert_eq!(report["messageToFirstToken"]["note"], "none yet");
        assert_eq!(report["messageToFirstToken"]["count"], 0);

        let stored = fs::read_to_string(&launch).unwrap();
        assert_eq!(stored.lines().count(), STORED.min(10));
        assert!(stored.ends_with("10\n"));
    }

    #[test]
    fn a_file_kept_past_thirty_two_drops_the_oldest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("samples");
        for ms in 1..=40 {
            append_sample(&path, ms).unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        let numbers: Vec<u64> = text.lines().map(|line| line.parse().unwrap()).collect();
        assert_eq!(numbers.first().copied(), Some(9));
        assert_eq!(numbers.last().copied(), Some(40));
        assert_eq!(numbers.len(), STORED);
    }

    #[test]
    fn a_directory_where_the_file_should_be_is_unreadable_and_no_directory_is_none_yet() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join(LAUNCH_TO_GATEWAY_READY_FILE)).unwrap();
        let report = doctor_timings(Some(directory.path()));
        assert_eq!(report["launchToGatewayReady"]["note"], "unreadable");
        assert_eq!(
            doctor_timings(None)["launchToGatewayReady"]["note"],
            "none yet"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_not_appended_to() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("elsewhere");
        fs::write(&target, "1\n").unwrap();
        let link = directory.path().join(MESSAGE_TO_FIRST_TOKEN_FILE);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(append_sample(&link, 5).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "1\n");
    }
}
