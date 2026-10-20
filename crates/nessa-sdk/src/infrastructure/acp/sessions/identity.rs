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
use std::ffi::OsStr;

/// Environment keys that name where a vendor keeps its account. Present keys
/// are hashed in this order. Every other key is process plumbing.
const CONTEXT_ENVIRONMENT: &[&str] = &[
    "HOME",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

// Length prefixes and explicit collection lengths keep ordered fields unambiguous.
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

/// The restoration fingerprint: what a saved conversation's context must
/// match to be restored. This function is the complete list of its inputs;
/// the `AcpConfig` field docs that state their membership link here.
///
/// Hashed: the executable's path, the ordered arguments, the account
/// environment ([`CONTEXT_ENVIRONMENT`]), the workspace, whether tools are
/// enabled, the token limits, the permission decisions, and the composed
/// system prompt. Changing any of them changes the identity. A saved
/// conversation whose fingerprint differs opens a new native session; it is
/// refused only when the provider name or the model differs. The rows are
/// `docs/design/restoration-fingerprint.md`.
/// (`a_context_only_mismatch_opens_a_fresh_session_and_credentials_rotate_without_persistence`,
/// `plumbing_environment_does_not_change_the_fingerprint`,
/// `fingerprint_tracks_workspace_policy_prompt_limits_and_unambiguous_arguments`).
///
/// Not hashed:
/// - process plumbing in the environment: `PATH`, `TMPDIR`, `USER`,
///   `LOGNAME`, `CODEX_PATH`, `CLAUDE_CODE_EXECUTABLE`, and any key that is
///   not in [`CONTEXT_ENVIRONMENT`]. On macOS `PATH` is the login shell's
///   path, rewritten into the launchd job on every app update, and `TMPDIR`
///   differs between launchd and an app-launched child. Neither chooses which
///   native session to resume;
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
/// What follows from this for the gateway — what a fingerprint change does,
/// and what still strands a saved conversation — is in
/// `docs/design/mcp-connections.md`, "MCP servers and the restoration
/// identity", and `docs/design/restoration-fingerprint.md`.
pub(crate) fn fingerprint(
    config: &AcpConfig,
    limits: TokenLimits,
    prompt: Option<&SystemPrompt>,
) -> String {
    fingerprint_fields(config, Some(limits), prompt)
}

/// Execution inputs shared by model choices; leaves existing resume fingerprints unchanged.
pub(crate) fn model_change_context(config: &AcpConfig, prompt: Option<&SystemPrompt>) -> String {
    fingerprint_fields(config, None, prompt)
}

fn fingerprint_fields(
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
    let context_environment: Vec<_> = CONTEXT_ENVIRONMENT
        .iter()
        .filter_map(|key| config.environment.get_key_value(OsStr::new(key)))
        .collect();
    hash.update((context_environment.len() as u64).to_be_bytes());
    for (key, value) in context_environment {
        field(&mut hash, key.as_encoded_bytes());
        field(&mut hash, value.as_encoded_bytes());
    }
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
