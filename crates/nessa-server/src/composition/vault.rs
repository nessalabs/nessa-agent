//! One vault before any provider is configured. No native store is opened here.
use crate::product::VaultRoutes;
use crate::timer::clock;
use nessa_vault::{
    application::{Caller, Clock, ObservationClock, Session, Vault},
    domain::{Failure as VaultFailure, Passphrase, Status as VaultStatus},
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
use std::{
    fmt,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use zeroize::Zeroizing;

/// Upper bound on a headless passphrase file read (larger than any valid
/// passphrase, bounded so a misconfigured path cannot allocate unbounded RAM).
const MAX_PASSPHRASE_FILE_BYTES: u64 = 4096;

struct Monotonic(Instant);
impl Clock for Monotonic {
    fn seconds(&self) -> u64 {
        self.0.elapsed().as_secs()
    }
}
pub(super) fn compose(namespace: &Path) -> Arc<VaultRoutes> {
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            namespace.join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(
            namespace.join("vault/audit"),
            Arc::new(WallClock),
        )),
        Arc::new(Monotonic(clock::os::now())),
    );
    Arc::new(VaultRoutes {
        vault,
        migration: Arc::new(crate::vault::infrastructure::LegacyGithubMigration::new(
            namespace.to_path_buf(),
            crate::secrets::KeychainSecret::new(namespace, "github-sign-in")
                .map(|entry| Arc::new(entry) as Arc<dyn crate::secrets::SecretStore>),
            crate::secrets::KeychainSecret::new(namespace, "github-inbox-key")
                .map(|entry| Arc::new(entry) as Arc<dyn crate::secrets::SecretStore>),
        )),
    })
}

/// Typed failures when bootstrapping the encrypted vault for headless startup.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HeadlessVaultError {
    NotRegularFile,
    #[cfg(unix)]
    InsecurePermissions,
    TooLarge,
    ReadFailed,
    InvalidUtf8,
    Vault(VaultFailure),
}

impl fmt::Display for HeadlessVaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegularFile => {
                f.write_str("vault passphrase file must be a regular, non-symlink file")
            }
            #[cfg(unix)]
            Self::InsecurePermissions => {
                f.write_str("vault passphrase file must not be accessible to group or other users")
            }
            Self::TooLarge => f.write_str("vault passphrase file exceeds maximum size"),
            Self::ReadFailed => f.write_str("could not read vault passphrase file"),
            Self::InvalidUtf8 => f.write_str("vault passphrase file is not valid UTF-8"),
            Self::Vault(failure) => write!(f, "headless vault bootstrap failed: {failure:?}"),
        }
    }
}

/// Seam for reading a headless vault passphrase from outside the process.
pub(super) trait PassphraseSource {
    fn read(&self) -> Result<Zeroizing<String>, HeadlessVaultError>;
}

/// Reads a vault passphrase from a private (`0600`/`0400` on Unix), regular,
/// non-symlink file, trimming at most one trailing line ending (`\r\n` or `\n`).
pub(super) struct FilePassphraseSource(pub(super) PathBuf);

