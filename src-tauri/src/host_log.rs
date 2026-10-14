//! The desktop startup log.
//!
//! A Finder launch keeps no stderr, so the lines that say whether this process
//! started, showed a window, registered its background service, and became
//! ready have to land in a file. That file is `host.log` in the stage data
//! directory's `logs` folder, beside `gateway.log`.
//!
//! The clock starts when [`install`] runs, which `main` does before anything
//! else. Lines written before a directory is open stay in memory and are
//! flushed into the file. Every line is also written to stderr, so a terminal
//! launch still shows it. A file that has reached the cap at open is renamed
//! aside; this run appends to a new `host.log`.
//!
//! ```text
//!   main ──install──▶ stderr
//!                 └── host.log  (default data logs, then the real namespace)
//!   tracing nessa.* ──▶ the same writer
//! ```
//!
//! Arrows are writes. Only targets under `nessa` are recorded, so opening the
//! subscriber does not copy every crate's diagnostics into this file.

use std::{
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use tracing_subscriber::{
    filter::filter_fn, fmt::MakeWriter, layer::SubscriberExt, util::SubscriberInitExt, Layer,
};

/// `host.log` may be this long at open. One launch writes a short record, so
/// this is many failed launches and still small enough to send.
const LIMIT: u64 = 1024 * 1024;
const FILE_NAME: &str = "host.log";
const MAX_PENDING: usize = 256;

static STARTED: OnceLock<Instant> = OnceLock::new();
static WINDOW_AT: Mutex<Option<Duration>> = Mutex::new(None);
static GATEWAY_AT: Mutex<Option<Duration>> = Mutex::new(None);
static ACTIVE: OnceLock<LogWriter> = OnceLock::new();

/// What a cheap look at the executable path can say about the bundle.
///
/// A signed copy opened from `/Applications` is inside an `.app` and is not
/// under `AppTranslocation`. This does not read a signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningFacts {
    pub app_bundle: bool,
    pub app_translocated: bool,
}

/// How far two startup marks are from process start, and from each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupDurations {
    pub since_process_start: Duration,
    pub since_other: Option<Duration>,
}

/// An absolute home directory, or nothing when the value is missing or relative.
pub fn absolute_home(value: Option<&OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_absolute().then_some(path)
}

pub fn signing_facts(executable: &Path) -> SigningFacts {
    let text = executable.to_string_lossy();
    SigningFacts {
        app_bundle: text.contains(".app/Contents/MacOS/")
            || text.contains(".app\\Contents\\MacOS\\"),
        app_translocated: text.contains("/AppTranslocation/")
            || text.contains("\\AppTranslocation\\"),
    }
}

/// `other_at` is when the other mark happened, measured from the same start.
/// A mark that is not in the past contributes no interval.
pub fn startup_durations(since_start: Duration, other_at: Option<Duration>) -> StartupDurations {
    StartupDurations {
        since_process_start: since_start,
        since_other: other_at.map(|other| since_start.saturating_sub(other)),
    }
}

/// Install the subscriber and open `directory` when it is already known.
///
/// Call this once, from `main`, before any other startup record. A second call
/// leaves the first subscriber in place.
pub fn install(directory: Option<&Path>) {
    let _ = STARTED.set(Instant::now());
    let writer = LogWriter::new();
    let open_error = directory.and_then(|directory| writer.open(directory).err());
    let installed = tracing_subscriber::registry()
        .with(nessa_layer(writer.clone()))
        .try_init()
        .is_ok();
    let _ = ACTIVE.set(writer);
    if !installed {
        eprintln!("[nessa] startup log subscriber was already installed");
    }
    if let Some(error) = open_error {
        tracing::warn!(
            target: "nessa.launch",
            %error,
            "could not open the startup log"
        );
    }
}

/// Open `host.log` in `directory`, creating the directory when it is missing.
///
/// The same directory again is a no-op. A different directory receives the
/// lines already written and the previous file says where they continued.
pub fn open(directory: &Path) -> io::Result<PathBuf> {
    match ACTIVE.get() {
        Some(writer) => writer.open(directory),
        None => Err(io::Error::other("startup log is not installed")),
    }
}

pub fn current_path() -> Option<PathBuf> {
    ACTIVE.get().and_then(|writer| writer.path())
}

pub fn flush() {
    if let Some(writer) = ACTIVE.get() {
        writer.flush_file();
    }
}

pub fn since_process_start_ms() -> u64 {
    milliseconds(elapsed())
}

/// A survivable startup failure. The words are the same ones stderr used to get.
pub fn warn(message: impl std::fmt::Display) {
    tracing::warn!(target: "nessa", "{message}");
}

