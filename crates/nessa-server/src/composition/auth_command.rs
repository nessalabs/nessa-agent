//! Offline local owner provisioning. OS access and the exclusive registry lock
//! establish authority; these commands are never available through the gateway.
use super::credential_registry::{self, RegistryOpenContext};
use super::local_auth::SystemClock;
use crate::{core::RunError, env::Environment};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::{
    adapters::local::{BootstrapRequest, LocalCredentialStore},
    application::{
        credential_admin::IssueCredentialOutcome,
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        ports::{Clock, Decision, GrantPolicyEvaluator, IssuePath},
    },
    domain::{Action, Grant, OrganizationId, PrincipalKind, Resource, ResourceId},
};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

/// Parse the explicit offline command; credential material never appears in argv.
pub(super) fn execute(args: &[String]) -> Result<(), RunError> {
    execute_with_context(args, RegistryOpenContext::LocalAuthCommand)
}

pub(super) fn execute_with_context(
    args: &[String],
    registry_context: RegistryOpenContext,
) -> Result<(), RunError> {
    if args.get(1).is_some_and(|arg| arg == "provision-surface") {
        return provision_command(args, registry_context);
    }
    let (recover, output) = parse(&args[..args.len().min(4)])?;
    let extra = &args[args.len().min(4)..];
    validate_options(
        extra,
        if recover {
            &["--expires-at"]
        } else {
            &["--expires-at", "--chat-grants"]
        },
    )?;
    let expires_at = expiry_option(extra)?;
    let chat_grants =
        option_value(&args[args.len().min(4)..], "--chat-grants")?.unwrap_or(OWNER_GRANTS);
    let policy = grant_policy()?;
    // Before anything is written: `init` commits the owner and only then
    // provisions the panel, so a grant the panel may not hold must be refused
    // here, or it would leave an initialized registry with no panel.
    validate_grants(policy.as_ref(), chat_grants)?;
    let directory = Environment::auth_directory_from_system()?;
    prepare_auth_directory(&directory)?;
    let settings = super::runtime_config::RuntimeConfig::load(&directory)?;
    let store = credential_registry::open(&directory, settings.registry, policy, registry_context)?;
    // Reserve a new protected output before changing durable state. Never overwrite.
    let mut file = private_output(&output).map_err(failure)?;
    let now = SystemClock.unix_seconds();
    let credential_id = Uuid::new_v4().to_string();
    let outcome = if recover {
        let owner = OWNER_GRANTS.split(',').collect::<Vec<_>>();
        store.recover_owner(credential_id, &owner, now, expires_at)
    } else {
        let gateway_id = Uuid::new_v4().to_string();
        let organization_id = Uuid::new_v4().to_string();
        let principal_id = Uuid::new_v4().to_string();
        store.bootstrap(BootstrapRequest {
            gateway_id: gateway_id.clone(),
            organization: OrganizationInputDto {
                id: organization_id.clone(),
            },
            principal: PrincipalInputDto {
                id: principal_id.clone(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: Uuid::new_v4().to_string(),
                principal_id,
                organization_id: organization_id.clone(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id,
            issued_at: now,
            expires_at,
            grants: OWNER_GRANTS
                .split(',')
                .map(|action| CredentialGrantDto {
                    action: action.into(),
                    resource: ResourceDto {
                        organization_id: organization_id.clone(),
                        id: gateway_id.clone(),
                    },
                })
                .collect(),
        })
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            // No secret has been delivered. Release the reserved output so a
            // corrected command can reuse it without leaving a misleading token.
            drop(file);
            std::fs::remove_file(&output).map_err(failure)?;
            return Err(failure(error));
        }
    };
    file.write_all(outcome.evidence.expose_bytes()).and_then(|()| file.write_all(b"\n")).and_then(|()| file.sync_all())
        .map_err(|_| RunError::Authentication("credential committed but token output failed; run nessa auth recover-owner --local with a new output path".into()))?;
    if let Some(parent) = output.parent() {
        nessa_local_storage::sync_directory(parent).map_err(failure)?;
    }
    if !recover {
        provision(&store, &directory, "nessa-panel", chat_grants, None, now)?;
    }
    println!(
        "Local owner credential {} written to {} (expires at {}).",
        outcome.metadata.id,
        output.display(),
        outcome
            .metadata
            .expires_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "never".into())
    );
    Ok(())
}

/// The Cedar bundle the registry asks which grants a credential may carry.
fn grant_policy() -> Result<Arc<CedarPolicyEvaluator>, RunError> {
    CedarPolicyEvaluator::new()
        .map(Arc::new)
        .map_err(|_| RunError::Authentication("the embedded policy bundle is invalid".into()))
}

fn validate_options(args: &[String], allowed: &[&str]) -> Result<(), RunError> {
    let (pairs, rest) = args.as_chunks::<2>();
    let mut seen = std::collections::HashSet::new();
    if !rest.is_empty() {
        return Err(failure("options require a value"));
    }
    for pair in pairs {
        if !allowed.contains(&pair[0].as_str()) || !seen.insert(&pair[0]) {
            return Err(failure("unknown or duplicate option for this auth command"));
        }
    }
    Ok(())
}
/// What the local owner, and the default owner surface `nessa-panel`, may do:
/// read the gateway, read and write its conversations, and manage credentials.
/// Reading conversations is also what lets the owner propose a device's read
/// consent when pairing (design row S17 in `docs/design/auth/device-pairing.md`).
const OWNER_GRANTS: &str = "server.read,conversation.read,conversation.write,credential.manage";

/// Each grant named once, and each one a surface may hold. Which actions that
/// is belongs to the policy bundle (`credential.hold`), asked here for the
/// surface's kind and path, the same question the registry asks when it
/// issues. The resource is a stand-in: holding depends on the action alone.
fn validate_grants(policy: &dyn GrantPolicyEvaluator, grants: &str) -> Result<(), RunError> {
    let mut seen = std::collections::HashSet::new();
    let resource = Resource::new(
        OrganizationId::new("local").map_err(failure)?,
        ResourceId::new("local").map_err(failure)?,
    );
    for action in grants.split(',') {
        if action.is_empty() || !seen.insert(action) {
            return Err(failure("grants must be distinct actions"));
        }
        let grant = Grant::new(
            Action::new(action).map_err(|_| failure("unsupported grant"))?,
            resource.clone(),
        );
        if policy.may_hold(
            PrincipalKind::Integration,
            &grant,
            IssuePath::SurfaceProvision,
        ) != Ok(Decision::Allow)
        {
            return Err(failure(format!("unsupported grant: {action}")));
        }
    }
    Ok(())
}

fn option_value<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, RunError> {
    if !args.len().is_multiple_of(2) {
        return Err(failure("options require a value"));
    }
    for pair in args.as_chunks::<2>().0.iter() {
        if !matches!(
            pair[0].as_str(),
            "--expires-at" | "--chat-grants" | "--surface-id" | "--grants"
        ) {
            return Err(failure("unknown auth option"));
        }
    }
    let values: Vec<_> = args
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|pair| pair[0] == name)
        .collect();
    if values.len() > 1 {
        return Err(failure("duplicate auth option"));
    }
    Ok(values.first().map(|pair| pair[1].as_str()))
}

