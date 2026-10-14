//! One audited credential-registry open path for serving and offline auth.

use super::local_auth::SystemClock;
use crate::core::RunError;
use nessa_auth::{
    adapters::local::{
        DurableCredentialRegistryRefusalAudit, LocalCredentialStore, LocalStoreConfig,
    },
    application::{
        credential_registry::{
            CredentialRegistryRefusal, CredentialRegistryRefusalAudit,
            CredentialRegistryRefusalCause, CredentialRegistryRefusalInitiator,
        },
        ports::GrantPolicyEvaluator,
    },
};
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegistryOpenContext {
    GatewayStartup,
    AutomaticProvisioning,
    LocalAuthCommand,
}

pub(super) fn open(
    directory: &Path,
    config: LocalStoreConfig,
    grants: Arc<dyn GrantPolicyEvaluator>,
    context: RegistryOpenContext,
) -> Result<LocalCredentialStore, RunError> {
    match LocalCredentialStore::open_with_config(directory, "credentials.v1.json", config, grants) {
        Ok(store) => Ok(store),
        Err(error) => {
            let audit_failure = error.invalid_registry().and_then(|(target, fault)| {
                let (cause, initiator) = match context {
                    RegistryOpenContext::GatewayStartup => (
                        CredentialRegistryRefusalCause::GatewayStartup,
                        CredentialRegistryRefusalInitiator::Automatic,
                    ),
                    RegistryOpenContext::AutomaticProvisioning => (
                        CredentialRegistryRefusalCause::AutomaticProvisioning,
                        CredentialRegistryRefusalInitiator::Automatic,
                    ),
                    RegistryOpenContext::LocalAuthCommand => (
                        CredentialRegistryRefusalCause::LocalAuthCommand,
                        CredentialRegistryRefusalInitiator::LocalProcess,
                    ),
                };
                let refusal = CredentialRegistryRefusal::new(
                    target.to_path_buf(),
                    fault.clone(),
                    cause,
                    initiator,
                );
                DurableCredentialRegistryRefusalAudit::new(
                    directory.to_path_buf(),
                    "audit/credential-registry-refusals".into(),
                    Arc::new(SystemClock),
                )
                .record(&refusal)
                .err()
            });
            Err(RunError::registry(error, audit_failure))
        }
    }
}
