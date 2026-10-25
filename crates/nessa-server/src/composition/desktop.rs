//! Desktop composition: relocatable application resources plus private user data.
//!
//! The credentials this app needs are created by
//! [`super::provisioning::ensure_local_credentials`], which the developer loop
//! asks for by the same name; nothing here provisions separately.
use super::{
    agent::{AgentRuntime, AgentsConfig},
    runtime_config::RuntimeConfig,
};
use crate::mcp_servers::domain::{
    ConfiguredMcpServer, StdioServer, StoredMcpServer, MANAGED_SERVER_NAME,
};
use crate::{core::RunError, desktop_runtime::domain::RunningRuntime};
use nessa_protocol::agents::AgentId;
use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The agent the desktop starts with when nothing else has been chosen.
///
/// Which agent a caller that names none runs on: Claude, because that is the
/// agent Nessa shipped with and the one every conversation already on disk
/// belongs to.
const DEFAULT_AGENT: AgentId = AgentId::Claude;

/// How the desktop launches a bundled JavaScript adapter, relative to the
/// resource root. Managed composition supplies its native runtime separately.
///
/// A command and its arguments, so an agent that speaks ACP through the bundled
/// Node runtime and one that ships as its own executable are both sayable here.
/// Claude and Codex use the bundled Node adapters.
///
/// Opencode is not bundled. It is a whole runtime of its own — nearly two
/// hundred megabytes, against a few for a Node adapter — and shipping it would
/// put that in every download for the people who already have an agent. It is
/// meant to be fetched onto the machine that wants it instead, which is why
/// this says nothing about where it lives.
///
/// `None` means only that the desktop does not ship the agent. Composition
/// resolves a managed launch separately from the installed-runtime store.
pub(super) fn bundled_launch(agent: AgentId) -> Option<(&'static str, &'static str)> {
    match agent {
        AgentId::Claude => Some((
            "node",
            "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
        )),
        AgentId::Codex => Some((
            "node",
            "codex-acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
        )),
        AgentId::Opencode => None,
    }
}

/// The model each agent runs until someone configures another.
///
/// A starting point rather than a recommendation: each is a current model from
/// the bundled catalog that the agent's own harness can reach, and `config.json`
/// replaces it without rebuilding.
fn default_model(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "claude-sonnet-5",
        AgentId::Codex => "gpt-5.6-terra",
        AgentId::Opencode => unreachable!("OpenCode is not a bundled runtime"),
    }
}