fn expiry_option(args: &[String]) -> Result<Option<u64>, RunError> {
    option_value(args, "--expires-at")?
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|value| {
                    *value > SystemClock.unix_seconds() && *value <= 9_007_199_254_740_991
                })
                .ok_or_else(|| failure("expires-at must be future safe integer Unix seconds"))
        })
        .transpose()
}

/// The set `auth init` gave the owner and the `nessa-panel` surface before
/// row S17 added `conversation.read`.
const EARLIER_OWNER_GRANTS: [&str; 3] = ["server.read", "conversation.write", "credential.manage"];

/// Re-provision the `nessa-panel` surface with the owner set when the
/// credential it holds now was provisioned with the set before S17 (row S20 in
/// `docs/design/auth/device-pairing.md`). Without `conversation.read` the
/// window's sign-in cannot propose a device's read consent, so Pair a device
/// is refused. Any other set, including one chosen with `--chat-grants`, is
/// left as it is. Answers whether it re-provisioned.
pub(super) fn upgrade_panel(
    directory: &Path,
    registry_context: RegistryOpenContext,
) -> Result<bool, RunError> {
    let policy = grant_policy()?;
    let settings = super::runtime_config::RuntimeConfig::load(directory)?;
    let store = credential_registry::open(directory, settings.registry, policy, registry_context)?;
    let organizations = store.identity().map_err(failure)?.organization_ids;
    let mut earlier = false;
    for organization_id in organizations {
        let listed = store
            .list_sync(
                &nessa_auth::application::credential_admin::ListCredentialsRequest {
                    organization_id,
                },
            )
            .map_err(failure)?;
        earlier |= listed.iter().any(|credential| {
            let mut actions = credential
                .grants
                .iter()
                .map(|grant| grant.action.as_str())
                .collect::<Vec<_>>();
            actions.sort_unstable();
            actions.dedup();
            let mut wanted = EARLIER_OWNER_GRANTS;
            wanted.sort_unstable();
            credential.principal_id == "surface:nessa-panel"
                && credential.revoked_at.is_none()
                && actions == wanted
        });
    }
    if !earlier {
        return Ok(false);
    }
    provision(
        &store,
        directory,
        "nessa-panel",
        OWNER_GRANTS,
        None,
        SystemClock.unix_seconds(),
    )?;
    Ok(true)
}

