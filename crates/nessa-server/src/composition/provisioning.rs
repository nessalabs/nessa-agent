//! Create the local credentials a single-user namespace needs, once.
//!
//! ```text
//! auth/credentials.v1.json absent ──► auth init          ──► <root>/owner.token
//! auth/surfaces/nessa-panel.token absent ──► provision-surface ──► panel token
//! ```
//! Arrows mean "is created by". Each check is a guard, not a refresh: a file that
//! is already there is left exactly as it is, so restarting a server never
//! invalidates the token a running surface holds. Callers ask for this
//! explicitly ([`crate::cli::entrypoint::LocalProvisioning`]); minting an owner
//! credential is never a silent side effect of serving.
use crate::{core::RunError, env::Environment};

pub(super) fn ensure_local_credentials(config: &Environment) -> Result<(), RunError> {
    let auth = config
        .auth_directory
        .as_ref()
        .ok_or_else(|| failure("missing data directory"))?;
    let root = auth
        .parent()
        .ok_or_else(|| failure("invalid data directory"))?;
    nessa_local_storage::create_directory(root).map_err(|error| {
        failure(format!(
            "could not create the local data directory {}: {error}",
            root.display()
        ))
    })?;
    if !auth.join("credentials.v1.json").exists() {
        let owner_token = root.join("owner.token");
        tracing::info!(
            stage = config.stage.as_str(),
            registry = %auth.join("credentials.v1.json").display(),
            owner_token = %owner_token.display(),
            "no local credential registry; creating one and an owner credential",
        );
        super::auth_command::execute(&[
            "auth".into(),
            "init".into(),
            "--owner-token-file".into(),
            owner_token.to_string_lossy().into_owned(),
        ])
        .map_err(|error| context("could not initialize the local credential registry", error))?;
    }
    // Never rotate an existing surface credential on app startup.
    if !auth.join("surfaces/nessa-panel.token").exists() {
        tracing::info!(
            stage = config.stage.as_str(),
            token = %auth.join("surfaces/nessa-panel.token").display(),
            "no chat surface credential; provisioning one for nessa-panel",
        );
        super::auth_command::execute(&[
            "auth".into(),
            "provision-surface".into(),
            "--surface-id".into(),
            "nessa-panel".into(),
        ])
        .map_err(|error| {
            context(
                "could not provision the nessa-panel surface credential",
                error,
            )
        })?;
    }
    Ok(())
}

/// Keep the step that failed attached to the reason it failed, so the process
/// exit says which of the two provisioning steps a person has to repair.
///
/// An authentication failure carries its message rather than its display form,
/// because the whole thing is reported under "authentication setup failed" once
/// already and saying it twice reads like two different failures.
fn context(step: &str, error: RunError) -> RunError {
    match error {
        RunError::Authentication(message) => failure(format!("{step}: {message}")),
        other => failure(format!("{step}: {other}")),
    }
}

fn failure(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, Stage, STAGE};
    use std::io::Write;

    fn environment(auth: std::path::PathBuf) -> Environment {
        let mut config =
            Environment::load(&MockEnv::new().set(STAGE, "ci")).expect("isolated ci config");
        assert_eq!(config.stage, Stage::Ci);
        config.auth_directory = Some(auth);
        config
    }

    /// The guard that matters: with both files present nothing is minted, so a
    /// second start cannot invalidate the token a surface is already holding.
    /// This runs without the process environment the offline commands read,
    /// which is itself the assertion that neither command was reached.
    #[test]
    fn an_already_provisioned_namespace_is_left_untouched() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("namespace");
        let auth = root.join("auth");
        nessa_local_storage::create_directory(&auth.join("surfaces")).unwrap();
        let mut registry = nessa_local_storage::open(
            &auth.join("credentials.v1.json"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        registry.write_all(b"fixture-registry").unwrap();
        let mut token = nessa_local_storage::open(
            &auth.join("surfaces/nessa-panel.token"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        token.write_all(b"fixture-token").unwrap();
        drop((registry, token));

        ensure_local_credentials(&environment(auth.clone())).unwrap();

        assert_eq!(
            std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap(),
            "fixture-token"
        );
        assert_eq!(
            std::fs::read_to_string(auth.join("credentials.v1.json")).unwrap(),
            "fixture-registry"
        );
        assert!(!root.join("owner.token").exists());
    }

    /// A namespace nobody could write to fails with the directory in the message,
    /// rather than leaving the surface to report a missing credential later.
    #[test]
    fn an_unusable_namespace_is_reported_as_itself() {
        let mut config = environment(std::path::PathBuf::from("/"));
        let error = ensure_local_credentials(&config).unwrap_err().to_string();
        assert!(error.contains("invalid data directory"), "{error}");

        config.auth_directory = None;
        assert!(ensure_local_credentials(&config)
            .unwrap_err()
            .to_string()
            .contains("missing data directory"));
    }
}
