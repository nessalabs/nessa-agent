//! Private upgrade exchange and native process effects for the launchd adapter.
use super::startup::{
    diagnose, log_tail, parse_last_exit, recorded_failure, LastExit, RecordedFailure,
};
use crate::gateway::domain::value_objects::RetirementRefusal;
use nessa_local_storage::OpenMode;
use serde::{Deserialize, Deserializer};
use std::{
    fs::File,
    io::{Error, ErrorKind, Read, Write},
    net::TcpStream,
    os::fd::AsRawFd,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Identity advertised by a managed gateway process, bound to launchd's exact PID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gateway::infrastructure) struct ManagedRuntime {
    pub fingerprint: String,
    pub generation: String,
    pub instance: String,
    pub pid: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gateway::infrastructure) enum Health {
    Legacy,
    Managed(ManagedRuntime),
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Registration {
    Unloaded,
    Loaded,
}
pub(in crate::gateway::infrastructure) struct ServiceStatus {
    pub loaded: bool,
    pub pid: Option<u32>,
    pub process_identity_known: bool,
    /// Diagnostic only: what launchd last saw this service's process do. It
    /// decides nothing about identity, and never authorizes an effect.
    pub last_exit: LastExit,
}
/// A loaded label and a responsive port are independent observations.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ServiceState {
    Unloaded,
    ManagedCurrent(ManagedRuntime),
    ManagedStale(ManagedRuntime),
    LegacyExactService,
    ForeignPort,
    UnavailableLoadedService,
}
pub(super) fn classify(
    registration: Registration,
    loaded_pid: Option<u32>,
    definition_matches: bool,
    running: Option<Health>,
    expected: (&str, &str),
    port_occupied: bool,
    legacy_listener: Option<u32>,
) -> ServiceState {
    if matches!(registration, Registration::Unloaded) {
        return if port_occupied {
            ServiceState::ForeignPort
        } else {
            ServiceState::Unloaded
        };
    }
    let Some(pid) = loaded_pid else {
        return ServiceState::UnavailableLoadedService;
    };
    match running {
        Some(Health::Managed(runtime)) if runtime.pid == pid => {
            if runtime.fingerprint == expected.0
                && runtime.generation == expected.1
                && definition_matches
            {
                ServiceState::ManagedCurrent(runtime)
            } else {
                ServiceState::ManagedStale(runtime)
            }
        }
        Some(Health::Legacy) if legacy_listener == Some(pid) => ServiceState::LegacyExactService,
        Some(_) => ServiceState::ForeignPort,
        None => ServiceState::UnavailableLoadedService,
    }
}
/// `print` is diagnostic output: unknown, missing or ambiguous PID syntax fails closed.
pub(super) fn service_status(service: &str) -> Result<ServiceStatus, String> {
    let output = bounded_output(
        Command::new("/bin/launchctl")
            .args(["print", service])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        LAUNCHCTL_DEADLINE,
    )?;
    if !output.status.success() {
        return Ok(ServiceStatus {
            loaded: false,
            pid: None,
            process_identity_known: true,
            last_exit: LastExit::Unknown,
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let process = parse_service_process(&text);
    Ok(ServiceStatus {
        loaded: true,
        pid: process.as_ref().ok().copied().flatten(),
        process_identity_known: process.is_ok(),
        last_exit: parse_last_exit(&text),
    })
}
fn parse_service_process(text: &str) -> Result<Option<u32>, ()> {
    let mut depth = 0usize;
    let mut pid = None;
    let mut saw_root = false;
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("pid = ") {
            if depth != 1 || pid.is_some() {
                return Err(());
            }
            pid = Some(parse_pid(value).ok_or(())?);
        }
        if line.ends_with("{") {
            if depth == 0 {
                if saw_root {
                    return Err(());
                }
                saw_root = true;
            }
            depth = depth.checked_add(1).ok_or(())?;
        }
        if line == "}" {
            depth = depth.checked_sub(1).ok_or(())?;
        }
    }
    if depth == 0 && saw_root {
        Ok(pid)
    } else {
        Err(())
    }
}
fn parse_pid(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok().filter(|pid| *pid > 0)
}
pub(super) fn legacy_listener_pid(port: u16) -> Option<u32> {
    let output = Command::new("/usr/sbin/lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_listener_pid(std::str::from_utf8(&output.stdout).ok()?)
}
fn parse_listener_pid(text: &str) -> Option<u32> {
    let mut pid = None;
    for line in text.lines() {
        let candidate = parse_pid(line.trim())?;
        if pid.is_some_and(|previous| previous != candidate) {
            return None;
        }
        pid = Some(candidate);
    }
    pid
}

/// The reconciliation lock, given up explicitly.
///
/// An `flock` belongs to the open file description, not to the descriptor. A
/// subprocess forked while this one is open — `uuidgen`, `plutil`, `launchctl`,
/// any of the ones reconciliation runs — keeps a duplicate of that description
/// until it execs, because `O_CLOEXEC` closes the descriptor there and not at
/// the fork. Closing ours alone would leave the namespace locked by a child
/// that has no interest in it, so the next reconciliation waits on nothing.
pub(super) struct NamespaceLock(File);
impl std::os::fd::AsRawFd for NamespaceLock {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}
impl Drop for NamespaceLock {
    fn drop(&mut self) {
        if unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) } != 0 {
            eprintln!(
                "[nessa] could not release the gateway reconciliation lock: {}",
                Error::last_os_error()
            );
        }
    }
}
pub(super) fn lock_namespace(data: &Path) -> Result<NamespaceLock, String> {
    let file =
        nessa_local_storage::open(&data.join("gateway-upgrade.lock"), OpenMode::OpenOrCreate)
            .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(NamespaceLock(file));
        }
        let error = Error::last_os_error();
        if error.kind() != ErrorKind::WouldBlock || Instant::now() >= deadline {
            return Err(format!(
                "Cannot acquire gateway reconciliation lock: {error}"
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub(super) fn launchctl(args: &[&str]) -> Result<(), String> {
    let output = bounded_output(
        Command::new("/bin/launchctl")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        LAUNCHCTL_DEADLINE,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "launchctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
/// The longest any one `launchctl` call in a registration may take (ADR 221).
/// A call past it is ended and fails the step it belongs to, so every startup
/// step finishes, one way or the other, without the panel keeping a timer.
pub(super) const LAUNCHCTL_DEADLINE: Duration = Duration::from_secs(30);

/// Why a bounded command gave no output.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BoundedOutputError {
    /// It was still running at its deadline and was ended. What it had already
    /// done is unknown.
    Deadline(Duration),
    /// It could not be run or waited on.
    Run(String),
}
impl std::fmt::Display for BoundedOutputError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deadline(deadline) => write!(
                output,
                "launchctl did not finish within {} s",
                deadline.as_secs()
            ),
            Self::Run(error) => output.write_str(error),
        }
    }
}
impl From<BoundedOutputError> for String {
    fn from(error: BoundedOutputError) -> Self {
        error.to_string()
    }
}

/// Run a `launchctl` command with piped output, ending it at `deadline`.
pub(super) fn bounded_output(
    command: &mut Command,
    deadline: Duration,
) -> Result<Output, BoundedOutputError> {
    let run = |error: String| BoundedOutputError::Run(error);
    let mut child = command.spawn().map_err(|error| run(error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| run("launchctl stdout unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| run("launchctl stderr unavailable".into()))?;
    let stdout = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut output = stdout;
        output.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut output = stderr;
        output.read_to_end(&mut bytes).map(|_| bytes)
    });
    let until = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(run(error.to_string()));
            }
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout.join();
            let _ = stderr.join();
            return Err(BoundedOutputError::Deadline(deadline));
        }
        thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        status,
        stdout: stdout
            .join()
            .map_err(|_| run("launchctl stdout reader panicked".into()))?
            .map_err(|error| run(error.to_string()))?,
        stderr: stderr
            .join()
            .map_err(|_| run("launchctl stderr reader panicked".into()))?
            .map_err(|error| run(error.to_string()))?,
    })
}
pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("Missing parent directory")?;
    // LaunchAgents itself need not be private, but the temporary file always is.
    let output = Command::new("/usr/bin/uuidgen")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not create private file identity".into());
    }
    let suffix = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let temporary = directory.join(format!(".nessa-{}.tmp", suffix.trim()));
    let result = (|| {
        let mut file = nessa_local_storage::open(&temporary, OpenMode::CreateNew)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        nessa_local_storage::replace(&temporary, path).map_err(|e| e.to_string())?;
        nessa_local_storage::sync_directory(directory).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetirementResult {
    request_id: String,
    target_fingerprint: String,
    running_fingerprint: String,
    running_instance: String,
    requested_instance: String,
    running_generation: String,
    requested_running_generation: String,
    target_generation: String,
    retired: bool,
    #[serde(deserialize_with = "required_nullable_error")]
    retirement_request_id: Option<String>,
    #[serde(deserialize_with = "required_nullable_cause")]
    retirement_cause: Option<RetirementCause>,
    #[serde(deserialize_with = "required_nullable_error")]
    cleanup_error: Option<String>,
    #[serde(deserialize_with = "required_nullable_error")]
    audit_error: Option<String>,
    /// Why not, by a published name (ADR 221). Absent from gateways that
    /// predate the names, which read as not confirmed.
    #[serde(default)]
    refusal: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetirementCause {
    principal_id: String,
    surface_id: String,
    request_id: String,
}
/// A recorded retirement cause proves admission was fenced, even when cleanup or
/// audit failed. Only the separately validated successful acknowledgement permits bootout.
#[derive(Debug)]
pub(super) struct RetirementEvidence {
    pub fingerprint: String,
    pub generation: String,
    /// Whether this is the retirement that completed: the old gateway cleaned
    /// up, audited, and said so. A request that has not been answered, and an
    /// admitted retirement whose cleanup or audit failed, are both false.
    /// Admission fencing does not read it — that is what a recorded cause
    /// alone decides — but whether the named runtime can still be in use does.
    pub retired: bool,
}
impl RetirementEvidence {
    pub fn matches(&self, fingerprint: &str, generation: &str) -> bool {
        self.fingerprint == fingerprint && self.generation == generation
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetirementRequest {
    request_id: String,
    target_fingerprint: String,
    running_instance: String,
    running_generation: String,
    target_generation: String,
}
pub(super) fn read_pending_retirement(
    data: &Path,
    running: &ManagedRuntime,
) -> Result<Option<RetirementEvidence>, String> {
    let file = match nessa_local_storage::open(
        &data.join("gateway-upgrade/request.json"),
        OpenMode::ReadNonblocking,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65536 {
        return Err("Pending retirement request exceeds limit; service preserved".into());
    }
    parse_pending_retirement(&bytes, running)
}
fn parse_pending_retirement(
    bytes: &[u8],
    running: &ManagedRuntime,
) -> Result<Option<RetirementEvidence>, String> {
    let request: RetirementRequest = serde_json::from_slice(bytes).map_err(|error| {
        format!("Cannot validate pending retirement request; service preserved: {error}")
    })?;
    if !canonical_uuid(&request.request_id)
        || !canonical_uuid(&request.running_instance)
        || !sha256(&request.target_fingerprint)
        || !sha256(&request.running_generation)
        || !sha256(&request.target_generation)
    {
        return Err("Invalid pending retirement request identity; service preserved".into());
    }
    Ok((request.running_instance == running.instance
        && request.running_generation == running.generation
        && request.target_generation != request.running_generation)
        .then_some(RetirementEvidence {
            fingerprint: running.fingerprint.clone(),
            generation: running.generation.clone(),
            retired: false,
        }))
}
pub(super) fn read_retirement_evidence(data: &Path) -> Result<Option<RetirementEvidence>, String> {
    let file = match nessa_local_storage::open(
        &data.join("gateway-upgrade/result.json"),
        OpenMode::ReadNonblocking,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65536 {
        return Err("Retirement fence exceeds limit; service preserved".into());
    }
    parse_retirement_evidence(&bytes)
}
fn parse_retirement_evidence(bytes: &[u8]) -> Result<Option<RetirementEvidence>, String> {
    let result: RetirementResult = serde_json::from_slice(bytes)
        .map_err(|error| format!("Cannot validate retirement fence; service preserved: {error}"))?;
    let admitted_identity = result.requested_instance == result.running_instance
        && result.requested_running_generation == result.running_generation;
    if !canonical_uuid(&result.request_id)
        || !canonical_uuid(&result.requested_instance)
        || !canonical_uuid(&result.running_instance)
        || !sha256(&result.target_fingerprint)
        || !sha256(&result.running_fingerprint)
        || !sha256(&result.target_generation)
        || !sha256(&result.running_generation)
        || !sha256(&result.requested_running_generation)
        || !valid_retirement_cause(result.retirement_cause.as_ref())
        || result.retirement_request_id.as_deref()
            != result
                .retirement_cause
                .as_ref()
                .map(|cause| cause.request_id.as_str())
        || (result.retirement_cause.is_none() && admitted_identity)
        || (result.retirement_request_id.is_some()
            && (result.requested_instance != result.running_instance
                || result.requested_running_generation != result.running_generation
                || result.target_generation == result.requested_running_generation))
        || result
            .retirement_request_id
            .as_deref()
            .is_some_and(|id| !canonical_uuid(id))
        || result.retired
            && (result.retirement_request_id.is_none()
                || !valid_confirmed_cause(result.retirement_cause.as_ref())
                || result.cleanup_error.is_some()
                || result.audit_error.is_some())
        || (!result.retired && result.cleanup_error.is_none() && result.audit_error.is_none())
        || (result.retired && result.refusal.is_some())
    {
        return Err(
            "Invalid retirement fence identity or acknowledgement; service preserved".into(),
        );
    }
    Ok(result
        .retirement_request_id
        .is_some()
        .then_some(RetirementEvidence {
            fingerprint: result.running_fingerprint,
            generation: result.running_generation,
            retired: result.retired,
        }))
}
fn required_nullable_error<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
fn required_nullable_cause<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RetirementCause>, D::Error> {
    Option::<RetirementCause>::deserialize(deserializer)
}
fn valid_retirement_cause(cause: Option<&RetirementCause>) -> bool {
    cause.is_none_or(|cause| {
        [&cause.principal_id, &cause.surface_id, &cause.request_id]
            .into_iter()
            .all(|value| !value.trim().is_empty() && value.len() <= 256)
            && canonical_uuid(&cause.request_id)
    })
}
fn valid_confirmed_cause(cause: Option<&RetirementCause>) -> bool {
    cause.is_some_and(|cause| {
        cause.principal_id == "gateway" && cause.surface_id == "gateway_upgrade"
    })
}
fn acknowledge(
    bytes: &[u8],
    request: &str,
    target: &str,
    running: &str,
    instance: &str,
    running_generation: &str,
    target_generation: &str,
) -> Result<bool, RetirementFailure> {
    let Ok(result) = serde_json::from_slice::<RetirementResult>(bytes) else {
        return Ok(false);
    };
    if result.request_id != request {
        return Ok(false);
    }
    if result.target_fingerprint != target
        || result.running_fingerprint != running
        || result.running_instance != instance
        || result.requested_instance != instance
        || result.running_generation != running_generation
        || result.requested_running_generation != running_generation
        || result.target_generation != target_generation
        || (result.retirement_request_id.is_some()
            && result.target_generation == result.requested_running_generation)
        || !sha256(&result.running_generation)
        || !sha256(&result.target_generation)
        || !valid_retirement_cause(result.retirement_cause.as_ref())
        || result.retirement_request_id.as_deref()
            != result
                .retirement_cause
                .as_ref()
                .map(|cause| cause.request_id.as_str())
    {
        return Ok(false);
    }
    if result
        .retirement_request_id
        .as_deref()
        .is_some_and(|identity| !canonical_uuid(identity))
        || (result.retired
            && (result.retirement_request_id.is_none()
                || !valid_confirmed_cause(result.retirement_cause.as_ref())))
        || (result.retired && result.refusal.is_some())
    {
        return Ok(false);
    }
    if !result.retired || result.cleanup_error.is_some() || result.audit_error.is_some() {
        return Err(RetirementFailure::Refused {
            refusal: RetirementRefusal::named(result.refusal.as_deref()),
            message: format!(
                "Gateway retirement was not acknowledged: retired={}, cleanup={:?}, audit={:?}",
                result.retired, result.cleanup_error, result.audit_error
            ),
        });
    }
    Ok(true)
}
/// Called under the reconciliation lock, before notifying the running gateway.
fn prepare_request(
    directory: &Path,
    request: &str,
    target: &str,
    instance: &str,
    running_generation: &str,
    target_generation: &str,
) -> Result<(), String> {
    // A successful old result is the server's durable retirement fence. Preserve it.
    let bytes = serde_json::to_vec(&serde_json::json!({"requestId":request,"targetFingerprint":target,"runningInstance":instance,"runningGeneration":running_generation,"targetGeneration":target_generation}))
        .map_err(|error| error.to_string())?;
    atomic_write(&directory.join("request.json"), &bytes)
}
fn read_acknowledgement(
    directory: &Path,
    request: &str,
    target: &str,
    running: &str,
    instance: &str,
    running_generation: &str,
    target_generation: &str,
) -> Result<bool, RetirementFailure> {
    match nessa_local_storage::open(&directory.join("result.json"), OpenMode::ReadNonblocking) {
        Ok(file) => {
            let mut bytes = Vec::new();
            (&file)
                .take(65537)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > 65536 {
                return Ok(false);
            }
            let answer = acknowledge(
                &bytes,
                request,
                target,
                running,
                instance,
                running_generation,
                target_generation,
            );
            if matches!(answer, Ok(false)) {
                return Ok(false);
            }
            // Reading a renamed file does not establish crash durability. Complete
            // both persistence barriers before bootout can rely on this fence,
            // and before a refusal can be acted on (ADR 221): either one may be
            // what lets the host unload the old service.
            file.sync_all()
                .map_err(|error| format!("Cannot persist retirement acknowledgement: {error}"))?;
            nessa_local_storage::sync_directory(directory)
                .map_err(|error| format!("Cannot persist retirement fence directory: {error}"))?;
            answer
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string().into()),
    }
}
/// Why a retirement request did not end in a retired gateway.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RetirementFailure {
    /// The gateway answered this very request, and refused it (ADR 221).
    Refused {
        refusal: RetirementRefusal,
        message: String,
    },
    /// No answer to act on: the request, the signal, or the wait failed.
    Unanswered(String),
}
impl std::fmt::Display for RetirementFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The refusal's name is part of the text the journal records for the
        // failed retirement step, so the durable record says why, not only the
        // plan the host chose next.
        match self {
            Self::Refused { refusal, message } => {
                write!(output, "{message} (refusal: {})", refusal.name())
            }
            Self::Unanswered(message) => output.write_str(message),
        }
    }
}
impl From<String> for RetirementFailure {
    fn from(message: String) -> Self {
        Self::Unanswered(message)
    }
}
impl From<&str> for RetirementFailure {
    fn from(message: &str) -> Self {
        Self::Unanswered(message.to_owned())
    }
}
/// The running gateway asked to retire, and the runtime replacing it.
pub(super) struct Retirement<'a> {
    pub service: &'a str,
    pub target: &'a str,
    pub running: &'a str,
    pub instance: &'a str,
    pub running_generation: &'a str,
    pub target_generation: &'a str,
}

pub(super) fn retire(
    launchctl: &dyn super::Launchctl,
    data: &Path,
    retirement: Retirement<'_>,
) -> Result<(), RetirementFailure> {
    let Retirement {
        service,
        target,
        running,
        instance,
        running_generation,
        target_generation,
    } = retirement;
    let directory = data.join("gateway-upgrade");
    nessa_local_storage::create_directory(&directory).map_err(|e| e.to_string())?;
    let output = Command::new("/usr/bin/uuidgen")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not create retirement request identity".into());
    }
    let request = String::from_utf8(output.stdout)
        .map_err(|e| e.to_string())?
        .trim()
        .to_ascii_lowercase();
    prepare_request(
        &directory,
        &request,
        target,
        instance,
        running_generation,
        target_generation,
    )?;
    // The request is durable, so the signal waits for launchctl to answer.
    match launchctl.signal(service, "SIGUSR2", &mut || {
        std::thread::sleep(Duration::from_millis(10));
        true
    }) {
        super::LifecycleCommandResult::Accepted => {}
        super::LifecycleCommandResult::Rejected(message)
        | super::LifecycleCommandResult::Failed(message)
        | super::LifecycleCommandResult::Indeterminate(message) => {
            return Err(format!("launchctl kill SIGUSR2 {service}: {message}").into())
        }
    }
    let deadline = Instant::now() + Duration::from_secs(75);
    loop {
        if read_acknowledgement(
            &directory,
            &request,
            target,
            running,
            instance,
            running_generation,
            target_generation,
        )? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(
                "Gateway retirement acknowledgement timed out; service was preserved".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub(super) fn health(port: u16) -> Option<Health> {
    let mut stream =
        TcpStream::connect_timeout(&super::address(port), Duration::from_millis(200)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(300)))
        .ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut bytes = Vec::new();
    stream.take(8192).read_to_end(&mut bytes).ok()?;
    parse_health(&bytes)
}
fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
}
fn parse_health(bytes: &[u8]) -> Option<Health> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (headers, _) = text.split_once("\r\n\r\n")?;
    let mut lines = headers.split("\r\n");
    if lines.next()?.split_whitespace().nth(1)? != "200" {
        return None;
    }
    let mut fingerprint = None;
    let mut generation = None;
    let mut instance = None;
    let mut pid = None;
    for line in lines {
        let (key, value) = line.split_once(':')?;
        let slot = if key.eq_ignore_ascii_case("x-nessa-runtime-fingerprint") {
            &mut fingerprint
        } else if key.eq_ignore_ascii_case("x-nessa-service-generation") {
            &mut generation
        } else if key.eq_ignore_ascii_case("x-nessa-runtime-instance") {
            &mut instance
        } else if key.eq_ignore_ascii_case("x-nessa-process-id") {
            &mut pid
        } else {
            continue;
        };
        if slot.replace(value.trim()).is_some() {
            return None;
        }
    }
    match (fingerprint, generation, instance, pid) {
        (None, None, None, None) => Some(Health::Legacy),
        (Some(fingerprint), Some(generation), Some(instance), Some(pid))
            if sha256(fingerprint) && sha256(generation) && canonical_uuid(instance) =>
        {
            Some(Health::Managed(ManagedRuntime {
                fingerprint: fingerprint.into(),
                generation: generation.into(),
                instance: instance.into(),
                pid: parse_pid(pid)?,
            }))
        }
        _ => None,
    }
}
/// How long a healthy-but-slow gateway is allowed to take to answer.
const READINESS_DEADLINE: Duration = Duration::from_secs(75);
/// How often launchd is asked what happened to the process, while the port is
/// still silent. `launchctl print` is a subprocess; the health probe is not.
const LIVENESS_INTERVAL: Duration = Duration::from_millis(500);
/// Consecutive liveness checks that must agree the process exited and was not
/// replaced. One observation can land in the gap between a clean exit and the
/// next spawn; three across a second and a half is a service that is not coming
/// up, well inside launchd's own five-second restart throttle.
const DEAD_OBSERVATIONS: u32 = 3;
const READINESS_FAILURE: &str = "the gateway did not advertise the expected runtime identity owned by its launchd service before the readiness deadline";

/// A failed registration, and whether its message is already the one to show.
///
/// Every mechanism failure here is retryable and says so; a service that exits
/// on startup is not a mechanism failure, and the sentence naming its cause is
/// finished before it leaves this module.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum InstallFailure {
    Reconciliation(String),
    Startup(String),
}
impl From<String> for InstallFailure {
    fn from(message: String) -> Self {
        Self::Reconciliation(message)
    }
}
impl From<&str> for InstallFailure {
    fn from(message: &str) -> Self {
        Self::Reconciliation(message.into())
    }
}

/// What one look at the port and at launchd says about the service being
/// started. Separated from the waiting so the judgement can be tested without
/// a port, a subprocess or a clock, the way `classify` already is.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// The expected runtime answered, owned by the exact process launchd runs.
    Ready(ManagedRuntime),
    /// launchd has positively established that the process is gone and that
    /// its last exit failed.
    Dead,
    /// Anything else, including everything we could not establish.
    Waiting,
}