pub fn error(message: impl std::fmt::Display) {
    tracing::error!(target: "nessa", "{message}");
}

/// The first call is the startup mark. Later shows are still recorded.
pub fn window_shown(label: &str) {
    let since_start = elapsed();
    let first = {
        let mut slot = lock(&WINDOW_AT);
        let first = slot.is_none();
        if first {
            *slot = Some(since_start);
        }
        first
    };
    let since_gateway = startup_durations(since_start, *lock(&GATEWAY_AT));
    tracing::info!(
        target: "nessa.window",
        label = %label,
        first,
        since_process_start_ms = milliseconds(since_start),
        "window shown"
    );
    if !first {
        return;
    }
    match since_gateway.since_other {
        Some(delta) => tracing::info!(
            target: "nessa.timing",
            mark = "window_shown",
            label = %label,
            since_process_start_ms = milliseconds(since_start),
            since_gateway_ready_ms = milliseconds(delta),
            "startup timing"
        ),
        None => tracing::info!(
            target: "nessa.timing",
            mark = "window_shown",
            label = %label,
            since_process_start_ms = milliseconds(since_start),
            "startup timing"
        ),
    }
}

/// The first time the gateway phase becomes ready.
pub fn gateway_ready() {
    let since_start = elapsed();
    let first = {
        let mut slot = lock(&GATEWAY_AT);
        let first = slot.is_none();
        if first {
            *slot = Some(since_start);
        }
        first
    };
    tracing::info!(target: "nessa.launch", "gateway startup succeeded");
    if !first {
        return;
    }
    let timing = startup_durations(since_start, *lock(&WINDOW_AT));
    match timing.since_other {
        Some(delta) => tracing::info!(
            target: "nessa.timing",
            mark = "gateway_ready",
            since_process_start_ms = milliseconds(timing.since_process_start),
            since_window_shown_ms = milliseconds(delta),
            "startup timing"
        ),
        None => tracing::info!(
            target: "nessa.timing",
            mark = "gateway_ready",
            since_process_start_ms = milliseconds(timing.since_process_start),
            since_window_shown_ms = "not_shown",
            "startup timing"
        ),
    }
}

fn nessa_target(target: &str) -> bool {
    target == "nessa" || target.starts_with("nessa.")
}

fn nessa_layer<S>(writer: LogWriter) -> impl Layer<S> + Send + Sync
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .with_writer(writer)
        .with_filter(filter_fn(|meta| {
            nessa_target(meta.target()) && *meta.level() <= tracing::Level::INFO
        }))
}

fn elapsed() -> Duration {
    STARTED.get().map(Instant::elapsed).unwrap_or_default()
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone)]
struct LogWriter {
    state: Arc<Mutex<WriterState>>,
}

struct WriterState {
    file: Option<tracing_appender::rolling::RollingFileAppender>,
    pending: Vec<Vec<u8>>,
    dropped: usize,
    path: Option<PathBuf>,
}

impl LogWriter {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(WriterState {
                file: None,
                pending: Vec::new(),
                dropped: 0,
                path: None,
            })),
        }
    }

    fn path(&self) -> Option<PathBuf> {
        lock(&self.state).path.clone()
    }

    fn flush_file(&self) {
        if let Some(file) = lock(&self.state).file.as_mut() {
            let _ = file.flush();
        }
    }

    fn open(&self, directory: &Path) -> io::Result<PathBuf> {
        let path = directory.join(FILE_NAME);
        if lock(&self.state)
            .path
            .as_deref()
            .is_some_and(|current| same_file(current, &path))
        {
            return Ok(path);
        }
        nessa_local_storage::create_directory(directory)?;
        refuse_symlink(&path)?;
        roll(&path, LIMIT)?;
        let mut appender = tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::NEVER)
            .filename_prefix(FILE_NAME)
            .build(directory)
            .map_err(io::Error::other)?;
        set_private(&path)?;

        let mut state = lock(&self.state);
        if state
            .path
            .as_deref()
            .is_some_and(|current| same_file(current, &path))
        {
            return Ok(path);
        }
        if let Some(previous) = state.path.clone() {
            match fs::read(&previous) {
                Ok(earlier) => {
                    let _ = appender.write_all(&earlier);
                }
                Err(error) => {
                    let note = format!(
                        "earlier startup lines stayed in {}: {error}\n",
                        previous.display()
                    );
                    let _ = appender.write_all(note.as_bytes());
                }
            }
            if let Some(old) = state.file.as_mut() {
                let note = format!("startup log continues in {}\n", path.display());
                let _ = old.write_all(note.as_bytes());
                let _ = old.flush();
            }
        }
        take_pending(&mut appender, &mut state);
        let _ = appender.flush();
        state.file = Some(appender);
        state.path = Some(path.clone());
        Ok(path)
    }
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        let mut state = lock(&self.state);
        if let Some(file) = state.file.as_mut() {
            let _ = file.write_all(buf);
            let _ = file.flush();
        } else if state.pending.len() < MAX_PENDING {
            state.pending.push(buf.to_vec());
        } else {
            state.dropped = state.dropped.saturating_add(1);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_file();
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn take_pending(
    file: &mut tracing_appender::rolling::RollingFileAppender,
    state: &mut WriterState,
) {
    for line in state.pending.drain(..) {
        let _ = file.write_all(&line);
    }
    if state.dropped > 0 {
        let note = format!(
            "dropped {} startup lines before the log file opened\n",
            state.dropped
        );
        let _ = file.write_all(note.as_bytes());
        state.dropped = 0;
    }
}

fn same_file(current: &Path, next: &Path) -> bool {
    if current == next {
        return true;
    }
    match (fs::canonicalize(current), fs::canonicalize(next)) {
        (Ok(current), Ok(next)) => current == next,
        _ => false,
    }
}

fn refuse_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "startup log path is a symbolic link",
        )),
        Ok(_) => Ok(()),
    }
}

