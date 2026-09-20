//! launchd registration and loopback readiness. Service lifetime belongs to launchd.
use crate::gateway::application::{GatewayError, GatewayHost, ReconciledGateway};
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
use staging::{launch_settings, stage_runtime};

pub(super) struct Launchd;
impl GatewayHost for Launchd {
    fn register(&self, runtime: &Path, stage: &str) -> Result<ReconciledGateway, GatewayError> {
        register(runtime, stage).map_err(GatewayError::Registration)
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
fn register(runtime: &Path, stage: &str) -> Result<ReconciledGateway, String> {
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
    let staged_runtime = stage_runtime(runtime, &runtime_root.join(&label), &fingerprint)?;
    let runtime = staged_runtime.as_path();
    let (arguments, executable_path) = launch_settings(runtime);
    let agents = home.join("Library/LaunchAgents");
    let path = agents.join(format!("{label}.plist"));
    let mut environment = serde_json::Map::new();
    for (key, value) in [
        ("HOME", home.to_string_lossy().into_owned()),
        ("NESSA_STAGE", stage.into()),
        ("NESSA_HOST", "127.0.0.1".into()),
        ("NESSA_PORT", port.to_string()),
        ("PATH", executable_path),
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
    let mut definition = serde_json::json!({
        "Label":label, "ProgramArguments":arguments,
        "WorkingDirectory":data, "EnvironmentVariables": environment, "RunAtLoad":true,"KeepAlive":true,
        "ThrottleInterval":5,"ExitTimeOut":30,"ProcessType":"Background",
        "StandardOutPath":log,"StandardErrorPath":log
    });
    let installed = read_definition(&path).ok();
    let installed_data = installed
        .as_ref()
        .and_then(|definition| definition.get("WorkingDirectory"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
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
            if !incomplete_install_retry(
                loaded_pid,
                process_identity_known,
                authorizes_rebootstrap(&lock_directory, &service, &definition, installed.as_ref())?,
            ) {
                return Err(
                    "Loaded gateway has no valid health response; service was preserved".into(),
                );
            }
            launchctl(&["bootout", &service])?;
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
        finish_bootstrap, incomplete_install_retry, matches_reconciled_gateway,
        prepare_data_directory, runtime_fingerprint, service_matches,
    };
    use crate::gateway::application::ReconciledGateway;
    use crate::gateway::infrastructure::macos::control::{Health, ManagedRuntime, ServiceStatus};
    use crate::gateway::infrastructure::macos::startup::LastExit;
    use serde_json::{json, Value};
    use std::{cell::Cell, fs, path::PathBuf};

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
            r#"{"ProgramArguments":["/Applications/Nessa.app/runtime/nessa","server","--desktop-runtime","/Applications/Nessa.app/runtime"],"WorkingDirectory":"/Users/me/.nessa","EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":"current","HOME":"/Users/me","NESSA_STAGE":"prod","NESSA_HOST":"127.0.0.1","NESSA_PORT":"7420","PATH":"/runtime:/usr/bin"}}"#,
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
        fs::remove_dir_all(directory).unwrap();
    }
}
