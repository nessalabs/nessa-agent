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

/// Running one `nessa auth …` command.
///
/// A port, because it is a subprocess-shaped thing reaching a keyring and a
/// disk: behind it, the two branches below — mint an owner credential, mint a
/// surface credential — can be made to happen and to fail without either. They
/// are on the startup path of both the packaged app and `just start`, and until
/// this existed neither branch was reachable from a test at all.
pub(super) trait AuthCommand {
    fn execute(&self, args: &[String]) -> Result<(), RunError>;
}

/// The real one: this binary's own `auth` subcommand, in process.
pub(super) struct NessaAuth;

impl AuthCommand for NessaAuth {
    fn execute(&self, args: &[String]) -> Result<(), RunError> {
        super::auth_command::execute_with_context(
            args,
            super::credential_registry::RegistryOpenContext::AutomaticProvisioning,
        )
    }
}

pub(super) fn ensure_local_credentials(config: &Environment) -> Result<(), RunError> {
    provision_with(&NessaAuth, config)
}

fn provision_with(auth_command: &dyn AuthCommand, config: &Environment) -> Result<(), RunError> {
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
        auth_command
            .execute(&[
                "auth".into(),
                "init".into(),
                "--owner-token-file".into(),
                owner_token.to_string_lossy().into_owned(),
            ])
            .map_err(|error| {
                context("could not initialize the local credential registry", error)
            })?;
    }
    // Never rotate an existing surface credential on app startup.
    if !auth.join("surfaces/nessa-panel.token").exists() {
        tracing::info!(
            stage = config.stage.as_str(),
            token = %auth.join("surfaces/nessa-panel.token").display(),
            "no chat surface credential; provisioning one for nessa-panel",
        );
        auth_command
            .execute(&[
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
        registry @ RunError::Registry(_) => registry,
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

#[cfg(test)]
mod minting_tests {
    use super::*;
    use crate::env::{MockEnv, Stage, STAGE};
    use std::sync::Mutex;

    /// An `auth` command that records what it was asked and answers as told.
    struct FakeAuth {
        asked: Mutex<Vec<Vec<String>>>,
        outcome: Result<(), RunError>,
    }

    impl FakeAuth {
        fn succeeding() -> Self {
            Self {
                asked: Mutex::new(Vec::new()),
                outcome: Ok(()),
            }
        }

        fn refusing(error: RunError) -> Self {
            Self {
                asked: Mutex::new(Vec::new()),
                outcome: Err(error),
            }
        }

        /// The subcommand of each call, which is what the branches differ by.
        fn subcommands(&self) -> Vec<String> {
            self.asked
                .lock()
                .unwrap()
                .iter()
                .filter_map(|args| args.get(1).cloned())
                .collect()
        }
    }

    impl AuthCommand for FakeAuth {
        fn execute(&self, args: &[String]) -> Result<(), RunError> {
            self.asked.lock().unwrap().push(args.to_vec());
            match &self.outcome {
                Ok(()) => Ok(()),
                Err(RunError::Authentication(message)) => {
                    Err(RunError::Authentication(message.clone()))
                }
                Err(other) => Err(failure(other.to_string())),
            }
        }
    }

    fn namespace() -> (tempfile::TempDir, Environment) {
        let temporary = tempfile::tempdir().unwrap();
        let auth = temporary.path().join("namespace/auth");
        let mut config =
            Environment::load(&MockEnv::new().set(STAGE, "ci")).expect("isolated ci config");
        assert_eq!(config.stage, Stage::Ci);
        config.auth_directory = Some(auth);
        (temporary, config)
    }

    /// The startup path of a fresh machine: both credentials are minted, in
    /// that order, and each by the command that mints it.
    #[test]
    fn an_empty_namespace_mints_an_owner_and_a_surface() {
        let (_temporary, config) = namespace();
        let auth = FakeAuth::succeeding();

        provision_with(&auth, &config).expect("a fresh namespace provisions");

        assert_eq!(auth.subcommands(), vec!["init", "provision-surface"]);
    }

    /// Each guard is its own: a registry that exists is not re-initialised, and
    /// the surface credential is still minted beside it.
    #[test]
    fn an_existing_registry_is_not_initialised_again() {
        let (_temporary, config) = namespace();
        let auth_directory = config.auth_directory.clone().unwrap();
        nessa_local_storage::create_directory(&auth_directory).unwrap();
        std::fs::write(auth_directory.join("credentials.v1.json"), b"{}").unwrap();
        let auth = FakeAuth::succeeding();

        provision_with(&auth, &config).expect("provisioning continues");

        assert_eq!(auth.subcommands(), vec!["provision-surface"]);
    }

    /// A failure names the step, so the process exit says which of the two a
    /// person has to repair.
    #[test]
    fn a_failure_says_which_step_it_was() {
        let (_temporary, config) = namespace();
        let auth = FakeAuth::refusing(RunError::Authentication("keyring locked".into()));

        let failed = provision_with(&auth, &config).expect_err("the command refused");

        let message = failed.to_string();
        assert!(message.contains("credential registry"), "{message}");
        assert!(message.contains("keyring locked"), "{message}");
    }

    /// And it says it once. An authentication failure is already reported under
    /// "authentication setup failed", so `context` carries its message rather
    /// than its display form — saying it twice reads as two failures.
    #[test]
    fn an_authentication_failure_is_not_announced_twice() {
        let wrapped = context(
            "could not do the thing",
            RunError::Authentication("keyring locked".into()),
        );

        let message = wrapped.to_string();
        assert_eq!(
            message.matches("authentication setup failed").count(),
            1,
            "{message}"
        );
        assert!(
            message.contains("could not do the thing: keyring locked"),
            "{message}"
        );
    }

    /// Any other failure keeps its own words, because they are not doubled.
    #[test]
    fn another_failure_keeps_its_own_display() {
        let wrapped = context("could not do the thing", failure("disk fell off"));

        assert!(
            wrapped.to_string().contains("could not do the thing:"),
            "{wrapped}"
        );
    }
}
