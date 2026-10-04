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
/// each `AcpConfig` field's documentation states its own membership and links
/// here.
///
/// Hashed: the executable's path, the ordered arguments, the context
/// environment, the workspace, whether tools are enabled, the token limits,
/// the permission decisions, and the composed system prompt. Changing any of
/// them changes the identity, and a saved conversation is refused before launch
/// (`context_changes_reject_restore_before_launch_but_credentials_rotate_without_persistence`,
/// `fingerprint_tracks_workspace_policy_prompt_limits_and_unambiguous_arguments`).
///
/// Not hashed:
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
/// What follows from this for the gateway — the one-time change of every
/// saved identity, and what still strands a saved conversation — is in
/// `docs/design/mcp-connections.md`, "MCP servers and the restoration
/// identity".
pub(crate) fn fingerprint(
    config: &AcpConfig,
    limits: TokenLimits,
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
    hash.update((config.environment.len() as u64).to_be_bytes());
    for (key, value) in &config.environment {
        field(&mut hash, key.as_encoded_bytes());
        field(&mut hash, value.as_encoded_bytes());
    }
    field(&mut hash, config.workspace.as_os_str().as_encoded_bytes());
    hash.update([u8::from(config.tools_enabled)]);
    hash.update(limits.max_context_window().to_be_bytes());
    hash.update(limits.max_output().to_be_bytes());
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
