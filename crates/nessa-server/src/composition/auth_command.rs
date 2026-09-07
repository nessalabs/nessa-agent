//! Offline local owner provisioning. OS access and the exclusive registry lock
//! establish authority; these commands are never available through the gateway.
use super::local_auth::SystemClock;
use crate::{core::RunError, env::Environment};
use nessa_auth::{
    adapters::local::{BootstrapRequest, LocalCredentialStore},
    application::{
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        ports::Clock,
    },
};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Parse the explicit offline command; credential material never appears in argv.
pub(super) fn execute(args: &[String]) -> Result<(), RunError> {
    if args.get(1).is_some_and(|arg| arg == "provision-surface") {
        return provision_command(args);
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
    let chat_grants = option_value(&args[args.len().min(4)..], "--chat-grants")?
        .unwrap_or("server.read,conversation.write,credential.manage");
    validate_grants(chat_grants)?;
    let directory = Environment::auth_directory_from_system()?;
    let settings = super::runtime_config::RuntimeConfig::load(&directory)?;
    let store = LocalCredentialStore::open_with_config(
        directory.join("credentials.v1.json"),
        settings.registry,
    )
    .map_err(failure)?;
    // Reserve a new protected output before changing durable state. Never overwrite.
    let mut file = private_output(&output).map_err(failure)?;
    let now = SystemClock.unix_seconds();
    let credential_id = Uuid::new_v4().to_string();
    let outcome = if recover {
        store.recover_owner(credential_id, now, expires_at)
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
            grants: ["server.read", "conversation.write", "credential.manage"]
                .into_iter()
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
        .map_err(|_| RunError::Authentication("credential committed but token output failed; run auth recover-owner with a new output path".into()))?;
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
fn validate_grants(grants: &str) -> Result<(), RunError> {
    let mut seen = std::collections::HashSet::new();
    if !grants.split(',').all(|grant| {
        matches!(
            grant,
            "server.read" | "conversation.write" | "credential.manage"
        ) && seen.insert(grant)
    }) {
        return Err(failure("grants must be distinct supported actions"));
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

fn provision_command(args: &[String]) -> Result<(), RunError> {
    let args = &args[2..];
    validate_options(args, &["--surface-id", "--grants", "--expires-at"])?;
    let surface =
        option_value(args, "--surface-id")?.ok_or_else(|| failure("--surface-id is required"))?;
    let defaults = if surface == "nessa-panel" {
        "server.read,conversation.write,credential.manage"
    } else {
        "server.read"
    };
    let grants = option_value(args, "--grants")?.unwrap_or(defaults);
    validate_grants(grants)?;
    let directory = Environment::auth_directory_from_system()?;
    let settings = super::runtime_config::RuntimeConfig::load(&directory)?;
    let store = LocalCredentialStore::open_with_config(
        directory.join("credentials.v1.json"),
        settings.registry,
    )
    .map_err(failure)?;
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
    let nessa_auth::application::credential_admin::IssueCredentialOutcome::Issued {
        evidence, ..
    } = outcome
    else {
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
            "usage: nessa-server auth <init|recover-owner> --owner-token-file <new-absolute-path>"
                .into(),
        )),
    }
}

fn private_output(path: &Path) -> std::io::Result<File> {
    nessa_local_storage::open(path, nessa_local_storage::OpenMode::CreateNew)
}

fn failure(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
