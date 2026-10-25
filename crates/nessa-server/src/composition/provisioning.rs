//! Create the local credentials a single-user namespace needs, once.
//!
//! ```text
//! auth/credentials.v1.json absent ──► auth init          ──► <root>/owner.token
//! auth/surfaces/nessa-panel.token absent ──► provision-surface ──► panel token
//! panel credential has the set before S17 ──► provision-surface ──► panel token
//! ```
//! Arrows mean "is created by". Each check is a guard, not a refresh: a file that
//! is already there is left exactly as it is, so restarting a server never
//! invalidates the token a running surface holds. The one exception is a panel
//! credential provisioned before `conversation.read` joined the owner set (row
//! S20): it is re-provisioned once, before this gateway listens, so the window
//! that connects to it reads the new token. Callers ask for this
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
    /// Re-provision the panel surface when it holds the set before S17;
    /// whether it did.
    fn upgrade_panel(&self, auth: &std::path::Path) -> Result<bool, RunError>;
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

    fn upgrade_panel(&self, auth: &std::path::Path) -> Result<bool, RunError> {
        super::auth_command::upgrade_panel(
            auth,
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
    // Never rotate an existing surface credential on app startup, except
    // once to bring one provisioned before S17 up to the owner set.
    if auth.join("surfaces/nessa-panel.token").exists() {
        let upgraded = auth_command.upgrade_panel(auth).map_err(|error| {
            context(
                "could not bring the nessa-panel surface credential up to date",
                error,
            )
        })?;
        if upgraded {
            tracing::info!(
                stage = config.stage.as_str(),
                token = %auth.join("surfaces/nessa-panel.token").display(),
                "the chat surface credential predated conversation.read; provisioned it again with the owner grants",
            );
        }
    } else {
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
        // Both are permanent and already name their file; rewrapping them as an
        // authentication failure would make a managed gateway relaunch forever.
        registry @ RunError::Registry(_) => registry,
        config @ RunError::RuntimeConfig(_) => config,
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

    /// A registry as `auth init` wrote it with `panel` as the panel's set,
    /// built through the registry API. Answers the panel token, written where
    /// the window reads it.
    fn namespace_with_panel(
        auth: &std::path::Path,
        panel: &[&str],
        expires_at: Option<u64>,
    ) -> String {
        use nessa_auth::{
            adapters::local::{BootstrapRequest, LocalCredentialStore},
            application::{credential_admin::IssueCredentialOutcome, dto::*},
        };
        nessa_local_storage::create_directory(&auth.join("surfaces")).unwrap();
        let store = LocalCredentialStore::open(
            auth,
            "credentials.v1.json",
            std::sync::Arc::new(nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        };
        store
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "owner-membership".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "owner-credential".into(),
                issued_at: 1,
                expires_at: None,
                grants: ["server.read", "conversation.write", "credential.manage"]
                    .into_iter()
                    .map(grant)
                    .collect(),
            })
            .unwrap();
        let IssueCredentialOutcome::Issued { evidence, .. } = store
            .provision_surface(
                "nessa-panel",
                "panel-request".into(),
                "panel-credential".into(),
                panel.iter().map(|action| (*action).to_owned()).collect(),
                2,
                expires_at,
            )
            .unwrap()
        else {
            panic!("the panel was not issued")
        };
        let token = String::from_utf8(evidence.expose_bytes().to_vec()).unwrap();
        let mut file = nessa_local_storage::open(
            &auth.join("surfaces/nessa-panel.token"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        file.write_all(token.as_bytes()).unwrap();
        token
    }

    /// The live panel credential's actions, sorted, read back from `auth`.
    fn panel_grants(auth: &std::path::Path) -> Vec<String> {
        panel(auth).0
    }

    /// The live panel credential's sorted actions and its expiry.
    fn panel(auth: &std::path::Path) -> (Vec<String>, Option<u64>) {
        let store = nessa_auth::adapters::local::LocalCredentialStore::open(
            auth,
            "credentials.v1.json",
            std::sync::Arc::new(nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        let listed = store
            .list_sync(
                &nessa_auth::application::credential_admin::ListCredentialsRequest {
                    organization_id: "org".into(),
                },
            )
            .unwrap();
        let live = listed
            .iter()
            .filter(|credential| {
                credential.principal_id == "surface:nessa-panel" && credential.revoked_at.is_none()
            })
            .collect::<Vec<_>>();
        assert_eq!(live.len(), 1, "one live panel credential");
        let mut actions = live[0]
            .grants
            .iter()
            .map(|grant| grant.action.clone())
            .collect::<Vec<_>>();
        actions.sort();
        (actions, live[0].expires_at)
    }

    /// The guard that matters: with both files present and the panel holding
    /// the owner set, nothing is minted, so a second start cannot invalidate
    /// the token a surface is already holding. This runs without the process
    /// environment the offline commands read, which is itself the assertion
    /// that neither command was reached.
    #[test]
    fn an_already_provisioned_namespace_is_left_untouched() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("namespace");
        let auth = root.join("auth");
        let token = namespace_with_panel(
            &auth,
            &[
                "server.read",
                "conversation.read",
                "conversation.write",
                "credential.manage",
            ],
            None,
        );
        let registry = std::fs::read(auth.join("credentials.v1.json")).unwrap();

        ensure_local_credentials(&environment(auth.clone())).unwrap();

        assert_eq!(
            std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap(),
            token
        );
        assert_eq!(
            std::fs::read(auth.join("credentials.v1.json")).unwrap(),
            registry
        );
        assert!(!root.join("owner.token").exists());
    }

    /// Row S20: a panel provisioned before S17 holds no `conversation.read`,
    /// so its window cannot pair a device. A start re-provisions it once with
    /// the owner set, writing a new token where the window reads it, and the
    /// start after that leaves it alone.
    #[test]
    fn a_panel_provisioned_before_conversation_read_is_brought_up_to_date_once() {
        let temporary = tempfile::tempdir().unwrap();
        let auth = temporary.path().join("namespace/auth");
        let old = namespace_with_panel(
            &auth,
            &["server.read", "conversation.write", "credential.manage"],
            None,
        );

        ensure_local_credentials(&environment(auth.clone())).unwrap();

        let new = std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap();
        assert_ne!(new.trim(), old, "the panel was provisioned again");
        assert_eq!(
            panel_grants(&auth),
            [
                "conversation.read",
                "conversation.write",
                "credential.manage",
                "server.read"
            ]
        );
        let registry = std::fs::read(auth.join("credentials.v1.json")).unwrap();
        ensure_local_credentials(&environment(auth.clone())).unwrap();
        assert_eq!(
            std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap(),
            new
        );
        assert_eq!(
            std::fs::read(auth.join("credentials.v1.json")).unwrap(),
            registry
        );
    }

    /// The upgrade changes the grants, never how long the panel may sign in:
    /// an earlier panel with an expiry keeps it.
    #[test]
    fn an_upgraded_panel_keeps_its_expiry() {
        let temporary = tempfile::tempdir().unwrap();
        let auth = temporary.path().join("namespace/auth");
        let expires = 9_000_000_000;
        let old = namespace_with_panel(
            &auth,
            &["server.read", "conversation.write", "credential.manage"],
            Some(expires),
        );

        ensure_local_credentials(&environment(auth.clone())).unwrap();

        let new = std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap();
        assert_ne!(new.trim(), old);
        assert_eq!(
            panel(&auth),
            (
                vec![
                    "conversation.read".to_owned(),
                    "conversation.write".to_owned(),
                    "credential.manage".to_owned(),
                    "server.read".to_owned(),
                ],
                Some(expires)
            )
        );
    }

    /// An earlier panel that has expired is not live: it is not brought back
    /// with a new credential.
    #[test]
    fn an_expired_earlier_panel_is_not_brought_back() {
        let temporary = tempfile::tempdir().unwrap();
        let auth = temporary.path().join("namespace/auth");
        let token = namespace_with_panel(
            &auth,
            &["server.read", "conversation.write", "credential.manage"],
            Some(3),
        );
        let registry = std::fs::read(auth.join("credentials.v1.json")).unwrap();

        ensure_local_credentials(&environment(auth.clone())).unwrap();

        assert_eq!(
            std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap(),
            token
        );
        assert_eq!(
            std::fs::read(auth.join("credentials.v1.json")).unwrap(),
            registry
        );
    }

    /// A set chosen some other way, here a panel that only reads the gateway,
    /// is not the earlier owner set, and is left as it is.
    #[test]
    fn a_panel_with_another_set_is_left_as_it_is() {
        let temporary = tempfile::tempdir().unwrap();
        let auth = temporary.path().join("namespace/auth");
        let token = namespace_with_panel(&auth, &["server.read"], None);

        ensure_local_credentials(&environment(auth.clone())).unwrap();

        assert_eq!(
            std::fs::read_to_string(auth.join("surfaces/nessa-panel.token")).unwrap(),
            token
        );
        assert_eq!(panel_grants(&auth), ["server.read"]);
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
        upgrades: Mutex<usize>,
    }

    impl FakeAuth {
        fn succeeding() -> Self {
            Self {
                asked: Mutex::new(Vec::new()),
                outcome: Ok(()),
                upgrades: Mutex::new(0),
            }
        }

        fn refusing(error: RunError) -> Self {
            Self {
                asked: Mutex::new(Vec::new()),
                outcome: Err(error),
                upgrades: Mutex::new(0),
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
                Err(RunError::RuntimeConfig(message)) => {
                    Err(RunError::RuntimeConfig(message.clone()))
                }
                Err(other) => Err(failure(other.to_string())),
            }
        }

        fn upgrade_panel(&self, _auth: &std::path::Path) -> Result<bool, RunError> {
            *self.upgrades.lock().unwrap() += 1;
            Ok(false)
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

    /// A refused `config.json` met while minting stays a configuration
    /// failure, so the managed service stops instead of relaunching (S2).
    #[test]
    fn refused_runtime_configuration_survives_provisioning() {
        let (_temporary, config) = namespace();
        let auth = FakeAuth::refusing(RunError::RuntimeConfig("unknown field `tls`".into()));

        let error = provision_with(&auth, &config).unwrap_err();

        // core::restart classifies RuntimeConfig as Pointless (its own tests).
        assert!(matches!(error, RunError::RuntimeConfig(_)), "{error}");
    }

    /// The startup path of a fresh machine: both credentials are minted, in
    /// that order, and each by the command that mints it.
    #[test]
    fn an_empty_namespace_mints_an_owner_and_a_surface() {
        let (_temporary, config) = namespace();
        let auth = FakeAuth::succeeding();

        provision_with(&auth, &config).expect("a fresh namespace provisions");

        assert_eq!(auth.subcommands(), vec!["init", "provision-surface"]);
        // A panel minted just now has the owner set: nothing to bring up to date.
        assert_eq!(*auth.upgrades.lock().unwrap(), 0);
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