impl PassphraseSource for FilePassphraseSource {
    fn read(&self) -> Result<Zeroizing<String>, HeadlessVaultError> {
        #[cfg(not(unix))]
        {
            let link_meta =
                std::fs::symlink_metadata(&self.0).map_err(|_| HeadlessVaultError::ReadFailed)?;
            if !link_meta.file_type().is_file() {
                return Err(HeadlessVaultError::NotRegularFile);
            }
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(&self.0).map_err(|error| {
            #[cfg(unix)]
            if error.raw_os_error() == Some(libc::ELOOP) {
                return HeadlessVaultError::NotRegularFile;
            }
            let _ = error;
            HeadlessVaultError::ReadFailed
        })?;
        let metadata = file
            .metadata()
            .map_err(|_| HeadlessVaultError::ReadFailed)?;
        if !metadata.is_file() {
            return Err(HeadlessVaultError::NotRegularFile);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(HeadlessVaultError::InsecurePermissions);
            }
        }
        if metadata.len() > MAX_PASSPHRASE_FILE_BYTES {
            return Err(HeadlessVaultError::TooLarge);
        }
        // Pre-allocate full bounded capacity so `read_to_end` never reallocates
        // and leaves unzeroized passphrase copies on the heap.
        let mut raw = Zeroizing::new(Vec::with_capacity(MAX_PASSPHRASE_FILE_BYTES as usize + 1));
        file.take(MAX_PASSPHRASE_FILE_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|_| HeadlessVaultError::ReadFailed)?;
        if raw.len() as u64 > MAX_PASSPHRASE_FILE_BYTES {
            return Err(HeadlessVaultError::TooLarge);
        }
        // Validate UTF-8 while bytes are still owned by `Zeroizing` so an
        // invalid-UTF-8 error never drops unzeroized bytes.
        if std::str::from_utf8(&raw).is_err() {
            return Err(HeadlessVaultError::InvalidUtf8);
        }
        let mut text = Zeroizing::new(
            String::from_utf8(std::mem::take(&mut *raw))
                .map_err(|_| HeadlessVaultError::InvalidUtf8)?,
        );
        if text.ends_with("\r\n") {
            let len = text.len() - 2;
            text.truncate(len);
        } else if text.ends_with('\n') {
            let len = text.len() - 1;
            text.truncate(len);
        }
        Ok(text)
    }
}

/// Initialize (on first boot) or unlock the encrypted vault for a headless
/// server, returning the owning `Session` that keeps the vault unlocked until
/// shutdown. Fails closed on any read, policy, or decryption error.
pub(super) fn bootstrap_headless(
    vault: &Arc<Vault>,
    source: &dyn PassphraseSource,
) -> Result<Session, HeadlessVaultError> {
    let session = vault.session(
        format!("headless-startup-{}", uuid::Uuid::new_v4()),
        Caller {
            principal: "headless-bootstrap".into(),
            credential: "startup-passphrase-file".into(),
        },
    );
    let lease = session.lease();
    let status = vault.status().map_err(HeadlessVaultError::Vault)?;
    match status {
        VaultStatus::Unlocked => {
            tracing::info!("headless vault already unlocked");
            Ok(session)
        }
        VaultStatus::Unconfigured => {
            let mut raw = source.read()?;
            let passphrase =
                Passphrase::new(std::mem::take(&mut *raw)).map_err(HeadlessVaultError::Vault)?;
            let after = vault
                .create(&lease, passphrase, &[])
                .map_err(HeadlessVaultError::Vault)?;
            tracing::info!(?after, "headless vault initialized and unlocked");
            Ok(session)
        }
        VaultStatus::Locked => {
            let mut raw = source.read()?;
            let passphrase =
                Passphrase::new(std::mem::take(&mut *raw)).map_err(HeadlessVaultError::Vault)?;
            let after = vault
                .unlock(&lease, passphrase)
                .map_err(HeadlessVaultError::Vault)?;
            tracing::info!(?after, "headless vault unlocked");
            Ok(session)
        }
    }
}

