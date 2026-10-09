//! `nessa env serve`: compose the environment role from this host's own
//! configuration and serve one gateway over standard input and output.
//!
//! ```text
//! config.json "agents" ──▶ ConfiguredLauncher (workspace, each agent's command)
//! <data>/environment/serve.lock ──▶ ServeLock (one serving process)
//! <data>/environment/leases.jsonl ──▶ FileLedger
//! stdin, stdout ──▶ serve(...) until the gateway's stream ends
//! ```
//!
//! Arrows are construction, then the call that runs it. Whatever stops it
//! from serving is still said in frames after the hello, so a gateway learns
//! this build and why, instead of a stream that just ends.
use super::{agent::launch_environment, runtime_config::RuntimeConfig};
use crate::{
    core::RunError,
    env::{Environment, VERSION},
    env_serve::{
        application::{refuse, serve, ServeTimings},
        infrastructure::{ConfiguredLauncher, FileLedger, LaunchSpec, ServeLock, ServeLockError},
    },
};
use nessa_protocol::{
    agents::AgentId,
    lease::{Hello, Unavailability},
};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

/// How long a new serving process waits for the previous one to finish
/// ending its leases.
const LOCK_WAIT: Duration = Duration::from_secs(15);

/// Serve until the gateway's stream ends.
///
/// # Errors
/// Only what could not even be said to the gateway: its stream failing.
pub(super) async fn execute() -> Result<(), RunError> {
    let unconfigured = Hello {
        build: VERSION.into(),
        workspace: String::new(),
    };
    let composed = match compose().await {
        Ok(composed) => composed,
        Err((reason, failure)) => {
            tracing::error!(%failure, "this host cannot serve leases");
            refuse(tokio::io::stdout(), unconfigured, reason).await?;
            return Ok(());
        }
    };
    let Composed {
        launcher,
        ledger,
        lock,
    } = composed;
    serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        VERSION,
        launcher,
        ledger,
        ServeTimings::default(),
    )
    .await;
    drop(lock);
    Ok(())
}

struct Composed {
    launcher: Arc<ConfiguredLauncher>,
    ledger: Arc<FileLedger>,
    lock: ServeLock,
}

async fn compose() -> Result<Composed, (Unavailability, RunError)> {
    let not_configured = |failure: RunError| (Unavailability::NotConfigured, failure);
    let auth = Environment::auth_directory_from_system()
        .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    let namespace = auth
        .parent()
        .ok_or_else(|| not_configured(RunError::Agent("invalid data directory".into())))?
        .to_path_buf();
    let config = RuntimeConfig::load(&auth).map_err(not_configured)?;
    let launcher = launcher(&config).map_err(not_configured)?;
    let directory = namespace.join("environment");
    nessa_local_storage::create_directory(&directory)
        .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    let lock = match ServeLock::acquire(&directory.join("serve.lock"), LOCK_WAIT).await {
        Ok(lock) => lock,
        Err(ServeLockError::Busy) => {
            return Err((
                Unavailability::Busy,
                RunError::Agent("another nessa env serve holds this data directory".into()),
            ))
        }
        Err(ServeLockError::Io(error)) => {
            return Err(not_configured(RunError::Agent(error.to_string())))
        }
    };
    let ledger = FileLedger::open(directory.join("leases.jsonl"))
        .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    Ok(Composed {
        launcher: Arc::new(launcher),
        ledger: Arc::new(ledger),
        lock,
    })
}

/// The launcher this host's `agents` block describes: its workspace, and
/// for each agent its command with the account variables and credentials a
/// local launch would have.
fn launcher(config: &RuntimeConfig) -> Result<ConfiguredLauncher, RunError> {
    let agents = config
        .agents
        .as_ref()
        .ok_or_else(|| RunError::Agent("config.json configures no agents on this host".into()))?;
    let workspace = absolute(&agents.workspace)?;
    let mut specs = HashMap::new();
    for (agent, runtime) in agents.agents() {
        if agent == AgentId::Opencode {
            continue;
        }
        let executable = runtime.command.executable();
        if !executable.is_absolute() {
            return Err(RunError::Agent(format!(
                "{}: command must be absolute",
                agent.name()
            )));
        }
        specs.insert(
            agent,
            LaunchSpec {
                executable: executable.to_path_buf(),
                args: runtime.args.clone(),
                environment: launch_environment(agent),
            },
        );
    }
    Ok(ConfiguredLauncher::new(workspace, specs))
}

fn absolute(path: &Path) -> Result<String, RunError> {
    match path.to_str() {
        Some(text) if path.is_absolute() => Ok(text.to_owned()),
        _ => Err(RunError::Agent(
            "the agents workspace must be an absolute UTF-8 path".into(),
        )),
    }
}
