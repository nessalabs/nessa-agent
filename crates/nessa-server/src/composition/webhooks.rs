//! Private endpoint seeds for the existing product credential command.
use super::runtime_config::RuntimeConfig;
use crate::core::RunError;
use crate::webhooks::{
    application::{MacKey, RegistrationConfig, MAX_REGISTRATIONS},
    domain::Destination,
    infrastructure::{ConfiguredWebhook, SqlOutbox, WebhookRuntime},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_auth::{
    adapters::local::LocalCredentialStore,
    application::{credential_admin::CredentialAdmin, ports::Clock},
    domain::PrincipalId,
};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fmt::{Debug, Formatter, Result as FmtResult},
    path::Path,
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct WebhookConfig {
    principal_id: String,
    url: String,
    mac_key: String,
}
impl Debug for WebhookConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.write_str("WebhookConfig([REDACTED])")
    }
}
fn invalid() -> RunError {
    RunError::RuntimeConfig("invalid private webhook configuration".into())
}
pub(super) async fn prepare(
    settings: &RuntimeConfig,
    directory: &Path,
    credentials: Arc<LocalCredentialStore>,
    base: Arc<dyn CredentialAdmin>,
    clock: Arc<dyn Clock>,
) -> Result<Option<Arc<WebhookRuntime>>, RunError> {
    if settings.webhooks.len() > MAX_REGISTRATIONS {
        return Err(invalid());
    }
    let mut principals = HashSet::new();
    let mut configurations = Vec::with_capacity(settings.webhooks.len());
    for input in &settings.webhooks {
        let principal = PrincipalId::new(&input.principal_id).map_err(|_| invalid())?;
        if !principals.insert(principal.clone()) || input.mac_key.len() != 44 {
            return Err(invalid());
        }
        let key: [u8; 32] = STANDARD
            .decode(&input.mac_key)
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        configurations.push(ConfiguredWebhook {
            principal,
            registration: RegistrationConfig {
                destination: Destination::parse(&input.url).map_err(|_| invalid())?,
                key: Arc::new(MacKey::new(key)),
            },
        });
    }
    let path = directory.join("webhooks.sqlite");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && configurations.is_empty() => {
            return Ok(None)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(RunError::RuntimeConfig(
                "private webhook storage unavailable".into(),
            ))
        }
    }
    let store = Arc::new(
        SqlOutbox::open(&path, directory.join("webhook-audit"), clock.clone())
            .map_err(|_| RunError::RuntimeConfig("private webhook storage unavailable".into()))?,
    );
    let runtime = Arc::new(
        WebhookRuntime::new(configurations, base, credentials, store, clock)
            .map_err(|_| invalid())?,
    );
    runtime
        .recover()
        .await
        .map_err(|_| RunError::RuntimeConfig("webhook command recovery incomplete".into()))?;
    Ok(Some(runtime))
}

#[cfg(test)]
#[path = "../../tests/composition/webhooks.rs"]
mod tests;