/// Judge one observation.
///
/// Two things are deliberately not evidence of death. A health response from
/// something that is not the service being started says nothing about that
/// service — another gateway can hold this stage's port, and the per-label
/// registration lock does not exclude it, so its answer must not stop us
/// looking at our own. And a `launchctl print` whose process could not be read
/// is an answer we did not get: `service_status` keeps "no process" and "could
/// not tell" apart for exactly this reason, and only the first is absence.
pub(super) fn assess(
    running: Option<&Health>,
    status: &ServiceStatus,
    expected: (&str, &str),
    gave_up: Option<&RecordedFailure>,
) -> Step {
    if let Some(Health::Managed(runtime)) = running {
        if status.loaded
            && status.process_identity_known
            && status.pid == Some(runtime.pid)
            && runtime.fingerprint == expected.0
            && runtime.generation == expected.1
        {
            return Step::Ready(runtime.clone());
        }
    }
    // A gone process whose last exit failed is the ordinary death. The other
    // one is a server that exited *successfully* on purpose, because that is
    // the only thing launchd reads as "do not start me again" — a zero status
    // proves nothing on its own, so what makes it death is the gateway's own
    // record of giving up, for this exact registration.
    if status.loaded
        && status.process_identity_known
        && status.pid.is_none()
        && (status.last_exit.is_failure()
            || gave_up.is_some_and(|record| record.belongs_to(expected.1)))
    {
        return Step::Dead;
    }
    Step::Waiting
}

