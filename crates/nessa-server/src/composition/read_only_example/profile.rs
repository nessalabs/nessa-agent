//! Private command configuration: where this device keeps its enrollment
//! state, the one cache that holds its data, and the gateway's native address.
//! No secret is in it; the device key and issued credential live in the
//! private state directory it names. Online commands take no cache argument:
//! the profile's cache is the only one a Terminal status can purge, so no
//! other cache can be left holding a revoked device's data (design row PC3).
use nessa_auth::adapters::pairing::FilePairingState;
use nessa_local_storage::OpenMode;
use serde::Deserialize;
use std::{
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};

const MAX_PROFILE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileError {
    Unavailable,
    TooLarge,
    Invalid,
    PrivateStateUnavailable,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileDocument {
    state_root: PathBuf,
    state_directory: PathBuf,
    cache: PathBuf,
    gateway_address: SocketAddr,
}

pub(super) struct Profile {
    /// The gateway's native listener: a numeric address, never looked up.
    pub(super) gateway: SocketAddr,
    /// This device's private cache: the one its reads fill and its purge
    /// empties.
    pub(super) cache: PathBuf,
    state_root: PathBuf,
    state_directory: PathBuf,
}

impl Profile {
    pub(super) fn load(path: &Path) -> Result<Self, ProfileError> {
        let bytes = read_private(path, MAX_PROFILE_BYTES)?;
        let document: ProfileDocument =
            serde_json::from_slice(&bytes).map_err(|_| ProfileError::Invalid)?;
        if !document.state_root.is_absolute()
            || document.state_directory.is_absolute()
            || !document.cache.is_absolute()
        {
            return Err(ProfileError::Invalid);
        }
        Ok(Self {
            gateway: document.gateway_address,
            cache: document.cache,
            state_root: document.state_root,
            state_directory: document.state_directory,
        })
    }

    /// Open this device's private enrollment state, creating its directory
    /// beneath the root the first time. The state owner refuses unsafe
    /// existing storage and a second opener.
    pub(super) fn private_state(&self) -> Result<FilePairingState, ProfileError> {
        // The private-state owner wants a trusted absolute root; on Unix that
        // is the canonical spelling, as gateway composition opens its own.
        #[cfg(unix)]
        let root = self
            .state_root
            .canonicalize()
            .map_err(|_| ProfileError::PrivateStateUnavailable)?;
        #[cfg(not(unix))]
        let root = self.state_root.clone();
        nessa_local_storage::create_directory_beneath(&root, &self.state_directory)
            .map_err(|_| ProfileError::PrivateStateUnavailable)?;
        FilePairingState::open(&root, &self.state_directory)
            .map_err(|_| ProfileError::PrivateStateUnavailable)
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
