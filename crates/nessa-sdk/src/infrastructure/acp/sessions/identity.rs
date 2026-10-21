//! Credential-free restoration identity for exact context-selecting launch inputs.
//!
//! Every ACP profile launches a process the same way, so the inputs that select
//! a restorable context are the same inputs for all of them. The provider name
//! is not hashed here: [`ProviderIdentity`] already carries it beside this
//! fingerprint, so two profiles with byte-identical configuration still hold
//! distinct identities.
//!
//! [`ProviderIdentity`]: crate::application::agent_execution::providers::ProviderIdentity

use super::AcpConfig;
use crate::domain::agent_execution::{
    permissions::{PermissionEffect, PermissionScopeView},
    prompts::SystemPrompt,
};
use crate::domain::common::value_objects::TokenLimits;
use sha2::{Digest, Sha256};

// Length prefixes and explicit collection lengths keep ordered fields unambiguous.
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

/// The restoration fingerprint: what a saved conversation's context must
/// match to be restored. This function is the complete list of its inputs;
/// the `AcpConfig` field docs that state their membership link here.
///
/// Hashed: the context environment (`AcpConfig::environment`: HOME and the
/// harness's configuration directories, which is where it keeps its
/// sessions), the workspace, whether tools are enabled, the token limits, the
/// permission decisions, and the composed system prompt. Changing any of them
/// changes the identity, and a saved conversation whose context would be
/// restored is refused before launch; one whose saved context is not restored
/// (opened on another worker of a pool) goes on under the new fingerprint
/// (`context_changes_reject_restore_before_launch_but_credentials_rotate_without_persistence`,
/// `fingerprint_tracks_workspace_policy_prompt_limits_and_context_environment`).
///
/// Not hashed:
/// - the executable, its arguments and the runtime environment
///   (`AcpConfig::runtime_environment`: the search path, the temporary
///   directory, the user name, a native executable's location). They say
///   which program runs, from where, and how it finds its tools; the harness
///   itself is the provider name beside this fingerprint. All of them move on
///   their own: a desktop release stages the runtime under a new directory,
///   and the search path is read from the person's shell again, and a
///   conversation must not be lost to either
///   (`moving_the_runtime_keeps_the_identity_and_restores`);
/// - the credential environment, so credentials rotate without stranding a
///   conversation;
/// - the stand-in grants (`AcpConfig::stand_ins`), fresh on every open;
/// - the MCP servers (`AcpConfig::mcp_servers`): their names, commands and
///   arguments. They are attached to each provider open, like the grants, and
///   select no provider context, so adding, editing, removing or moving a
///   server keeps the identity and the conversation resumes with the current
///   list (`adding_an_mcp_server_keeps_the_identity_and_restores`,
///   `editing_...`, `moving_an_mcp_servers_command_...`, `removing_...`).
///
/// What follows from this for the gateway — each one-time change of every
/// saved identity, and what still strands a saved conversation — is in
/// `docs/design/mcp-connections.md`, "MCP servers and the restoration
/// identity".
pub(crate) fn fingerprint(
    config: &AcpConfig,
    limits: TokenLimits,
    prompt: Option<&SystemPrompt>,
) -> String {
    let mut hash = Sha256::new();
    environment(&mut hash, config, false);
    context_fields(hash, config, Some(limits), prompt)
}

/// Every launch input, credentials and per-open attachments aside: the
/// restoration fingerprint's inputs plus the executable, its arguments and the
/// runtime environment. It changes whenever the runtime moves, which is what
/// a warm-up is for (`AgentProvider::launch_fingerprint`).
pub(crate) fn launch_fingerprint(
    config: &AcpConfig,
    limits: TokenLimits,
    prompt: Option<&SystemPrompt>,
) -> String {
    launch_fields(config, Some(limits), prompt)
}

/// Execution inputs shared by model choices: every launch input but the
/// limits, which a model choice may change.
pub(crate) fn model_change_context(config: &AcpConfig, prompt: Option<&SystemPrompt>) -> String {
    launch_fields(config, None, prompt)
}

fn launch_fields(
    config: &AcpConfig,
    limits: Option<TokenLimits>,
    prompt: Option<&SystemPrompt>,
) -> String {
    let mut hash = Sha256::new();
    field(
        &mut hash,
        config
            .executable
            .executable()
            .as_os_str()
            .as_encoded_bytes(),
    );
    hash.update((config.arguments.len() as u64).to_be_bytes());
    for argument in &config.arguments {
        field(&mut hash, argument.as_encoded_bytes());
    }
    environment(&mut hash, config, true);
    context_fields(hash, config, limits, prompt)
}

/// The context environment, and with `runtime` the runtime environment after
/// it, each length-prefixed so a key cannot move from one to the other unseen.
fn environment(hash: &mut Sha256, config: &AcpConfig, runtime: bool) {
    let maps = [
        Some(&config.environment),
        runtime.then_some(&config.runtime_environment),
    ];
    for map in maps.into_iter().flatten() {
        hash.update((map.len() as u64).to_be_bytes());
        for (key, value) in map {
            field(hash, key.as_encoded_bytes());
            field(hash, value.as_encoded_bytes());
        }
    }
}

fn context_fields(
    mut hash: Sha256,
    config: &AcpConfig,
    limits: Option<TokenLimits>,
    prompt: Option<&SystemPrompt>,
) -> String {
    field(&mut hash, config.workspace.as_os_str().as_encoded_bytes());
    hash.update([u8::from(config.tools_enabled)]);
    if let Some(limits) = limits {
        hash.update(limits.max_context_window().to_be_bytes());
        hash.update(limits.max_output().to_be_bytes());
    }
    hash.update((config.permissions.decisions().len() as u64).to_be_bytes());
    for decision in config.permissions.decisions() {
        hash.update([match decision.effect() {
            PermissionEffect::Allow => 0,
            PermissionEffect::Deny => 1,
        }]);
        match decision.scope().view() {
            PermissionScopeView::Request => hash.update([0]),
            PermissionScopeView::Session {
                application_id,
                session_id,
            } => {
                hash.update([1]);
                field(&mut hash, application_id.as_str().as_bytes());
                field(&mut hash, session_id.as_str().as_bytes());
            }
            PermissionScopeView::Application(application_id) => {
                hash.update([2]);
                field(&mut hash, application_id.as_str().as_bytes());
            }
        }
    }
    hash.update([u8::from(prompt.is_some())]);
    if let Some(prompt) = prompt {
        field(&mut hash, prompt.text().as_str().as_bytes());
    }
    format!("sha256:{:x}", hash.finalize())
}