/// Consecutive `Dead` observations, and the rule for giving up on them.
///
/// Kept apart from the waiting so the reset is testable without a clock: one
/// observation can land in the gap between a clean exit and the next spawn,
/// and anything that is not death has to start the count again or a service
/// that is restarting normally would eventually accumulate three.
#[derive(Default)]
pub(super) struct DeadCount(u32);
impl DeadCount {
    /// Records one observation and says whether the service should be given up.
    pub(super) fn observe(&mut self, step: &Step) -> bool {
        self.0 = if matches!(step, Step::Dead) {
            self.0 + 1
        } else {
            0
        };
        self.0 >= DEAD_OBSERVATIONS
    }
}

/// How often the port is asked, while it is silent.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The port, launchd, and the passing of time — the three outside things
/// readiness depends on.
///
/// Behind a trait so the waiting can be tested at all. The rules that matter
/// here are about *when*: that a healthy-but-slow start keeps its full
/// deadline, that a crash loop is given up in about a second and a half, and
/// that `launchctl print` — a subprocess — is not run ten times a second.
/// None of that is observable through a function that sleeps and calls the
/// real thing.
pub(super) trait ServiceWatch {
    fn now(&self) -> Instant;
    fn sleep(&mut self, duration: Duration);
    fn health(&mut self) -> Option<Health>;
    fn status(&mut self) -> Result<ServiceStatus, String>;
    /// What the gateway wrote down about giving up, read from beside its log.
    /// A fourth outside thing, and behind the same seam for the same reason.
    fn gave_up(&mut self) -> Option<RecordedFailure>;
}