fn provision_command(
    args: &[String],
    registry_context: RegistryOpenContext,
) -> Result<(), RunError> {
    let args = &args[2..];
    validate_options(args, &["--surface-id", "--grants", "--expires-at"])?;
    let surface =
        option_value(args, "--surface-id")?.ok_or_else(|| failure("--surface-id is required"))?;
    let defaults = if surface == "nessa-panel" {
        OWNER_GRANTS
    } else {
        "server.read"
    };
    let grants = option_value(args, "--grants")?.unwrap_or(defaults);
    let policy = grant_policy()?;
    validate_grants(policy.as_ref(), grants)?;
    let directory = Environment::auth_directory_from_system()?;
    prepare_auth_directory(&directory)?;
    let settings = super::runtime_config::RuntimeConfig::load(&directory)?;
    let store = credential_registry::open(&directory, settings.registry, policy, registry_context)?;
    provision(
        &store,
        &directory,
        surface,
        grants,
        expiry_option(args)?,
        SystemClock.unix_seconds(),
    )
}

fn provision(
    store: &LocalCredentialStore,
    directory: &Path,
    surface: &str,
    grants: &str,
    expires_at: Option<u64>,
    now: u64,
) -> Result<(), RunError> {
    if surface.is_empty()
        || surface.len() > 100
        || !surface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(failure(
            "surface-id must use letters, numbers, hyphens or underscores",
        ));
    }
    let surfaces = directory.join("surfaces");
    nessa_local_storage::create_directory(&surfaces).map_err(failure)?;
    let output = surfaces.join(format!("{surface}.token"));
    let temporary = surfaces.join(format!(".{}.token", Uuid::new_v4()));
    let mut file = private_output(&temporary).map_err(failure)?;
    let result = store.provision_surface(
        surface,
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        grants.split(',').map(str::to_owned).collect(),
        now,
        expires_at,
    );
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            drop(file);
            std::fs::remove_file(&temporary).map_err(failure)?;
            return Err(failure(error));
        }
    };
    let IssueCredentialOutcome::Issued { evidence, .. } = outcome else {
        return Err(failure(
            "surface credential already issued; secret unavailable",
        ));
    };
    file.write_all(evidence.expose_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(failure)?;
    nessa_local_storage::replace(&temporary, &output).map_err(failure)?;
    nessa_local_storage::sync_directory(&surfaces).map_err(failure)?;
    println!(
        "Local surface {surface} credential written to {}.",
        output.display()
    );
    Ok(())
}

fn parse(args: &[String]) -> Result<(bool, PathBuf), RunError> {
    match args {
        [auth, operation, flag, path]
            if auth == "auth"
                && matches!(operation.as_str(), "init" | "recover-owner")
                && flag == "--owner-token-file"
                && Path::new(path).is_absolute() =>
        {
            Ok((operation == "recover-owner", PathBuf::from(path)))
        }
        _ => Err(RunError::Authentication(
            "usage: nessa auth <init|recover-owner> --local --owner-token-file <new-absolute-path>"
                .into(),
        )),
    }
}

fn private_output(path: &Path) -> std::io::Result<File> {
    nessa_local_storage::open(path, nessa_local_storage::OpenMode::CreateNew)
}

fn prepare_auth_directory(path: &Path) -> Result<(), RunError> {
    nessa_local_storage::create_directory(path).map_err(failure)
}

fn failure(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_prepares_a_private_auth_root() {
        let temporary = tempfile::tempdir().unwrap();
        let auth = temporary.path().join("data/dev/auth");

        prepare_auth_directory(&auth).unwrap();

        nessa_local_storage::verify_directory(&auth).unwrap();
        LocalCredentialStore::open(
            &auth,
            "credentials.v1.json",
            std::sync::Arc::new(nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
    }

    #[test]
    fn only_explicit_offline_commands_and_absolute_output_are_accepted() {
        let parse_args =
            |args: &[&str]| parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let output = std::env::temp_dir().join("new-token");
        let output = output.to_str().unwrap();
        assert!(parse_args(&["auth", "init", "--owner-token-file", output]).is_ok());
        assert!(
            parse_args(&["auth", "recover-owner", "--owner-token-file", output])
                .unwrap()
                .0
        );
        assert!(parse_args(&["auth", "init", "--owner-token-file", "relative"]).is_err());
        assert!(parse_args(&["auth", "init", "--token", "secret"]).is_err());
    }

    /// `init` commits the owner before it provisions the panel, so the
    /// panel's grants are asked of the policy first: a grant it may not hold
    /// is refused while nothing has been written.
    #[test]
    fn a_surface_grant_the_policy_refuses_is_refused_before_anything_is_written() {
        let policy = grant_policy().unwrap();
        assert!(validate_grants(policy.as_ref(), OWNER_GRANTS).is_ok());
        assert!(validate_grants(policy.as_ref(), "server.read").is_ok());
        for grants in [
            "server.read,environment.serve",
            "conversation.reed",
            "server.read,server.read",
            "server.read,",
        ] {
            assert!(
                validate_grants(policy.as_ref(), grants).is_err(),
                "{grants}"
            );
        }
    }
}
