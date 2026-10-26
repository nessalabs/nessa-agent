//! The tunnel's private directory, `<namespace>/tunnel`: the provider's
//! program, its record and log, and a named tunnel's token. The token is
//! kept here and only here, never in `config.json` or a log: Settings writes
//! it (`pairing.setTunnel`), startup reads it, and `cloudflared` is pointed
//! at the file, so it never appears on a command line.
use crate::tunnel::application::TunnelToken;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// The directory's name beneath the namespace.
pub(crate) const TUNNEL_DIRECTORY: &str = "tunnel";
/// The token's file in it.
const TOKEN_FILE: &str = "token";

/// `<namespace>/tunnel`.
#[derive(Clone, Debug)]
pub(crate) struct TunnelFiles {
    directory: PathBuf,
}

impl TunnelFiles {
    pub(crate) fn new(namespace: &Path) -> Self {
        Self {
            directory: namespace.join(TUNNEL_DIRECTORY),
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Where the token is, for the program that reads it.
    pub(crate) fn token_path(&self) -> PathBuf {
        self.directory.join(TOKEN_FILE)
    }

    /// The saved token, if any. A file that is not a token is an error, not
    /// an absent token.
    pub(crate) fn read_token(&self) -> io::Result<Option<TunnelToken>> {
        let file = match open(&self.token_path(), OpenMode::Read) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(TunnelToken::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        String::from_utf8(bytes)
            .ok()
            .and_then(TunnelToken::new)
            .map(Some)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
    }

    /// Replace the token, privately and synced.
    pub(crate) fn write_token(&self, token: &TunnelToken) -> io::Result<()> {
        create_directory(&self.directory)?;
        let mut file = PrivateTempFile::new_in(&self.directory)?;
        file.as_file_mut().write_all(token.expose().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&self.token_path())?;
        sync_directory(&self.directory)
    }

    /// Remove the token, if there is one.
    pub(crate) fn remove_token(&self) -> io::Result<()> {
        match std::fs::remove_file(self.token_path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => sync_directory(&self.directory),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_token_is_kept_privately_and_read_back() {
        let root = tempfile::tempdir().unwrap();
        let files = TunnelFiles::new(root.path());
        assert!(files.read_token().unwrap().is_none());
        let token = TunnelToken::new("eyJhIjoiYiJ9".into()).unwrap();
        files.write_token(&token).unwrap();
        assert_eq!(files.read_token().unwrap(), Some(token));
        let mode = std::fs::metadata(files.token_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn a_file_that_is_not_a_token_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let files = TunnelFiles::new(root.path());
        files
            .write_token(&TunnelToken::new("placeholder".into()).unwrap())
            .unwrap();
        std::fs::write(files.token_path(), b"has a space").unwrap();
        assert_eq!(
            files.read_token().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        // One that others can read is not taken as the gateway's.
        std::fs::set_permissions(files.token_path(), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::write(files.token_path(), b"token").unwrap();
        assert!(files.read_token().is_err());
    }
}
