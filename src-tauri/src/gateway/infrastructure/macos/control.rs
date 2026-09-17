//! Private upgrade exchange and native process effects for the launchd adapter.
use nessa_local_storage::OpenMode;
use serde::{Deserialize, Deserializer};
use std::{
    fs::File,
    io::{Error, ErrorKind, Read, Write},
    net::TcpStream,
    os::fd::AsRawFd,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

/// Identity advertised by a managed gateway process, bound to launchd's exact PID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ManagedRuntime {
    pub fingerprint: String,
    pub generation: String,
    pub instance: String,
    pub pid: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Health {
    Legacy,
    Managed(ManagedRuntime),
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Registration {
    Unloaded,
    Loaded,
}
pub(super) struct ServiceStatus {
    pub loaded: bool,
    pub pid: Option<u32>,
    pub process_identity_known: bool,
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
    let output = Command::new("/bin/launchctl")
        .args(["print", service])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(ServiceStatus {
            loaded: false,
            pid: None,
            process_identity_known: true,
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let process = parse_service_process(&text);
    Ok(ServiceStatus {
        loaded: true,
        pid: process.as_ref().ok().copied().flatten(),
        process_identity_known: process.is_ok(),
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
pub(super) fn legacy_listener_pid() -> Option<u32> {
    let output = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-iTCP:7420", "-sTCP:LISTEN", "-t"])
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

pub(super) fn lock_namespace(data: &Path) -> Result<File, String> {
    let file =
        nessa_local_storage::open(&data.join("gateway-upgrade.lock"), OpenMode::OpenOrCreate)
            .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
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
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
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
) -> Result<bool, String> {
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
    {
        return Ok(false);
    }
    if !result.retired || result.cleanup_error.is_some() || result.audit_error.is_some() {
        return Err(format!(
            "Gateway retirement was not acknowledged: retired={}, cleanup={:?}, audit={:?}",
            result.retired, result.cleanup_error, result.audit_error
        ));
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
) -> Result<bool, String> {
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
            if !acknowledge(
                &bytes,
                request,
                target,
                running,
                instance,
                running_generation,
                target_generation,
            )? {
                return Ok(false);
            }
            // Reading a renamed file does not establish crash durability. Complete
            // both persistence barriers before bootout can rely on this fence.
            file.sync_all()
                .map_err(|error| format!("Cannot persist retirement acknowledgement: {error}"))?;
            nessa_local_storage::sync_directory(directory)
                .map_err(|error| format!("Cannot persist retirement fence directory: {error}"))?;
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}
pub(super) fn retire(
    data: &Path,
    service: &str,
    target: &str,
    running: &str,
    instance: &str,
    running_generation: &str,
    target_generation: &str,
) -> Result<(), String> {
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
    launchctl(&["kill", "SIGUSR2", service])?;
    let deadline = Instant::now() + Duration::from_secs(45);
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
pub(super) fn health() -> Option<Health> {
    let mut stream =
        TcpStream::connect_timeout(&super::address(), Duration::from_millis(200)).ok()?;
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
pub(super) fn wait_fingerprint(
    service: &str,
    expected: (&str, &str),
) -> Result<ManagedRuntime, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(Health::Managed(runtime)) = health() {
            let status = service_status(service)?;
            if status.loaded
                && status.pid == Some(runtime.pid)
                && runtime.fingerprint == expected.0
                && runtime.generation == expected.1
            {
                return Ok(runtime);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Gateway did not advertise the expected runtime identity owned by its launchd service before the readiness deadline".into())
}
/// Installation failure never authorizes stopping a process or restoring old configuration.
pub(super) fn forward_recovery<T>(result: Result<T, String>) -> Result<T, String> {
    result.map_err(|primary| format!("{primary}; gateway registration and any loaded process were preserved for forward recovery; retry reconciliation"))
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/control.rs"]
mod tests;
