//! `nessa env serve`: compose the environment role from this host's own
//! configuration and serve one gateway over standard input and output.
//!
//! ```text
//! config.json "agents" ──▶ ConfiguredLauncher (workspace, each agent's command)
//! config.json "envServe.commands" ──▶ ShepherdCommands (in that workspace), or none
//! <data>/environment/serve.lock ──▶ ServeLock (one serving process)
//! <data>/environment/leases.jsonl ──▶ FileLedger
//! <data>/environment/grants.json ──▶ FileGrants (read at every lease)
//! the checked-in lease policy ──▶ CedarLeasePolicy (validated once, asked at every lease)
//! <data>/environment/outbox/, a private temporary directory ──▶ FileOutbox
//! config.json "envServe.artifactsIn" ──▶ <data>/environment/inbox/ ──▶ FileInbox, or none
//! TokioTimer ──▶ what serving's waits are measured on
//! stdin, stdout ──▶ serve(Peer::Ssh, ...) until the gateway's stream ends
//! ```
//!
//! Arrows are construction, then the call that runs it. Whatever stops it
//! from serving is still said in frames after the hello, so a gateway learns
//! this build and why, instead of a stream that just ends.
use super::{agent::launch_environment, runtime_config::RuntimeConfig};
use crate::{
    core::RunError,
    env::{Environment, LEASE_PROTOCOL, VERSION},
    env_serve::{
        application::{
            refuse, serve, ArtifactInbox, CommandRunner, HarnessLauncher, LeasePolicyEvaluator,
            Runners, ServeTimings, Serving,
        },
        domain::Peer,
        infrastructure::{
            CedarLeasePolicy, ConfiguredLauncher, FileGrants, FileInbox, FileLedger, FileOutbox,
            LaunchSpec, ServeLock, ServeLockError, ShepherdCommands, COMMAND_VARIABLES,
        },
    },
    timer::{Timer, TokioTimer},
};
use nessa_protocol::lease::{Hello, Unavailability};
use shepherd::{ProcessSupervisor, SupervisorBuilder};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
};

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
        protocol: LEASE_PROTOCOL.into(),
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
    // Over standard input and output: whoever logged in over SSH.
    // Over SSH the session's own end is the stop: nothing asks a drain.
    composed
        .serve(
            Peer::Ssh,
            tokio::io::stdin(),
            tokio::io::stdout(),
            watch::channel(false).1,
        )
        .await;
    composed.finish().await;
    Ok(())
}

/// What serving needs on this host, held for as long as it serves: one
/// connection's leases at a time, over standard input and output or over
/// each connection a paired environment dials.
pub(super) struct Composed {
    launcher: Arc<ConfiguredLauncher>,
    /// How commands run here, and what supervises them; `None` when this
    /// host does not run them.
    commands: Option<(Arc<ShepherdCommands>, ProcessSupervisor)>,
    ledger: Arc<FileLedger>,
    /// What this host's owner grants each peer.
    grants: Arc<FileGrants>,
    /// What decides each lease under those grants.
    policy: Arc<dyn LeasePolicyEvaluator>,
    outbox: Arc<FileOutbox>,
    /// Where fetched files arrive; `None` when this host takes none.
    inbox: Option<Arc<FileInbox>>,
    /// The publish points' directory, removed when serving ends.
    sockets: std::path::PathBuf,
    lock: ServeLock,
    timer: Arc<dyn Timer>,
}