struct WallClock;
impl ObservationClock for WallClock {
    fn unix_millis(&self) -> Result<u64, nessa_vault::domain::Failure> {
        clock::wall_now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .ok_or(nessa_vault::domain::Failure::AuditUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedPassphrase(Result<String, fn() -> HeadlessVaultError>);

    impl PassphraseSource for FixedPassphrase {
        fn read(&self) -> Result<Zeroizing<String>, HeadlessVaultError> {
            match &self.0 {
                Ok(value) => Ok(Zeroizing::new(value.clone())),
                Err(make) => Err(make()),
            }
        }
    }

    fn write_private(path: &Path, contents: &[u8]) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn bootstrap_headless_initializes_then_unlocks_and_locks_on_close_or_drop() {
        let dir = tempfile::tempdir().unwrap();
        let routes = compose(dir.path());

        // Weak passphrase on first boot fails closed with policy error.
        let weak = FixedPassphrase(Ok("short".into()));
        assert!(matches!(
            bootstrap_headless(&routes.vault, &weak).err().unwrap(),
            HeadlessVaultError::Vault(VaultFailure::PassphrasePolicy)
        ));
        assert!(!routes.vault.is_unlocked());

        // First boot: Unconfigured -> creates and unlocks.
        let good = FixedPassphrase(Ok("correct-horse-battery-staple".into()));
        let mut first = bootstrap_headless(&routes.vault, &good).unwrap();
        assert!(routes.vault.is_unlocked());

        // Already-unlocked vault is a no-op that does not re-read the source.
        let failing = FixedPassphrase(Err(|| HeadlessVaultError::ReadFailed));
        let mut nested = bootstrap_headless(&routes.vault, &failing).unwrap();
        assert!(routes.vault.is_unlocked());
        nested.close().unwrap();

        first.close().unwrap();
        assert!(!routes.vault.is_unlocked());

        // Wrong passphrase on locked vault fails closed.
        let wrong = FixedPassphrase(Ok("wrong-horse-battery-staple".into()));
        let err = bootstrap_headless(&routes.vault, &wrong).err().unwrap();
        assert!(matches!(
            err,
            HeadlessVaultError::Vault(VaultFailure::WrongPassphrase)
        ));
        assert!(!routes.vault.is_unlocked());

        // Second boot: Locked -> unlocks existing vault; dropping Session (as
        // on a subsequent startup failure) locks and wipes key material.
        {
            let _second = bootstrap_headless(&routes.vault, &good).unwrap();
            assert!(routes.vault.is_unlocked());
        }
        assert!(!routes.vault.is_unlocked());
    }

    #[test]
    fn file_passphrase_source_trims_one_newline_and_refuses_bad_files() {
        let dir = tempfile::tempdir().unwrap();

        let lf = dir.path().join("lf");
        write_private(&lf, b"passphrase-with-lf\n");
        assert_eq!(
            FilePassphraseSource(lf).read().unwrap().as_str(),
            "passphrase-with-lf"
        );

        let crlf = dir.path().join("crlf");
        write_private(&crlf, b"passphrase-with-crlf\r\n");
        assert_eq!(
            FilePassphraseSource(crlf).read().unwrap().as_str(),
            "passphrase-with-crlf"
        );

        let multi_nl = dir.path().join("multi-nl");
        write_private(&multi_nl, b"passphrase-with-two\n\n");
        assert_eq!(
            FilePassphraseSource(multi_nl).read().unwrap().as_str(),
            "passphrase-with-two\n"
        );

        let oversized = dir.path().join("big");
        write_private(
            &oversized,
            &vec![b'a'; (MAX_PASSPHRASE_FILE_BYTES + 1) as usize],
        );
        assert_eq!(
            FilePassphraseSource(oversized).read().unwrap_err(),
            HeadlessVaultError::TooLarge
        );

        let bad_utf8 = dir.path().join("bad-utf8");
        write_private(&bad_utf8, &[0xff, 0xfe]);
        assert_eq!(
            FilePassphraseSource(bad_utf8).read().unwrap_err(),
            HeadlessVaultError::InvalidUtf8
        );

        assert_eq!(
            FilePassphraseSource(dir.path().to_path_buf())
                .read()
                .unwrap_err(),
            HeadlessVaultError::NotRegularFile
        );

        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            use std::os::unix::fs::PermissionsExt;

            let world_readable = dir.path().join("world-readable");
            std::fs::write(&world_readable, b"secret-passphrase\n").unwrap();
            std::fs::set_permissions(&world_readable, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            assert_eq!(
                FilePassphraseSource(world_readable).read().unwrap_err(),
                HeadlessVaultError::InsecurePermissions
            );

            let target = dir.path().join("target");
            write_private(&target, b"secret-passphrase\n");
            let link = dir.path().join("symlink");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(
                FilePassphraseSource(link).read().unwrap_err(),
                HeadlessVaultError::NotRegularFile
            );

            let fifo = dir.path().join("fifo");
            let c_fifo = CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);
            assert_eq!(
                FilePassphraseSource(fifo).read().unwrap_err(),
                HeadlessVaultError::NotRegularFile
            );
        }
    }
}