/// What opening did to a log that had already reached `limit`.
#[derive(Debug, PartialEq, Eq)]
enum Roll {
    Kept,
    Rolled,
}

fn previous_path(log: &Path) -> PathBuf {
    let mut name = log.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    log.with_file_name(name)
}

fn roll(log: &Path, limit: u64) -> io::Result<Roll> {
    let metadata = match fs::symlink_metadata(log) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Roll::Kept),
        Err(error) => return Err(error),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "startup log path is a symbolic link",
        ));
    }
    if !metadata.is_file() || metadata.len() < limit {
        return Ok(Roll::Kept);
    }
    let previous = previous_path(log);
    refuse_symlink(&previous)?;
    let _ = fs::remove_file(&previous);
    fs::rename(log, &previous)?;
    set_private(&previous)?;
    Ok(Roll::Rolled)
}

#[cfg(unix)]
fn set_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn logs(directory: &Path) -> PathBuf {
        directory.join(FILE_NAME)
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn absolute_home_keeps_only_an_absolute_path() {
        assert_eq!(absolute_home(None), None);
        assert_eq!(absolute_home(Some(OsStr::new(""))), None);
        assert_eq!(absolute_home(Some(OsStr::new("relative"))), None);
        let home = if cfg!(windows) {
            r"C:\Users\nessa"
        } else {
            "/Users/nessa"
        };
        assert_eq!(
            absolute_home(Some(OsStr::new(home))),
            Some(PathBuf::from(home))
        );
    }

    #[test]
    fn signing_facts_come_from_the_executable_path() {
        assert_eq!(
            signing_facts(Path::new("/Applications/Nessa.app/Contents/MacOS/Nessa")),
            SigningFacts {
                app_bundle: true,
                app_translocated: false,
            }
        );
        assert_eq!(
            signing_facts(Path::new(
                "/private/var/folders/ab/AppTranslocation/XYZ/d/Nessa.app/Contents/MacOS/Nessa"
            )),
            SigningFacts {
                app_bundle: true,
                app_translocated: true,
            }
        );
        assert_eq!(
            signing_facts(Path::new("/usr/local/bin/nessa")),
            SigningFacts {
                app_bundle: false,
                app_translocated: false,
            }
        );
    }

    #[test]
    fn startup_durations_subtract_an_earlier_mark_and_saturate() {
        let start = Duration::from_millis(1_000);
        assert_eq!(
            startup_durations(start, Some(Duration::from_millis(400))),
            StartupDurations {
                since_process_start: start,
                since_other: Some(Duration::from_millis(600)),
            }
        );
        assert_eq!(
            startup_durations(start, None),
            StartupDurations {
                since_process_start: start,
                since_other: None,
            }
        );
        assert_eq!(
            startup_durations(Duration::from_millis(100), Some(Duration::from_millis(250)))
                .since_other,
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn nessa_records_are_the_nessa_targets_at_info_or_louder() {
        assert!(nessa_target("nessa"));
        assert!(nessa_target("nessa.launch"));
        assert!(!nessa_target("hyper"));
        assert!(!nessa_target("nessabot"));
    }

    #[test]
    fn the_limit_is_the_size_an_open_will_no_longer_leave_alone() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = logs(directory.path());
        for (size, expected) in [
            (0, Roll::Kept),
            (LIMIT - 1, Roll::Kept),
            (LIMIT, Roll::Rolled),
            (LIMIT + 1, Roll::Rolled),
        ] {
            fs::write(&log, vec![b'x'; size as usize]).expect("log contents");
            assert_eq!(
                roll(&log, LIMIT).expect("roll"),
                expected,
                "at {size} bytes"
            );
            if expected == Roll::Rolled {
                assert_eq!(read(&previous_path(&log)).len() as u64, size);
                assert!(!log.exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_previous_file_stays_private() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = logs(directory.path());
        fs::write(&log, vec![b'x'; LIMIT as usize]).expect("log contents");
        assert_eq!(roll(&log, LIMIT).expect("roll"), Roll::Rolled);
        let mode = fs::metadata(previous_path(&log))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_log_is_refused() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("elsewhere");
        fs::write(&target, "keep").expect("target");
        let log = logs(directory.path());
        std::os::unix::fs::symlink(&target, &log).expect("link");
        let error = roll(&log, 1).expect_err("symlink");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(read(&target), "keep");

        let logs = directory.path().join("logs");
        LogWriter::new().open(&logs).expect("create the log");
        fs::remove_file(logs.join(FILE_NAME)).expect("remove log");
        std::os::unix::fs::symlink(&target, logs.join(FILE_NAME)).expect("link");
        let error = LogWriter::new().open(&logs).expect_err("open symlink");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(read(&target), "keep");
    }

    #[test]
    fn lines_written_before_open_land_in_the_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut writer = LogWriter::new();
        writer.write_all(b"process started\n").expect("buffer");
        let path = writer.open(&directory.path().join("logs")).expect("open");
        assert_eq!(path, logs(&directory.path().join("logs")));
        assert!(read(&path).contains("process started\n"));
        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
    }

    #[test]
    fn opening_again_appends_and_a_new_directory_keeps_the_earlier_lines() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut writer = LogWriter::new();
        let first = writer.open(&directory.path().join("one")).expect("first");
        writer.write_all(b"process started\n").expect("write");
        let again = writer.open(&directory.path().join("one")).expect("again");
        assert_eq!(again, first);
        writer.write_all(b"still here\n").expect("append");
        let text = read(&first);
        assert!(text.contains("process started\n"));
        assert!(text.contains("still here\n"));
        assert_eq!(text.matches("process started").count(), 1);

        let second = writer.open(&directory.path().join("two")).expect("second");
        assert!(read(&second).contains("process started\n"));
        assert!(read(&second).contains("still here\n"));
        assert!(read(&first).contains("startup log continues in"));
        assert!(read(&first).contains(&second.display().to_string()));
    }

    #[test]
    fn a_nessa_record_is_timestamped_and_other_targets_stay_out() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let writer = LogWriter::new();
        writer.open(&directory.path().join("logs")).expect("open");
        let subscriber = tracing_subscriber::registry().with(nessa_layer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "nessa.launch", "process started");
            tracing::info!(target: "hyper", "noise");
            tracing::debug!(target: "nessa.launch", "hidden");
        });
        let text = read(&logs(&directory.path().join("logs")));
        assert!(text.contains("process started"), "{text}");
        assert!(text.contains("nessa.launch"), "{text}");
        assert!(
            text.contains("20"),
            "a timestamp should be on the line: {text}"
        );
        assert!(!text.contains("noise"), "{text}");
        assert!(!text.contains("hidden"), "{text}");
    }

    #[test]
    fn the_first_window_and_gateway_marks_are_logged_once() {
        *lock(&WINDOW_AT) = None;
        *lock(&GATEWAY_AT) = None;
        let _ = STARTED.set(Instant::now());
        let directory = tempfile::tempdir().expect("temporary directory");
        let writer = LogWriter::new();
        writer.open(&directory.path().join("logs")).expect("open");
        let subscriber = tracing_subscriber::registry().with(nessa_layer(writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            window_shown("main");
            gateway_ready();
            gateway_ready();
            window_shown("main");
        });
        let text = read(&logs(&directory.path().join("logs")));
        assert_eq!(text.matches("window shown").count(), 2, "{text}");
        assert_eq!(
            text.matches("gateway startup succeeded").count(),
            2,
            "{text}"
        );
        assert_eq!(text.matches("startup timing").count(), 2, "{text}");
        assert!(text.contains("nessa.window"), "{text}");
        assert!(text.contains("nessa.timing"), "{text}");
        assert!(text.contains("window_shown"), "{text}");
        assert!(text.contains("gateway_ready"), "{text}");
        assert!(text.contains("since_process_start_ms"), "{text}");
        assert!(text.contains("since_window_shown_ms"), "{text}");
        *lock(&WINDOW_AT) = None;
        *lock(&GATEWAY_AT) = None;
    }
}