/// The real one: a loopback probe, `launchctl print`, the system clock, and the
/// gateway's own record beside its log.
struct LaunchdWatch<'a> {
    launchctl: &'a dyn super::Launchctl,
    service: &'a str,
    port: u16,
    logs: &'a Path,
}
impl ServiceWatch for LaunchdWatch<'_> {
    fn gave_up(&mut self) -> Option<RecordedFailure> {
        recorded_failure(self.logs)
    }
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn health(&mut self) -> Option<Health> {
        self.launchctl.health(self.port)
    }
    fn status(&mut self) -> Result<ServiceStatus, String> {
        self.launchctl.status(self.service)
    }
}

/// Wait for the expected runtime to answer, or for launchd to prove it cannot.
///
/// The deadline is for a server that is starting slowly. A server that has
/// exited, and that launchd has not replaced by the time we look again, is not
/// starting slowly, and waiting the rest of the deadline out only delays the
/// same answer by half a minute.
pub(super) fn wait_fingerprint(
    launchctl: &dyn super::Launchctl,
    service: &str,
    expected: (&str, &str),
    port: u16,
    log: &Path,
) -> Result<ManagedRuntime, InstallFailure> {
    // The record the gateway leaves when it gives up sits in the same directory
    // as the log launchd redirects it to, which is the directory this already
    // knows. Derived here so no caller has to learn a second path.
    let logs = log.parent().unwrap_or(Path::new(".")).to_path_buf();
    wait_ready(
        &mut LaunchdWatch {
            launchctl,
            service,
            port,
            logs: &logs,
        },
        expected,
        port,
        log,
    )
}

