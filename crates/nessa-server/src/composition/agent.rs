//! Trusted local provider configuration. Requests never select processes or workspaces.
use crate::core::RunError;
use nessa_auth::application::ports::Clock;
use nessa_sdk::{
    application::agent_execution::providers::{AgentProvider, UserImageSource},
    domain::model_metadata::entities::ModelMetadata,
    infrastructure::{acp::sessions::StdioMcpServer, model_metadata_json::load_catalog},
};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AgentConfig {
    pub catalog: PathBuf,
    pub node: PathBuf,
    pub acp_entry: PathBuf,
    pub workspace: PathBuf,
    pub model: String,
    #[serde(default)]
    pub tools_enabled: bool,
    #[serde(default)]
    pub mcp_servers: Vec<StdioMcpServer>,
    #[serde(default = "context_tokens")]
    pub context_tokens: u32,
    #[serde(default = "output_tokens")]
    pub output_tokens: u32,
}
impl AgentConfig {
    fn validate(&self) -> Result<(), RunError> {
        if [&self.catalog, &self.node, &self.acp_entry, &self.workspace]
            .iter()
            .any(|path| !path.is_absolute())
            || self.model.trim().is_empty()
            || self.output_tokens == 0
            || self.context_tokens <= self.output_tokens
        {
            return Err(RunError::Agent("agent paths must be absolute, model nonempty, and token limits positive with room for input".into()));
        }
        Ok(())
    }
}
/// The `PATH` the agent's process tree gets: Claude Code, its Bash tool, and
/// the Nessa MCP shell tool all inherit this one.
///
/// It is deliberately not this process's own. A packaged gateway is a launchd
/// service, and the `PATH` launchd gives it is the system one — nothing the user
/// installed is on it, which is how "run the tests" became `command not found:
/// pnpm` on a machine where every terminal has `pnpm`. The desktop host resolves
/// the user's login-shell path once, when it registers the service, and hands it
/// over as `NESSA_AGENT_PATH`: a variable of Nessa's own, so widening what the
/// agent can reach never widens what the service itself can.
///
/// A developer loop has no host and no such variable, and there the process
/// `PATH` *is* the developer's own shell path, which is the right answer.
// Only a Unix build supervises an agent process; the rule is still decided and
// tested here rather than inside that target's branch.
#[cfg_attr(not(unix), allow(dead_code))]
fn agent_search_path(resolved: Option<OsString>, inherited: Option<OsString>) -> Option<OsString> {
    resolved
        .filter(|path| !path.is_empty())
        .or(inherited)
        .filter(|path| !path.is_empty())
}

fn context_tokens() -> u32 {
    100_000
}
fn output_tokens() -> u32 {
    4096
}

/// The selected model as the catalog records it. Read once: the provider is
/// built for it, and uploaded images are fitted to its image limits.
pub(super) fn model(config: &AgentConfig) -> Result<ModelMetadata, RunError> {
    ModelMetadata::try_from(
        load_catalog(
            File::open(&config.catalog)
                .map_err(|_| RunError::Agent("cannot read model catalog".into()))?,
        )
        .map_err(|e| RunError::Agent(e.to_string()))?
        .select("anthropic", &config.model)
        .map_err(|e| RunError::Agent(e.to_string()))?,
    )
    .map_err(|e| RunError::Agent(e.to_string()))
}

