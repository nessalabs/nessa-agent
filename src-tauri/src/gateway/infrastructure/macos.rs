//! launchd registration and loopback readiness. Service lifetime belongs to launchd.
use crate::gateway::application::{GatewayError, GatewayHost, ReconciledGateway};
use crate::gateway::domain::value_objects::SearchPath;
use nessa_local_storage::OpenMode;
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

mod control;
mod generation;
mod install_attempt;
mod pruning;
mod staging;
mod startup;
use control::{
    classify, forward_recovery, health, launchctl, legacy_listener_pid, lock_namespace,
    read_pending_retirement, read_retirement_evidence, retire, service_status, wait_fingerprint,
    Health, InstallFailure, ManagedRuntime, Registration, ServiceState,
};
use generation::service_generation;
use install_attempt::{
    authorizes_rebootstrap, clear as clear_install_attempt, publish as publish_install_attempt,
};
use pruning::{prune_runtimes, retained_runtimes};
use staging::{launch_settings, stage_runtime};

pub(super) struct Launchd;
impl GatewayHost for Launchd {
    fn register(
        &self,
        runtime: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
    ) -> Result<ReconciledGateway, GatewayError> {
        register(runtime, stage, agent_path).map_err(GatewayError::Registration)
    }
    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
        let status = service_status(gateway.service()).map_err(GatewayError::Stop)?;
        let running = health(gateway.port());
        if !matches_reconciled_gateway(gateway, &status, running.as_ref()) {
            return Err(GatewayError::Stop(
                "Gateway runtime identity changed; no agent stop request was sent".into(),
            ));
        }
        // Address the revalidated registered service, never a PID discovered by port scanning.
        let result = Command::new("/bin/launchctl")
            .args(["kill", "SIGUSR1", gateway.service()])
            .output()
            .map_err(|error| GatewayError::Stop(error.to_string()))?;
        if result.status.success() {
            Ok(())
        } else {
            Err(GatewayError::Stop(
                String::from_utf8_lossy(&result.stderr).trim().to_owned(),
            ))
        }
    }
}