pub(super) fn wait_ready(
    watch: &mut impl ServiceWatch,
    expected: (&str, &str),
    port: u16,
    log: &Path,
) -> Result<ManagedRuntime, InstallFailure> {
    let deadline = watch.now() + READINESS_DEADLINE;
    let mut next_liveness_check = watch.now();
    let mut dead = DeadCount::default();
    while watch.now() < deadline {
        let running = watch.health();
        // `launchctl print` is a subprocess, so it is asked when there is
        // something to check against it or when a liveness check is due —
        // never on every hundred-millisecond turn of the loop.
        let due = watch.now() >= next_liveness_check;
        if matches!(running, Some(Health::Managed(_))) || due {
            let status = watch.status()?;
            // Only asked for when launchd has no process to show: a running
            // service has nothing to say about having given up, and this is a
            // file read on every liveness check otherwise. A record belonging
            // to another registration is dropped here rather than carried on
            // as something to judge or to say — it is about a service that is
            // not the one being started.
            let gave_up = (status.pid.is_none())
                .then(|| watch.gave_up())
                .flatten()
                .filter(|record| record.belongs_to(expected.1));
            match assess(running.as_ref(), &status, expected, gave_up.as_ref()) {
                Step::Ready(runtime) => return Ok(runtime),
                // An observation that is not due still counts for nothing:
                // the interval is what makes three of these a second and a
                // half of agreement rather than three turns of the loop.
                step if due => {
                    next_liveness_check = watch.now() + LIVENESS_INTERVAL;
                    if dead.observe(&step) {
                        let failure = diagnose(
                            &status.last_exit,
                            gave_up.as_ref(),
                            &log_tail(log),
                            port,
                            READINESS_FAILURE,
                        );
                        eprintln!("[nessa] {}", failure.detail);
                        return Err(InstallFailure::Startup(failure.sentence));
                    }
                }
                _ => {}
            }
        }
        watch.sleep(POLL_INTERVAL);
    }
    // The process outlived the deadline without answering, which is the
    // readiness contract's own failure and stays worded as one. Its log still
    // goes to ours, so the next person does not have to go and find it.
    let tail = log_tail(log);
    if !tail.is_empty() {
        eprintln!("[nessa] {READINESS_FAILURE}\ngateway log tail:\n{tail}");
    }
    Err(InstallFailure::Reconciliation(
        "Gateway did not advertise the expected runtime identity owned by its launchd service before the readiness deadline".into(),
    ))
}
/// Installation failure never authorizes stopping a process or restoring old configuration.
///
/// The forward-recovery clause is advice about the mechanism, and it belongs on
/// the mechanism's failures. A service that will not start is not waiting on a
/// retry of ours, and its sentence is left exactly as it was written.
pub(super) fn forward_recovery<T>(result: Result<T, InstallFailure>) -> Result<T, String> {
    result.map_err(|failure| match failure {
        InstallFailure::Startup(sentence) => sentence,
        InstallFailure::Reconciliation(primary) => format!("{primary}; gateway registration and any loaded process were preserved for forward recovery; retry reconciliation"),
    })
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/control.rs"]
mod tests;