pub(super) fn configure(
    settings: &mut RuntimeConfig,
    bundle: &Path,
    data: &Path,
) -> Result<(), RunError> {
    if !bundle.is_absolute() {
        return Err(unusable_runtime("runtime directory must be absolute"));
    }
    let catalog = bundle.join("models.json");
    let mcp = bundle.join("nessa-mcp");
    // Only the adapters this desktop ships. A bundle checked for files it was
    // never meant to contain would refuse to start, so an agent with no bundled
    // launch is skipped here rather than looked for. Current-agent composition
    // independently decides whether that agent has a verified launch on this
    // host.
    let launches: Vec<(AgentId, PathBuf, PathBuf)> = AgentId::ALL
        .iter()
        .filter_map(|agent| {
            let (command, entry) = bundled_launch(*agent)?;
            Some((*agent, bundle.join(command), bundle.join(entry)))
        })
        .collect();
    for path in [&catalog, &mcp].into_iter().chain(
        launches
            .iter()
            .flat_map(|(_, command, entry)| [command, entry]),
    ) {
        if !path.is_file() {
            return Err(unusable_runtime(format!(
                "missing bundled runtime file: {}",
                path.display()
            )));
        }
    }
    // The default workspace is made whenever it is the one configured, not
    // only when nothing is: the first `mcpServers` write on a fresh install
    // stores the `agents` block from these running values, and the next
    // start must not depend on that block being absent
    // (`the_default_workspace_is_made_whenever_it_is_the_one_configured`).
    // Making it again is a no-op; a workspace chosen elsewhere is left alone.
    let relative_workspace = Path::new("workspaces/default");
    let default_workspace = data.join(relative_workspace);
    if settings
        .agents
        .as_ref()
        .is_none_or(|agents| agents.workspace == default_workspace)
    {
        nessa_local_storage::create_directory_beneath(data, relative_workspace).map_err(failure)?;
    }
    if settings.agents.is_none() {
        settings.agents = Some(AgentsConfig {
            catalog: catalog.clone(),
            workspace: default_workspace,
            mcp_servers: vec![],
            #[cfg(unix)]
            stand_ins: Default::default(),
            #[cfg(unix)]
            open_credentials: Default::default(),
            #[cfg(unix)]
            mcp_stand_ins: Default::default(),
            selected: None,
            runtimes: HashMap::new(),
        });
    }
    let agents = settings.agents.as_mut().expect("agents configured above");
    // Whichever agent the existing configuration already answered for stays the
    // default. Filling in the agent it did not mention must not silently move a
    // running installation onto a different agent, so the one agent an existing
    // configuration named becomes the stated choice before the rest are added.
    //
    // Nessa's own default is written only into a configuration that names no
    // agent at all — a first run. Where someone has configured several agents
    // and left `selected` out, that is the same unanswered question the gateway
    // refuses to guess at, and it is refused here too: picking for them would
    // send every conversation to a vendor they never chose, and the desktop is
    // where almost nobody would ever be told.
    let selected =
        agents
            .selected
            .take()
            .or_else(|| match (agents.agents().as_slice(), agents.unknown()) {
                ([], None) => Some(DEFAULT_AGENT.name().into()),
                ([(only, _)], None) => Some(only.name().into()),
                _ => None,
            });
    agents.selected = selected;
    agents.catalog = catalog;
    for (id, command, entry) in launches {
        let staged = vec![entry.to_string_lossy().into_owned()];
        agents
            .runtimes
            .entry(id.name().into())
            // Only the bundled launch is replaced. A configured model and its
            // budgets are the user's and survive every upgrade. The adapter
            // entry is staged-runtime provenance, not an argument, and this
            // desktop launch's arguments are empty.
            .and_modify(|runtime| {
                runtime.command = ExecutableUseSnapshot::unmanaged(command.clone());
                runtime.staged_runtime = staged.clone();
                runtime.args.clear();
            })
            .or_insert_with(|| AgentRuntime {
                command: ExecutableUseSnapshot::unmanaged(command),
                staged_runtime: staged,
                args: Vec::new(),
                model: default_model(id).into(),
                tools_enabled: true,
                context_tokens: 100_000,
                output_tokens: 4096,
            });
    }
    // Only the Nessa-owned server is replaced, in memory: `config.json` never
    // gains it (`composed_settings_publish_privately_under_the_lock_and_audit_without_values`).
    // User-configured MCP servers retain their settings.
    agents.mcp_servers.retain(|server| !server.managed());
    let managed = StdioServer::new(
        MANAGED_SERVER_NAME,
        mcp,
        vec![
            "--workspace".into(),
            agents.workspace.to_string_lossy().into_owned(),
            "--audit-directory".into(),
            data.join("process-audit").to_string_lossy().into_owned(),
        ],
    );
    // No variables of its own, so none is named twice.
    agents.mcp_servers.push(StoredMcpServer::Stdio(
        ConfiguredMcpServer::new(managed, true, [])
            .expect("a server with no variables names none twice"),
    ));
    Ok(())
}

fn failure(error: impl std::fmt::Display) -> RunError {
    RunError::Agent(error.to_string())
}

/// A prepared runtime tree that is not there, cannot be read, or is not the one
/// this registration was fingerprinted against. Typed apart from the agent's
/// own setup failures because no number of restarts changes what is on disk.
fn unusable_runtime(error: impl std::fmt::Display) -> RunError {
    RunError::Runtime(error.to_string())
}

#[cfg(test)]
#[path = "../../tests/conversation/desktop.rs"]
mod tests;