fn matches_reconciled_gateway(
    gateway: &ReconciledGateway,
    status: &control::ServiceStatus,
    health: Option<&Health>,
) -> bool {
    matches!(
        health,
        Some(Health::Managed(runtime))
            if status.loaded
                && status.process_identity_known
                && status.pid == Some(gateway.process_id())
                && runtime.pid == gateway.process_id()
                && runtime.fingerprint == gateway.runtime_fingerprint()
                && runtime.instance == gateway.runtime_instance()
                && runtime.generation == gateway.service_generation()
    )
}
fn register(
    runtime: &Path,
    stage: &str,
    agent_path: Option<&SearchPath>,
) -> Result<ReconciledGateway, String> {
    let location = runtime.to_string_lossy();
    if location.starts_with("/Volumes/") || location.contains("/AppTranslocation/") {
        return Err("Move Nessa to Applications before starting its background service".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is required")?);
    let instance = std::env::var("NESSA_INSTANCE").ok();
    for value in std::iter::once(stage).chain(instance.as_deref()) {
        if value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err("Invalid gateway namespace".into());
        }
    }
    let base = std::env::var_os("NESSA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".nessa"));
    if !base.is_absolute() {
        return Err("NESSA_DATA_DIR must be absolute".into());
    }
    // One table decides where a stage listens. A packaged build registers the
    // `prod` service and keeps 7420; a stage with no entry gets no service at
    // all rather than silently taking the product's socket.
    let port = crate::stage_port::stage_port(stage)
        .ok_or_else(|| format!("No gateway port is defined for stage {stage}"))?;
    let data = prepare_data_directory(&base, stage, instance.as_deref())?;
    let log = data.join("logs/gateway.log");
    let label = format!(
        "so.nessa.gateway.{stage}{}",
        instance
            .as_ref()
            .map(|s| format!(".{s}"))
            .unwrap_or_default()
    );
    let lock_directory = home
        .join("Library/Application Support/Nessa/gateway-locks")
        .join(&label);
    nessa_local_storage::create_directory(&lock_directory).map_err(|e| e.to_string())?;
    let _lock = lock_namespace(&lock_directory)?;
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/{label}");
    let status = service_status(&service)?;
    let loaded = status.loaded;
    let loaded_pid = status.pid;
    let process_identity_known = status.process_identity_known;
    let fingerprint = runtime_fingerprint(runtime)?;
    let private_root = home.join("Library/Application Support/Nessa");
    nessa_local_storage::create_directory(&private_root).map_err(|error| error.to_string())?;
    nessa_local_storage::sync_directory(private_root.parent().ok_or("Missing runtime ancestor")?)
        .map_err(|error| error.to_string())?;
    let runtime_root = private_root.join("gateway-runtimes");
    nessa_local_storage::create_directory(&runtime_root).map_err(|error| error.to_string())?;
    nessa_local_storage::sync_directory(&private_root).map_err(|error| error.to_string())?;
    let installations = runtime_root.join(&label);
    let staged_runtime = stage_runtime(runtime, &installations, &fingerprint)?;
    let runtime = staged_runtime.as_path();
    let arguments = launch_settings(runtime);
    let agents = home.join("Library/LaunchAgents");
    let path = agents.join(format!("{label}.plist"));
    let installed = read_definition(&path).ok();
    let agent_path = registered_agent_path(agent_path, installed.as_ref(), runtime);
    let mut environment = serde_json::Map::new();
    for (key, value) in [
        ("HOME", home.to_string_lossy().into_owned()),
        ("NESSA_STAGE", stage.into()),
        ("NESSA_HOST", "127.0.0.1".into()),
        ("NESSA_PORT", port.to_string()),
        // The gateway's own path: the system tools and nothing else, because
        // everything else it runs it addresses absolutely.
        ("PATH", SearchPath::system().as_str().to_owned()),
        // The agent's, which is a different question with a different answer —
        // see `registered_agent_path`. It is deliberately not the service
        // `PATH`: what the gateway can reach and what the user's agent can
        // reach are not the same decision, and nothing should be able to widen
        // one by widening the other.
        ("NESSA_AGENT_PATH", agent_path.as_str().to_owned()),
        ("NESSA_RUNTIME_FINGERPRINT", fingerprint.clone()),
    ] {
        environment.insert(key.into(), value.into());
    }
    for key in [
        "NESSA_DATA_DIR",
        "NESSA_INSTANCE",
        "CLAUDE_CONFIG_DIR",
        "USER",
        "LOGNAME",
        "TMPDIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            environment.insert(key.into(), value.into());
        }
    }
    // `KeepAlive: true` restarts the service whatever it did, so a server that
    // could never start was relaunched every five seconds for as long as the
    // user was logged in. `SuccessfulExit: false` restarts it only when the
    // process ended unsuccessfully, which still covers every crash and every
    // failure the server thinks retrying can fix — those keep their non-zero
    // exit code from `protocol/defaults/gateway-exit-codes.json`. A failure
    // retrying cannot fix exits zero on purpose and is left alone, with its
    // reason in `logs/gateway-startup-failure.json` for this host to read.
    //
    // Verified against launchd on macOS 26 (Darwin 25.6) rather than assumed:
    // a job exiting 0 under this dictionary runs once and stops, one exiting 1
    // is respawned every ThrottleInterval, and one killed by SIGSEGV is
    // respawned too — launchd prints no `last exit code` for that at all, only
    // the terminating signal.
    let mut definition = serde_json::json!({
        "Label":label, "ProgramArguments":arguments,
        "WorkingDirectory":data, "EnvironmentVariables": environment, "RunAtLoad":true,
        "KeepAlive":{"SuccessfulExit":false},
        "ThrottleInterval":5,"ExitTimeOut":30,"ProcessType":"Background",
        "StandardOutPath":log,"StandardErrorPath":log
    });
    let installed_data = installed
        .as_ref()
        .and_then(|definition| definition.get("WorkingDirectory"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    // Where the loaded registration writes its log, and beside it the record it
    // leaves when it stops for good. A definition we cannot read leaves only
    // this host's own namespace to look in.
    let installed_logs = installed_data
        .clone()
        .unwrap_or_else(|| data.clone())
        .join("logs");
    let fence = match installed_data.as_deref() {
        Some(data) => read_retirement_evidence(data)?,
        None => None,
    };
    let running = if loaded { health(port) } else { None };
    let pending = match (&running, installed_data.as_deref()) {
        (Some(Health::Managed(runtime)), Some(data)) => read_pending_retirement(data, runtime)?,
        _ => None,
    };
    let mut generation = service_generation(&definition, installed.as_ref(), fence.as_ref())?;
    if pending
        .as_ref()
        .is_some_and(|pending| pending.matches(&fingerprint, &generation))
    {
        generation = service_generation(&definition, None, pending.as_ref())?;
    }
    definition["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] =
        Value::String(generation.clone());
    let running_fenced = matches!(&running, Some(Health::Managed(runtime)) if fence.as_ref().is_some_and(|fence| fence.matches(&runtime.fingerprint, &runtime.generation)) || pending.as_ref().is_some_and(|pending| pending.matches(&runtime.fingerprint, &runtime.generation)));
    let listener = if matches!(running, Some(Health::Legacy)) {
        legacy_listener_pid(port)
    } else {
        None
    };
    let registration = if loaded {
        Registration::Loaded
    } else {
        Registration::Unloaded
    };
    let state = classify(
        registration,
        loaded_pid,
        loaded && !running_fenced && service_matches(&path, &definition),
        running,
        (&fingerprint, &generation),
        TcpStream::connect_timeout(&address(port), Duration::from_millis(200)).is_ok(),
        listener,
    );
    match state {
        ServiceState::ManagedCurrent(running) => {
            clear_install_attempt(&lock_directory)?;
            // The loaded service already advertises the staged runtime, so the
            // versions nothing can be running are known here too. Collecting
            // only after a replacement would leave an ordinary launch holding
            // whatever the last update left behind until the next one.
            prune_runtimes(
                &installations,
                &retained_runtimes(
                    &fingerprint,
                    &running.fingerprint,
                    pending.as_ref(),
                    fence.as_ref(),
                ),
            );
            return Ok(ReconciledGateway::new(
                service,
                running.fingerprint,
                running.instance,
                running.generation,
                running.pid,
                port,
            ));
        }
        ServiceState::ManagedStale(running) => {
            let old_definition = read_definition(&path)?;
            let old_data = old_definition
                .get("WorkingDirectory")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or("Loaded definition has no absolute data namespace")?;
            retire(
                &old_data,
                &service,
                &fingerprint,
                &running.fingerprint,
                &running.instance,
                &running.generation,
                &generation,
            )?;
            launchctl(&["bootout", &service])?;
            clear_install_attempt(&lock_directory)?;
        }
        ServiceState::LegacyExactService => {
            // This exact pre-upgrade registration has no retirement protocol.
            // SIGTERM cancels its active agents; never send it SIGUSR2.
            eprintln!("[nessa] Retiring legacy gateway {service}; active agents will be stopped by server shutdown");
            launchctl(&["bootout", &service])?;
            clear_install_attempt(&lock_directory)?;
        }
        ServiceState::ForeignPort => {
            return Err(format!(
                "Port {port} is occupied by an unmanaged process; no service was stopped"
            ))
        }
        ServiceState::UnavailableLoadedService => {
            // A service that gave up is loaded with no process and will not be
            // restarted by launchd, so nothing but this host will ever start it
            // again — and whatever it gave up over may well have been fixed
            // since. Its own record, for the registration that is actually
            // installed, is what says so.
            let recorded = startup::recorded_failure(&installed_logs).filter(|record| {
                installed_generation(installed.as_ref())
                    .is_some_and(|generation| record.belongs_to(generation))
            });
            if !incomplete_install_retry(
                loaded_pid,
                process_identity_known,
                authorizes_rebootstrap(&lock_directory, &service, &definition, installed.as_ref())?,
            ) && !gave_up_retry(loaded_pid, process_identity_known, recorded.is_some())
            {
                return Err(unavailable_service(recorded.as_ref(), port));
            }
            if let Some(recorded) = &recorded {
                // What is being replaced, why, and on whose say-so, in the log
                // of the process doing it. The registration has no running
                // process to retire and no conversations to stop.
                eprintln!(
                    "[nessa] Replacing gateway {service}, which launchd will not start again: {}",
                    recorded.describe()
                );
            }
            launchctl(&["bootout", &service])?;
            if recorded.is_some() {
                startup::forget_recorded_failure(&installed_logs);
            }
            clear_install_attempt(&lock_directory)?;
        }
        ServiceState::Unloaded => clear_install_attempt(&lock_directory)?,
    }
    let installation = (|| -> Result<ManagedRuntime, InstallFailure> {
        std::fs::create_dir_all(&agents).map_err(|e| e.to_string())?;
        let logs = log.parent().ok_or("invalid log directory")?;
        nessa_local_storage::create_directory(logs).map_err(|e| e.to_string())?;
        // Reserve the log privately before launchd opens it.
        let _ =
            nessa_local_storage::open(&log, OpenMode::OpenOrCreate).map_err(|e| e.to_string())?;
        let next = agents.join(format!(".{label}.{}.plist", std::process::id()));
        let mut file =
            nessa_local_storage::open(&next, OpenMode::OpenOrCreate).map_err(|e| e.to_string())?;
        file.set_len(0).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(&definition).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let converted = Command::new("/usr/bin/plutil")
            .args(["-convert", "xml1"])
            .arg(&next)
            .output()
            .map_err(|e| e.to_string())?;
        if !converted.status.success() {
            let _ = fs::remove_file(&next);
            return Err("Could not write gateway service definition".into());
        }
        nessa_local_storage::open(&next, OpenMode::Read)
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        if let Err(error) = fs::rename(&next, &path) {
            return Err(error.to_string().into());
        }
        nessa_local_storage::sync_directory(&agents).map_err(|e| e.to_string())?;
        publish_install_attempt(&lock_directory, &service, &definition)?;
        let bootstrap = Command::new("/bin/launchctl")
            .args(["bootstrap", &domain])
            .arg(&path)
            .output()
            .map_err(|error| error.to_string())
            .and_then(|output| {
                output.status.success().then_some(()).ok_or_else(|| {
                    format!(
                        "Could not register gateway: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                })
            });
        finish_bootstrap(
            bootstrap,
            || service_status(&service).map(|status| status.loaded),
            || clear_install_attempt(&lock_directory),
        )?;
        let running = wait_fingerprint(&service, (&fingerprint, &generation), port, &log)?;
        clear_install_attempt(&lock_directory)?;
        Ok(running)
    })();
    let running = forward_recovery(installation)?;
    // Only past `forward_recovery`: an installation that failed leaves the old
    // registration, and possibly an old process, alive for a retry.
    prune_runtimes(
        &installations,
        &retained_runtimes(
            &fingerprint,
            &running.fingerprint,
            pending.as_ref(),
            fence.as_ref(),
        ),
    );
    Ok(ReconciledGateway::new(
        service,
        running.fingerprint,
        running.instance,
        running.generation,
        running.pid,
        port,
    ))
}
fn prepare_data_directory(
    trusted_base: &Path,
    stage: &str,
    instance: Option<&str>,
) -> Result<PathBuf, String> {
    nessa_local_storage::create_directory(trusted_base).map_err(|error| error.to_string())?;
    let mut relative = PathBuf::new();
    if stage != "prod" {
        relative.push(stage);
    }
    if let Some(instance) = instance {
        relative.push("instances");
        relative.push(instance);
    }
    if relative.as_os_str().is_empty() {
        return Ok(trusted_base.to_path_buf());
    }
    nessa_local_storage::create_directory_beneath(trusted_base, &relative)
        .map_err(|error| error.to_string())?;
    Ok(trusted_base.join(relative))
}
#[derive(Deserialize)]
struct RuntimeManifest {
    fingerprint: String,
}
fn runtime_fingerprint(runtime: &Path) -> Result<String, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(runtime.join("manifest.json"))
        .map_err(|_| "Missing or unsafe runtime manifest")?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Runtime manifest must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65536 {
        return Err("Runtime manifest exceeds limit".into());
    }
    let manifest: RuntimeManifest =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid runtime manifest")?;
    if manifest.fingerprint.len() != 64
        || !manifest
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("Invalid runtime fingerprint".into());
    }
    Ok(manifest.fingerprint)
}
/// The search path this registration will give the agent.
///
/// Three sources, in order of authority:
///
/// 1. what the login shell said this launch;
/// 2. failing that, what the installed service definition already says — the
///    same answer this host wrote the last time a login shell answered. This is
///    the case the equality check cares about: a definition that changed is a
///    definition that retires the running gateway and bootstraps a replacement,
///    so a profile that was slow once must not be a reason to restart a healthy
///    service with a narrower path than it already had;
/// 3. failing that — no shell, no prior registration — the system path, which
///    is a working `PATH` with none of the user's tools on it.
///
/// The staged runtime comes out of all three. It is where Nessa's own `node`
/// lives, and an agent that finds that one by name is running a Node the user
/// did not choose.
fn registered_agent_path(
    resolved: Option<&SearchPath>,
    installed: Option<&Value>,
    runtime: &Path,
) -> SearchPath {
    let registered = installed
        .and_then(|definition| definition.get("EnvironmentVariables"))
        .and_then(|environment| environment.get("NESSA_AGENT_PATH"))
        .and_then(Value::as_str)
        .and_then(|path| SearchPath::parse(path).ok());
    resolved
        .cloned()
        .or(registered)
        .and_then(|path| path.excluding(runtime))
        .unwrap_or_else(SearchPath::system)
}
fn service_matches(path: &Path, expected: &Value) -> bool {
    read_definition(path).is_ok_and(|actual| actual == *expected)
}
fn read_definition(path: &Path) -> Result<Value, String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Cannot read registered gateway definition".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}
/// Where a gateway on `port` answers. Loopback only; the service never binds wider.
fn address(port: u16) -> SocketAddr {
    ([127, 0, 0, 1], port).into()
}
fn incomplete_install_retry(
    loaded_pid: Option<u32>,
    process_identity_known: bool,
    host_attempt_matches: bool,
) -> bool {
    process_identity_known && loaded_pid.is_none() && host_attempt_matches
}

/// Whether a loaded service that gave up may be replaced with a fresh attempt.
///
/// launchd will not start it again — that is what giving up means — so it would
/// otherwise stay loaded and dead forever, including after someone has repaired
/// the thing it gave up over. Booting out a registration with no process of its
/// own destroys nothing; what has to be established is that there is no process,
/// unambiguously, and that the record is this registration's own. The same
/// shape as `incomplete_install_retry`, from different evidence.
fn gave_up_retry(
    loaded_pid: Option<u32>,
    process_identity_known: bool,
    recorded_for_this_registration: bool,
) -> bool {
    process_identity_known && loaded_pid.is_none() && recorded_for_this_registration
}

/// The generation the installed definition registered, if it names one.
fn installed_generation(installed: Option<&Value>) -> Option<&str> {
    installed?
        .get("EnvironmentVariables")?
        .get("NESSA_SERVICE_GENERATION")?
        .as_str()
}

/// What a loaded service that is not answering is reported as. A gateway that
/// said why it stopped is quoted; "no valid health response" is what is left
/// when nothing said anything.
fn unavailable_service(recorded: Option<&startup::RecordedFailure>, port: u16) -> String {
    match recorded.and_then(|record| record.sentence(port)) {
        Some(cause) => {
            format!(
                "Nessa's background service is not starting: {cause} The service was preserved."
            )
        }
        None => "Loaded gateway has no valid health response; service was preserved".into(),
    }
}
fn finish_bootstrap(
    bootstrap: Result<(), String>,
    loaded_after_failure: impl FnOnce() -> Result<bool, String>,
    clear_attempt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let Err(error) = bootstrap else {
        return Ok(());
    };
    match loaded_after_failure() {
        Ok(false) => match clear_attempt() {
            Ok(()) => Err(format!(
                "{error}; launchd reports no loaded service and the install attempt was cleared"
            )),
            Err(clear) => Err(format!(
                "{error}; launchd reports no loaded service, but the install attempt could not be cleared: {clear}"
            )),
        },
        Ok(true) => Err(format!(
            "{error}; launchd reports a loaded service and the install attempt was preserved for verified retry"
        )),
        Err(status) => Err(format!(
            "{error}; loaded service state could not be verified and the install attempt was preserved: {status}"
        )),
    }
}
#[cfg(test)]
mod tests {
    use super::{
        finish_bootstrap, gave_up_retry, incomplete_install_retry, installed_generation,
        matches_reconciled_gateway, prepare_data_directory, registered_agent_path,
        runtime_fingerprint, service_matches, startup, unavailable_service, SearchPath,
    };
    use crate::gateway::application::ReconciledGateway;
    use crate::gateway::infrastructure::macos::control::{Health, ManagedRuntime, ServiceStatus};
    use crate::gateway::infrastructure::macos::startup::LastExit;
    use serde_json::{json, Value};
    use std::{
        cell::Cell,
        fs,
        path::{Path, PathBuf},
    };

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nessa-gateway-{name}-{}", std::process::id()))
    }

    #[test]
    fn stop_authority_requires_the_same_live_runtime_incarnation() {
        let gateway = ReconciledGateway::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "550e8400-e29b-41d4-a716-446655440000".into(),
            "b".repeat(64),
            42,
            7420,
        );
        let status = ServiceStatus {
            loaded: true,
            pid: Some(42),
            process_identity_known: true,
            last_exit: LastExit::NeverExited,
        };
        let current = Health::Managed(ManagedRuntime {
            fingerprint: "a".repeat(64),
            generation: "b".repeat(64),
            instance: "550e8400-e29b-41d4-a716-446655440000".into(),
            pid: 42,
        });
        assert!(matches_reconciled_gateway(
            &gateway,
            &status,
            Some(&current)
        ));

        for replacement in [
            ManagedRuntime {
                fingerprint: "c".repeat(64),
                generation: "b".repeat(64),
                instance: "550e8400-e29b-41d4-a716-446655440000".into(),
                pid: 42,
            },
            ManagedRuntime {
                fingerprint: "a".repeat(64),
                generation: "d".repeat(64),
                instance: "550e8400-e29b-41d4-a716-446655440000".into(),
                pid: 42,
            },
            ManagedRuntime {
                fingerprint: "a".repeat(64),
                generation: "b".repeat(64),
                instance: "660e8400-e29b-41d4-a716-446655440000".into(),
                pid: 42,
            },
            ManagedRuntime {
                fingerprint: "a".repeat(64),
                generation: "b".repeat(64),
                instance: "550e8400-e29b-41d4-a716-446655440000".into(),
                pid: 43,
            },
        ] {
            assert!(!matches_reconciled_gateway(
                &gateway,
                &status,
                Some(&Health::Managed(replacement))
            ));
        }
        assert!(!matches_reconciled_gateway(
            &gateway,
            &ServiceStatus {
                loaded: true,
                pid: Some(43),
                process_identity_known: true,
                last_exit: LastExit::NeverExited,
            },
            Some(&current)
        ));
        assert!(!matches_reconciled_gateway(
            &gateway,
            &ServiceStatus {
                loaded: true,
                pid: Some(42),
                process_identity_known: false,
                last_exit: LastExit::NeverExited,
            },
            Some(&current)
        ));
        assert!(!matches_reconciled_gateway(
            &gateway,
            &status,
            Some(&Health::Legacy)
        ));
    }

    #[test]
    fn non_production_data_rejects_a_symlinked_stage_without_touching_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = temporary_directory("symlink-root");
        let outside = temporary_directory("symlink-target");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let base = root.join(".nessa");
        nessa_local_storage::create_directory(&base).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&outside, base.join("dev")).unwrap();

        assert!(prepare_data_directory(&base, "dev", Some("worktree")).is_err());
        assert!(!outside.join("instances/worktree").exists());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn runtime_manifest_requires_a_sha256_fingerprint() {
        let directory = temporary_directory("manifest");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("manifest.json"),
            r#"{"fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert_eq!(runtime_fingerprint(&directory).unwrap(), "a".repeat(64));
        fs::write(
            directory.join("manifest.json"),
            r#"{"fingerprint":"not-a-digest"}"#,
        )
        .unwrap();
        assert_eq!(
            runtime_fingerprint(&directory),
            Err("Invalid runtime fingerprint".into())
        );
        fs::write(
            directory.join("manifest.json"),
            format!(r#"{{"fingerprint":"{}"}}"#, "A".repeat(64)),
        )
        .unwrap();
        assert_eq!(
            runtime_fingerprint(&directory),
            Err("Invalid runtime fingerprint".into())
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn incomplete_install_retry_requires_no_process_and_exact_host_authority() {
        assert!(incomplete_install_retry(None, true, true));
        assert!(!incomplete_install_retry(None, true, false));
        assert!(!incomplete_install_retry(None, false, true));
        assert!(!incomplete_install_retry(Some(42), true, true));
        assert!(!incomplete_install_retry(Some(42), false, true));
    }

    /// A service that gave up is loaded, has no process, and launchd will
    /// never start it again — so this host is the only thing that can, and the
    /// cause may well have been repaired since. It may boot out only what it
    /// can establish has no process of its own and wrote the record itself.
    #[test]
    fn a_service_that_gave_up_may_be_replaced_only_on_its_own_unambiguous_absence() {
        assert!(gave_up_retry(None, true, true));
        // No record, or one belonging to another registration.
        assert!(!gave_up_retry(None, true, false));
        // An answer we did not get is not absence.
        assert!(!gave_up_retry(None, false, true));
        // Something is running under this label; a record is not licence to
        // boot it out.
        assert!(!gave_up_retry(Some(42), true, true));
        assert!(!gave_up_retry(Some(42), false, true));
    }

    /// The generation is read from the definition that is actually installed,
    /// which is what makes a record this registration's own rather than
    /// whatever ran in this directory before it.
    #[test]
    fn the_installed_generation_comes_from_the_installed_definition() {
        let generation = "a".repeat(64);
        let installed = json!({"EnvironmentVariables": {"NESSA_SERVICE_GENERATION": generation}});
        assert_eq!(installed_generation(Some(&installed)), Some(&*generation));
        assert_eq!(installed_generation(None), None);
        for incomplete in [
            json!({}),
            json!({"EnvironmentVariables": {}}),
            json!({"EnvironmentVariables": {"NESSA_SERVICE_GENERATION": 7}}),
        ] {
            assert_eq!(installed_generation(Some(&incomplete)), None);
        }
    }

    /// The generic sentence said nothing about why. A gateway that recorded a
    /// reason is quoted instead, and a service that recorded nothing still
    /// gets the only honest answer there is.
    #[test]
    fn an_unavailable_service_says_why_when_the_gateway_said_why() {
        let recorded = startup::parse_record(
            json!({
                "reason": "credentialRegistryInvalid",
                "exitCode": 28,
                "message": "authentication setup failed: credential registry is invalid",
                "serviceGeneration": "a".repeat(64),
                "processId": 4711,
            })
            .to_string()
            .as_bytes(),
        )
        .expect("record");
        let said = unavailable_service(Some(&recorded), 7420);
        assert!(
            said.contains("credential registry is not one this version of Nessa can read"),
            "{said}"
        );
        assert!(said.contains("preserved"), "{said}");
        assert_eq!(
            unavailable_service(None, 7420),
            "Loaded gateway has no valid health response; service was preserved"
        );
    }

    #[test]
    fn bootstrap_failure_clears_evidence_only_when_launchd_is_unloaded() {
        let called = Cell::new(false);
        assert_eq!(
            finish_bootstrap(
                Ok(()),
                || Ok(false),
                || {
                    called.set(true);
                    Ok(())
                }
            ),
            Ok(())
        );
        assert!(!called.get());

        let error = finish_bootstrap(
            Err("bootstrap failed".into()),
            || Ok(false),
            || {
                called.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(called.get());
        assert!(error.contains("install attempt was cleared"));

        called.set(false);
        let error = finish_bootstrap(
            Err("bootstrap failed".into()),
            || Ok(true),
            || {
                called.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(!called.get());
        assert!(error.contains("preserved for verified retry"));

        let error = finish_bootstrap(
            Err("bootstrap failed".into()),
            || Err("ambiguous status".into()),
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("could not be verified"));
        assert!(error.contains("ambiguous status"));
    }

    #[test]
    fn registered_service_must_match_runtime_path_and_content() {
        let directory = temporary_directory("plist");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let plist = directory.join("gateway.plist");
        fs::write(
            &plist,
            r#"{"ProgramArguments":["/Applications/Nessa.app/runtime/nessa","server","--desktop-runtime","/Applications/Nessa.app/runtime"],"WorkingDirectory":"/Users/me/.nessa","EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":"current","HOME":"/Users/me","NESSA_STAGE":"prod","NESSA_HOST":"127.0.0.1","NESSA_PORT":"7420","PATH":"/usr/bin:/bin:/usr/sbin:/sbin","NESSA_AGENT_PATH":"/opt/homebrew/bin:/usr/bin:/bin"}}"#,
        )
        .unwrap();
        let expected: Value = serde_json::from_slice(&fs::read(&plist).unwrap()).unwrap();
        assert!(service_matches(&plist, &expected));
        for (key, replacement) in [
            ("KeepAlive", json!(false)),
            ("WorkingDirectory", json!("/Users/me/other")),
            (
                "ProgramArguments",
                json!(["/Applications/Other.app/runtime/nessa"]),
            ),
        ] {
            let mut changed = expected.clone();
            changed[key] = replacement;
            assert!(!service_matches(&plist, &changed));
        }
        let mut changed = expected.clone();
        changed["EnvironmentVariables"]["CLAUDE_CONFIG_DIR"] = json!("/new/provider");
        assert!(!service_matches(&plist, &changed));
        // The agent's path is part of the service definition, so changing it is
        // a re-registration and not something that quietly takes effect.
        let mut changed = expected.clone();
        changed["EnvironmentVariables"]["NESSA_AGENT_PATH"] = json!("/opt/homebrew/bin:/usr/bin");
        assert!(!service_matches(&plist, &changed));
        fs::remove_dir_all(directory).unwrap();
    }

    /// The two ends of one variable, which no type connects: this host writes
    /// it into the service definition and the gateway reads it out of its own
    /// environment. Renaming it on one side alone leaves an agent quietly back
    /// on the system path, which is the failure this whole change is about.
    #[test]
    fn the_gateway_reads_the_agent_path_variable_this_host_writes() {
        let gateway = include_str!("../../../../crates/nessa-server/src/composition/agent.rs");
        assert!(
            include_str!("macos.rs").contains(r#"("NESSA_AGENT_PATH", agent_path"#),
            "this host no longer registers NESSA_AGENT_PATH"
        );
        assert!(
            gateway.contains(r#"var_os("NESSA_AGENT_PATH")"#),
            "crates/nessa-server/src/composition/agent.rs does not read NESSA_AGENT_PATH"
        );
    }

    /// The whole point of the retained path: two launches of the same app, the
    /// second with a login shell that did not answer, produce the same service
    /// definition — so the equality check above holds and the running gateway
    /// is left alone.
    #[test]
    fn an_unread_login_shell_keeps_the_registered_agent_path() {
        let runtime = Path::new("/Users/me/Library/Application Support/Nessa/runtimes/abc");
        let installed = json!({
            "EnvironmentVariables": {"NESSA_AGENT_PATH": "/opt/homebrew/bin:/usr/bin:/bin"}
        });
        let resolved = SearchPath::parse("/opt/homebrew/bin:/usr/bin:/bin").unwrap();
        assert_eq!(
            registered_agent_path(Some(&resolved), None, runtime),
            resolved
        );
        assert_eq!(
            registered_agent_path(None, Some(&installed), runtime),
            resolved
        );
    }

    /// A first launch with no login shell to read, and a definition that has
    /// nothing usable to keep, still leaves the agent a working path.
    #[test]
    fn nothing_to_resolve_and_nothing_registered_falls_back_to_the_system_path() {
        let runtime = Path::new("/staged/runtime");
        for installed in [
            None,
            Some(json!({})),
            Some(json!({"EnvironmentVariables": {}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": ""}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": "relative:./bin"}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": 7}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": "/staged/runtime"}})),
        ] {
            assert_eq!(
                registered_agent_path(None, installed.as_ref(), runtime),
                SearchPath::system(),
                "{installed:?}"
            );
        }
    }

    /// Nessa's own `node`, `nessa` and `nessa-mcp` live in the staged runtime.
    /// Whichever source the path came from, that directory is not on it.
    #[test]
    fn the_staged_runtime_never_reaches_the_agents_path() {
        let runtime = Path::new("/staged/runtime");
        let resolved = SearchPath::parse("/staged/runtime:/opt/homebrew/bin:/usr/bin").unwrap();
        assert_eq!(
            registered_agent_path(Some(&resolved), None, runtime).as_str(),
            "/opt/homebrew/bin:/usr/bin"
        );
        let installed = json!({
            "EnvironmentVariables": {"NESSA_AGENT_PATH": "/usr/bin:/staged/runtime"}
        });
        assert_eq!(
            registered_agent_path(None, Some(&installed), runtime).as_str(),
            "/usr/bin"
        );
    }
}
