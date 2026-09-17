//! Durable authority for retrying a service installation that this host bootstrapped.
//!
//! The record is useful only while the per-label reconciliation lock is held. It
//! binds the launchd service to the complete desired definition, including the
//! runtime fingerprint and service generation. The plist on disk is checked too,
//! but never acts as authority by itself.
use super::control::atomic_write;
use nessa_local_storage::OpenMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{ErrorKind, Read},
    path::Path,
};

const FILE_NAME: &str = "install-attempt.json";
const MAX_BYTES: u64 = 65_536;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallAttempt {
    service: String,
    fingerprint: String,
    generation: String,
    definition: Value,
}

pub(super) fn publish(directory: &Path, service: &str, definition: &Value) -> Result<(), String> {
    let (fingerprint, generation) = definition_identity(definition)?;
    let bytes = serde_json::to_vec(&InstallAttempt {
        service: service.into(),
        fingerprint: fingerprint.into(),
        generation: generation.into(),
        definition: definition.clone(),
    })
    .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Gateway install attempt exceeds limit".into());
    }
    atomic_write(&directory.join(FILE_NAME), &bytes)
}

/// Returns true only when host-owned evidence and both definitions agree on the
/// exact service, runtime fingerprint and generation.
pub(super) fn authorizes_rebootstrap(
    directory: &Path,
    service: &str,
    desired: &Value,
    installed: Option<&Value>,
) -> Result<bool, String> {
    let Some(installed) = installed else {
        return Ok(false);
    };
    if installed != desired {
        return Ok(false);
    }
    let attempt = match read(directory)? {
        Some(attempt) => attempt,
        None => return Ok(false),
    };
    let (fingerprint, generation) = definition_identity(desired)?;
    Ok(attempt.service == service
        && attempt.fingerprint == fingerprint
        && attempt.generation == generation
        && attempt.definition == *desired)
}

pub(super) fn clear(directory: &Path) -> Result<(), String> {
    match fs::remove_file(directory.join(FILE_NAME)) {
        Ok(()) => nessa_local_storage::sync_directory(directory).map_err(|error| error.to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn read(directory: &Path) -> Result<Option<InstallAttempt>, String> {
    let file =
        match nessa_local_storage::open(&directory.join(FILE_NAME), OpenMode::ReadNonblocking) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "Cannot validate gateway install attempt; service preserved: {error}"
                ))
            }
        };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Gateway install attempt exceeds limit; service preserved".into());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        format!("Cannot validate gateway install attempt; service preserved: {error}")
    })
}

fn definition_identity(definition: &Value) -> Result<(&str, &str), String> {
    let environment = definition
        .get("EnvironmentVariables")
        .and_then(Value::as_object)
        .ok_or("Gateway definition has no environment")?;
    let fingerprint = environment
        .get("NESSA_RUNTIME_FINGERPRINT")
        .and_then(Value::as_str)
        .filter(|value| sha256(value))
        .ok_or("Gateway definition has no valid runtime fingerprint")?;
    let generation = environment
        .get("NESSA_SERVICE_GENERATION")
        .and_then(Value::as_str)
        .filter(|value| sha256(value))
        .ok_or("Gateway definition has no valid service generation")?;
    Ok((fingerprint, generation))
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/install_attempt.rs"]
mod tests;