#[cfg(unix)]
pub(super) fn provider(
    config: &AgentConfig,
    model: &ModelMetadata,
    directory: &Path,
    clock: Arc<dyn Clock>,
    images: Arc<dyn UserImageSource>,
) -> Result<Arc<dyn AgentProvider>, RunError> {
    config.validate()?;
    build::provider(config, model, directory, clock, images)
}
#[cfg(not(unix))]
pub(super) fn provider(
    config: &AgentConfig,
    _: &ModelMetadata,
    _: &Path,
    _: Arc<dyn Clock>,
    _: Arc<dyn UserImageSource>,
) -> Result<Arc<dyn AgentProvider>, RunError> {
    config.validate()?;
    let profile = if config.tools_enabled {
        "Claude ACP with native and MCP tools"
    } else {
        "Claude ACP"
    };
    Err(RunError::Agent(format!(
        "{profile} requires Unix process supervision"
    )))
}
#[cfg(unix)]
mod build {
    use super::super::agent_budgets as budgets;
    use super::{AgentConfig, ModelMetadata, RunError};
    use crate::conversation::infrastructure::DurableExecutionAudit;
    use nessa_auth::application::ports::Clock;
    use nessa_sdk::{
        application::agent_execution::providers::{AgentProvider, UserImageSource},
        domain::{
            agent_execution::{
                permissions::PermissionOfferPolicy,
                prompts::{PromptSource, PromptSourceKind, SystemPromptBuilder, UserMessage},
            },
            common::value_objects::TokenLimits,
        },
        infrastructure::{acp::sessions::AcpConfig, claude_acp::sessions::ClaudeAcpProvider},
    };
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        path::{Path, PathBuf},
        sync::Arc,
    };

    /// The largest ACP frame, derived from the largest message rather than
    /// chosen beside it. One `session/prompt` carries every image of a message
    /// as base64, which grows bytes by a third: `UserMessage::MAX_IMAGE_BYTES`
    /// (10 MiB) becomes 13⅓ MiB. With 8 KiB of text and the JSON around each
    /// block, that fits 16 MiB and nothing smaller that is a round number. 16
    /// MiB is also the most `AcpConfig` accepts, so the image budget cannot
    /// grow without the SDK's ceiling growing first.
    const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
    const _: () = assert!(
        UserMessage::MAX_IMAGE_BYTES as usize / 3 * 4 + 1024 * 1024 <= MAX_FRAME_BYTES,
        "one message's images, encoded, must fit one ACP frame"
    );
    /// What an agent may send us, which is the buffer this host can be made to
    /// allocate for one frame and has nothing to do with the prompts it writes.
    /// An agent answers in text, so it keeps the mebibyte it had before images.
    const MAX_INCOMING_FRAME_BYTES: usize = 1024 * 1024;

    /// The launch configuration composition injects, separated from resolving
    /// what goes into it so a test can read back the values actually used.
    /// Everything here is a decision; nothing here reads the filesystem.
    ///
    /// `images` is the one dependency rather than a decision: the source a
    /// binding reads a message's uploads from, and `None` is a binding that
    /// sends none.
    pub(super) fn launch_configuration(
        config: &AgentConfig,
        workspace: PathBuf,
        environment: BTreeMap<OsString, OsString>,
        credential_environment: BTreeMap<OsString, OsString>,
        images: Option<Arc<dyn UserImageSource>>,
    ) -> AcpConfig {
        AcpConfig {
            executable: config.node.clone(),
            arguments: vec![config.acp_entry.clone().into_os_string()],
            environment,
            credential_environment,
            workspace,
            tools_enabled: config.tools_enabled,
            mcp_servers: config.mcp_servers.clone(),
            permissions: PermissionOfferPolicy::once_only(),
            // All four from protocol/defaults/agent-startup-budgets.json,
            // which the client compiles in too: a client that gives up before
            // the gateway has finished failing never sees the typed answer.
            // Spawning is the operating system's work — a runtime staged by a
            // fresh install is scanned on its first execution — so it has its
            // own, far larger budget than protocol work.
            launch_timeout: budgets::launch_timeout(),
            startup_timeout: budgets::startup_timeout(),
            execution_timeout: None,
            shutdown_grace: budgets::shutdown_grace(),
            kill_timeout: budgets::kill_timeout(),
            event_capacity: 256,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_incoming_frame_bytes: MAX_INCOMING_FRAME_BYTES,
            images,
        }
    }

    pub(super) fn provider(
        config: &AgentConfig,
        model: &ModelMetadata,
        directory: &Path,
        clock: Arc<dyn Clock>,
        images: Arc<dyn UserImageSource>,
    ) -> Result<Arc<dyn AgentProvider>, RunError> {
        let invalid = |error| RunError::Agent(format!("{error}"));
        let workspace = config
            .workspace
            .canonicalize()
            .map_err(|_| RunError::Agent("workspace must exist".into()))?;
        if !workspace.is_dir()
            || !config.node.is_absolute()
            || !config.acp_entry.is_absolute()
            || !config.node.is_file()
            || !config.acp_entry.is_file()
        {
            return Err(RunError::Agent(
                "node and acpEntry must be existing absolute files; workspace must be a directory"
                    .into(),
            ));
        }
        let limits = TokenLimits::new(config.context_tokens, config.output_tokens)
            .map_err(|e| RunError::Agent(e.to_string()))?;
        let mut environment = BTreeMap::new();
        for key in ["HOME", "USER", "LOGNAME", "TMPDIR", "CLAUDE_CONFIG_DIR"] {
            if let Some(value) = std::env::var_os(key) {
                environment.insert(key.into(), value);
            }
        }
        if let Some(path) = super::agent_search_path(
            std::env::var_os("NESSA_AGENT_PATH"),
            std::env::var_os("PATH"),
        ) {
            environment.insert("PATH".into(), path);
        }
        let mut credential_environment = BTreeMap::new();
        for key in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
            if let Some(value) = std::env::var_os(key) {
                credential_environment.insert(key.into(), value);
            }
        }
        let audit =
            Arc::new(DurableExecutionAudit::new(directory.join("audit"), clock).map_err(invalid)?);
        let provider = ClaudeAcpProvider::new(
            launch_configuration(
                config,
                workspace,
                environment,
                credential_environment,
                Some(images),
            ),
            model,
            limits,
            audit,
        )
        .map_err(|e| RunError::Agent(e.to_string()))?
        .with_system_prompt(
            SystemPromptBuilder::new()
                .text(
                    PromptSource::new(PromptSourceKind::Core, "nessa/gateway")
                        .map_err(|e| RunError::Agent(e.to_string()))?,
                    "You are Nessa, a helpful assistant. Follow the user's request. Use your native tools for file operations and web research. All Nessa tools are provided through MCP. For shell commands use the Nessa MCP shell tool, which tracks execution through Shepherd; never substitute an unmanaged shell path. Report tool failures accurately.",
                )
                .build()
                .map_err(|e| RunError::Agent(e.to_string()))?,
        );
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/configuration.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "../../tests/conversation/launch_configuration.rs"]
mod launch_configuration_tests;
