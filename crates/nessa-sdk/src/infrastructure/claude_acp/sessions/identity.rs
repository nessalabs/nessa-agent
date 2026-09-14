//! Credential-free restoration identity for exact context-selecting launch inputs.
use crate::domain::agent_execution::{
    permissions::{PermissionEffect, PermissionScopeView},
    prompts::SystemPrompt,
};
use crate::domain::common::value_objects::TokenLimits;
use crate::infrastructure::acp::sessions::AcpConfig;
use sha2::{Digest, Sha256};

// Length prefixes and explicit collection lengths keep ordered fields unambiguous.
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

pub(super) fn fingerprint(
    config: &AcpConfig,
    limits: TokenLimits,
    prompt: Option<&SystemPrompt>,
) -> String {
    let mut hash = Sha256::new();
    field(&mut hash, config.executable.as_os_str().as_encoded_bytes());
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
    hash.update([u8::from(config.file_tools)]);
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
