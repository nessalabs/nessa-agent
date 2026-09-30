//! Private command configuration names existing publications and credential files.
use nessa_gateway_endpoint::{
    application::DiscoverGatewayEndpoint, domain::GatewayEndpoint,
    infrastructure::FileEndpointDiscovery,
};
use nessa_local_storage::OpenMode;
use nessa_sync::replication::domain::Id;
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

const MAX_PROFILE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileError {
    Unavailable,
    TooLarge,
    Invalid,
    EndpointUnavailable,
    CredentialUnavailable,
    CredentialTooLarge,
    CredentialEncoding,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileDocument {
    receiver: String,
    access_epoch: u64,
    credential_file: PathBuf,
    endpoint_root: PathBuf,
    endpoint_directory: PathBuf,
}

pub(super) struct Profile {
    pub(super) receiver: Id,
    pub(super) access_epoch: u64,
    credential_file: PathBuf,
    endpoint_root: PathBuf,
    endpoint_directory: PathBuf,
}

impl Profile {
    pub(super) fn load(path: &Path) -> Result<Self, ProfileError> {
        let bytes = read_private(path, MAX_PROFILE_BYTES)?;
        let document: ProfileDocument =
            serde_json::from_slice(&bytes).map_err(|_| ProfileError::Invalid)?;
        if !document.credential_file.is_absolute() || !document.endpoint_root.is_absolute() {
            return Err(ProfileError::Invalid);
        }
        Ok(Self {
            receiver: Id::new(document.receiver).map_err(|_| ProfileError::Invalid)?,
            access_epoch: document.access_epoch,
            credential_file: document.credential_file,
            endpoint_root: document.endpoint_root,
            endpoint_directory: document.endpoint_directory,
        })
    }

    pub(super) fn endpoint(&self) -> Result<GatewayEndpoint, ProfileError> {
        DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(
            self.endpoint_root.clone(),
            self.endpoint_directory.clone(),
        ))
        .execute()
        .map_err(|_| ProfileError::EndpointUnavailable)?
        .ok_or(ProfileError::EndpointUnavailable)
    }

    /// The caller supplies the generated wire character ceiling; the session
    /// and authentication owners still decide the borrowed text's validity.
    pub(super) fn credential(&self, max_characters: usize) -> Result<String, ProfileError> {
        let bytes = max_characters
            .checked_mul(4)
            .ok_or(ProfileError::CredentialTooLarge)?;
        let value = read_private(&self.credential_file, bytes).map_err(|error| match error {
            ProfileError::TooLarge => ProfileError::CredentialTooLarge,
            _ => ProfileError::CredentialUnavailable,
        })?;
        let value = String::from_utf8(value).map_err(|_| ProfileError::CredentialEncoding)?;
        Ok(value.trim().to_owned())
    }
}

fn read_private(path: &Path, maximum: usize) -> Result<Vec<u8>, ProfileError> {
    let file = nessa_local_storage::open(path, OpenMode::ReadNonblocking)
        .map_err(|_| ProfileError::Unavailable)?;
    let witness = maximum
        .checked_add(1)
        .and_then(|limit| u64::try_from(limit).ok())
        .ok_or(ProfileError::TooLarge)?;
    let mut bytes = Vec::new();
    file.take(witness)
        .read_to_end(&mut bytes)
        .map_err(|_| ProfileError::Unavailable)?;
    if bytes.len() > maximum {
        return Err(ProfileError::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../../tests/composition/read_only_profile.rs"]
mod tests;