/// Match process configuration to the actual bundle before publishing desktop health.
///
/// The manifest field and `NESSA_RUNTIME_FINGERPRINT` can still name the
/// fingerprint the host recorded while the tree underneath has been replaced.
/// Health is that fingerprint only when the packaging digest of this tree is
/// the same value.
pub(super) fn runtime_identity(
    bundle: &Path,
    configured: String,
    generation: String,
    instance: String,
    process_id: u32,
) -> Result<RunningRuntime, RunError> {
    let manifest = std::fs::read(bundle.join("manifest.json")).map_err(unusable_runtime)?;
    let installed: serde_json::Value =
        serde_json::from_slice(&manifest).map_err(unusable_runtime)?;
    if installed
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        != Some(configured.as_str())
    {
        return Err(unusable_runtime(
            "configured desktop fingerprint differs from installed runtime manifest",
        ));
    }
    if installed_tree_fingerprint(bundle).map_err(unusable_runtime)? != configured {
        return Err(unusable_runtime(
            "configured desktop fingerprint differs from installed runtime tree",
        ));
    }
    RunningRuntime::new(configured, instance, process_id, generation).map_err(unusable_runtime)
}

/// Packaging digest of a prepared runtime.
///
/// The byte sequence is the one `scripts/desktop/runtime-fingerprint.mjs`
/// writes into `manifest.json`: framed names, kinds, executable bits, and
/// lengths, then file bytes, in UTF-16 entry order. The root manifest is not
/// part of the digest. Absolute link targets and links that resolve outside
/// the tree are refused.
pub(super) fn installed_tree_fingerprint(directory: &Path) -> Result<String, String> {
    let root = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    hash_runtime_entry(&root, &root, "", &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn hash_runtime_entry(
    root: &Path,
    path: &Path,
    name: &str,
    hash: &mut Sha256,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path).map_err(|error| error.to_string())?;
        let target = target
            .into_os_string()
            .into_string()
            .map_err(|_| "Runtime names must be UTF-8".to_string())?;
        let resolved = path.canonicalize().map_err(|error| error.to_string())?;
        if Path::new(&target).is_absolute() || !resolved.starts_with(root) {
            return Err("Runtime link must stay inside the bundle".into());
        }
        frame_runtime(hash, serde_json::json!([name, "link", target]))?;
    } else if metadata.is_dir() {
        frame_runtime(hash, serde_json::json!([name, "directory"]))?;
        let mut entries = std::fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by(|left, right| {
            left.file_name()
                .to_string_lossy()
                .encode_utf16()
                .cmp(right.file_name().to_string_lossy().encode_utf16())
        });
        for entry in entries {
            let child = entry
                .file_name()
                .into_string()
                .map_err(|_| "Runtime names must be UTF-8")?;
            if name.is_empty() && child == "manifest.json" {
                continue;
            }
            let relative = if name.is_empty() {
                child
            } else {
                format!("{name}/{child}")
            };
            hash_runtime_entry(root, &entry.path(), &relative, hash)?;
        }
    } else if metadata.is_file() {
        hash_runtime_file(hash, path, name)?;
    } else {
        return Err("Unsupported runtime entry".into());
    }
    Ok(())
}

fn hash_runtime_file(hash: &mut Sha256, path: &Path, name: &str) -> Result<(), String> {
    let mut file = open_runtime_file(path)?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Runtime changed during fingerprinting".into());
    }
    frame_runtime(
        hash,
        serde_json::json!([name, "file", executable_bits(&metadata), metadata.len()]),
    )?;
    let expected = metadata.len();
    let mut bytes = [0_u8; 65_536];
    let mut count = 0_u64;
    loop {
        let read = file.read(&mut bytes).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        count += read as u64;
        hash.update(&bytes[..read]);
    }
    if count != expected {
        return Err("Runtime file changed while hashing".into());
    }
    Ok(())
}

fn frame_runtime(hash: &mut Sha256, value: serde_json::Value) -> Result<(), String> {
    hash.update(serde_json::to_vec(&value).map_err(|error| error.to_string())?);
    hash.update(b"\n");
    Ok(())
}

#[cfg(unix)]
fn open_runtime_file(path: &Path) -> Result<File, String> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn open_runtime_file(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn executable_bits(metadata: &std::fs::Metadata) -> u32 {
    metadata.permissions().mode() & 0o111
}

#[cfg(not(unix))]
fn executable_bits(_metadata: &std::fs::Metadata) -> u32 {
    0
}