impl Composed {
    /// Serve one gateway's leases, `peer`'s, on `input` and `output` until
    /// that stream ends, or until `drain` has ended every lease.
    pub(super) async fn serve<R, W>(
        &self,
        peer: Peer,
        input: R,
        output: W,
        drain: watch::Receiver<bool>,
    ) where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let runner = self
            .commands
            .as_ref()
            .map(|(runner, _)| runner.clone() as Arc<dyn CommandRunner>);
        serve(
            input,
            output,
            VERSION,
            LEASE_PROTOCOL,
            Serving {
                peer,
                grants: self.grants.clone(),
                policy: self.policy.clone(),
            },
            Runners {
                harnesses: self.launcher.clone(),
                commands: runner,
            },
            self.ledger.clone(),
            self.outbox.clone(),
            self.inbox
                .clone()
                .map(|inbox| inbox as Arc<dyn ArtifactInbox>),
            ServeTimings::default(),
            self.timer.clone(),
            drain,
        )
        .await;
    }

    /// Stop serving on this host: what is left of commands is swept, the
    /// publish points removed, and the lock given back.
    pub(super) async fn finish(self) {
        // Every command's end is recorded before serving returns; the sweep
        // is what is left of any of them, and only said.
        if let Some((_, supervisor)) = self.commands {
            match supervisor.shutdown().await {
                Ok(report) if report.scopes.iter().all(|scope| scope.all_verified()) => {}
                Ok(_) => tracing::error!("a command's processes could not be confirmed stopped"),
                Err(error) => tracing::error!(%error, "the command supervisor did not shut down"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.sockets);
        drop(self.lock);
    }
}

pub(super) async fn compose() -> Result<Composed, (Unavailability, RunError)> {
    let not_configured = |failure: RunError| (Unavailability::NotConfigured, failure);
    let auth = Environment::auth_directory_from_system()
        .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    let config = RuntimeConfig::load(&auth).map_err(not_configured)?;
    let mut launcher = launcher(&config).map_err(not_configured)?;
    let opencode_database = auth.join("environment").join("opencode-sessions.sqlite");
    nessa_local_storage::create_directory(&auth.join("environment"))
        .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    launcher.opencode_database(opencode_database);
    let policy = CedarLeasePolicy::new().map_err(|error| not_configured(RunError::Agent(error)))?;
    let commands = config.env_serve.commands.then(|| {
        let supervisor = SupervisorBuilder::new().build();
        let environment = COMMAND_VARIABLES
            .into_iter()
            .filter_map(|key| std::env::var_os(key).map(|value| (key.into(), value)))
            .collect();
        let runner =
            ShepherdCommands::new(supervisor.clone(), launcher.workspace().into(), environment);
        (Arc::new(runner), supervisor)
    });
    let directory = environment_directory().map_err(not_configured)?;
    let lock = match ServeLock::acquire(&directory.join("serve.lock"), LOCK_WAIT).await {
        Ok(lock) => lock,
        Err(ServeLockError::Busy) => {
            return Err((
                Unavailability::Busy,
                RunError::Agent(
                    "another nessa env serve or nessa env dial holds this data directory".into(),
                ),
            ))
        }
        Err(ServeLockError::Io(error)) => {
            return Err(not_configured(RunError::Agent(error.to_string())))
        }
    };
    let ledger = FileLedger::open(directory.join("leases.jsonl"))
        .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    // Short whatever the data directory is called: a socket's path is
    // bounded (104 bytes on macOS), and the temporary directory is this
    // user's.
    let sockets = std::env::temp_dir().join(format!(
        "nessa-env-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let outbox = FileOutbox::open(
        directory.join("outbox"),
        sockets.clone(),
        Path::new(launcher.workspace()),
    )
    .map_err(|error| not_configured(RunError::Agent(error.to_string())))?;
    let inbox = match config.env_serve.artifacts_in {
        true => Some(Arc::new(
            FileInbox::open(directory.join("inbox"), Path::new(launcher.workspace()))
                .map_err(|error| not_configured(RunError::Agent(error.to_string())))?,
        )),
        false => None,
    };
    Ok(Composed {
        launcher: Arc::new(launcher),
        commands,
        ledger: Arc::new(ledger),
        grants: Arc::new(FileGrants::new(&directory)),
        policy: Arc::new(policy),
        outbox: Arc::new(outbox),
        inbox,
        sockets,
        lock,
        timer: Arc::new(TokioTimer::new()),
    })
}

/// `<data>/environment`, created when absent: where this host keeps what
/// serving leases needs, and a paired environment its pairing.
pub(super) fn environment_directory() -> Result<std::path::PathBuf, RunError> {
    let auth = Environment::auth_directory_from_system()
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let directory = auth
        .parent()
        .ok_or_else(|| RunError::Agent("invalid data directory".into()))?
        .join("environment");
    nessa_local_storage::create_directory(&directory)
        .map_err(|error| RunError::Agent(error.to_string()))?;
    Ok(directory)
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
                args: runtime
                    .staged_runtime
                    .iter()
                    .cloned()
                    .chain(runtime.args.iter().cloned())
                    .collect(),
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
