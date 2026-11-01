use crate::{
    application::{Entropy, Repository},
    contract,
    domain::{ChangePublication, EntryName, Failure, Passphrase, Secret, VaultKey},
};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use nessa_local_storage::{create_directory, open, OpenMode, PrivateTempFile};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;
const MAGIC: &[u8] = b"NESSAVAULT01";
const SALT: usize = 16;
const NONCE: usize = 24;
/// Native entropy, supplied at composition rather than selected by application code.
pub struct OsEntropy;
impl Entropy for OsEntropy {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), Failure> {
        getrandom::fill(bytes).map_err(|_| Failure::Unavailable)
    }
}
/// Atomic ciphertext-file effects, separated so publication failures can be
/// tested before and after replacement without touching any real user vault.
/// The adapter reports IO facts; the repository owns publication classification.
pub trait FilePublication: Send + Sync {
    fn prepare(&self, directory: &Path, bytes: &[u8]) -> Result<PrivateTempFile, Failure>;
    fn replace(&self, temporary: PrivateTempFile, destination: &Path) -> Result<(), Failure>;
    fn publish_new(&self, temporary: PrivateTempFile, destination: &Path) -> Result<(), Failure>;
    fn sync(&self, directory: &Path) -> Result<(), Failure>;
}
/// The existing private-file publication and platform durability implementation.
pub struct NativeFilePublication;
impl FilePublication for NativeFilePublication {
    fn prepare(&self, directory: &Path, bytes: &[u8]) -> Result<PrivateTempFile, Failure> {
        let mut temporary = PrivateTempFile::new_in(directory).map_err(io)?;
        temporary.as_file_mut().write_all(bytes).map_err(io)?;
        temporary.as_file().sync_all().map_err(io)?;
        Ok(temporary)
    }
    fn replace(&self, temporary: PrivateTempFile, destination: &Path) -> Result<(), Failure> {
        temporary.persist(destination).map_err(io)
    }
    fn publish_new(&self, temporary: PrivateTempFile, destination: &Path) -> Result<(), Failure> {
        temporary.publish(destination).map_err(io)
    }
    fn sync(&self, directory: &Path) -> Result<(), Failure> {
        nessa_local_storage::sync_directory(directory).map_err(io)
    }
}
/// One bounded encrypted file. All mutations reload durable truth under a shared
/// private lock before atomic replacement, so competing processes cannot lose updates.
pub struct EncryptedFileVault {
    directory: PathBuf,
    entropy: Arc<dyn Entropy>,
    publication: Arc<dyn FilePublication>,
}
impl EncryptedFileVault {
    pub fn new(directory: PathBuf, entropy: Arc<dyn Entropy>) -> Self {
        Self::with_publication(directory, entropy, Arc::new(NativeFilePublication))
    }
    /// Inject ciphertext publication effects, preserving the same repository rules.
    pub fn with_publication(
        directory: PathBuf,
        entropy: Arc<dyn Entropy>,
        publication: Arc<dyn FilePublication>,
    ) -> Self {
        Self {
            directory,
            entropy,
            publication,
        }
    }
    fn path(&self) -> PathBuf {
        self.directory.join("entries.enc")
    }
    fn lock(&self) -> Result<File, Failure> {
        create_directory(&self.directory).map_err(io)?;
        let file =
            open(&self.directory.join("entries.lock"), OpenMode::OpenOrCreate).map_err(io)?;
        file.lock().map_err(io)?;
        Ok(file)
    }
    fn load(&self) -> Result<Option<Vec<u8>>, Failure> {
        let mut file = match open(&self.path(), OpenMode::Read) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Failure::Unavailable),
        };
        Self::read_bytes(&mut file).map(Some)
    }
    fn read_bytes(file: &mut File) -> Result<Vec<u8>, Failure> {
        let mut bytes = Vec::new();
        Read::by_ref(file)
            .take((contract::MAXIMUM_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > contract::MAXIMUM_FILE_BYTES
            || bytes.len() < MAGIC.len() + SALT + NONCE + 16
            || !bytes.starts_with(MAGIC)
        {
            return Err(Failure::Unavailable);
        }
        Ok(bytes)
    }
    fn required(&self) -> Result<Vec<u8>, Failure> {
        self.load()?.ok_or(Failure::Unavailable)
    }
    fn derive(passphrase: &Passphrase, salt: &[u8]) -> Result<VaultKey, Failure> {
        let params = Params::new(
            contract::KDF_MEMORY_KI_B as u32,
            contract::KDF_ITERATIONS as u32,
            contract::KDF_PARALLELISM as u32,
            Some(32),
        )
        .map_err(|_| Failure::Unavailable)?;
        // Argon2's allocating convenience method does not erase its block Vec.
        // Own the scratch allocation so it is erased on success, error and unwind.
        let mut memory = Zeroizing::new(vec![Block::default(); params.block_count()]);
        let mut key = Zeroizing::new([0; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into_with_memory(passphrase.bytes(), salt, &mut *key, &mut *memory)
            .map_err(|_| Failure::Unavailable)?;
        Ok(VaultKey(key))
    }
    fn entries(bytes: &[u8], key: &VaultKey) -> Result<BTreeMap<EntryName, Secret>, Failure> {
        let boundary = MAGIC.len() + SALT;
        let nonce = XNonce::from_slice(&bytes[boundary..boundary + NONCE]);
        let cipher =
            XChaCha20Poly1305::new_from_slice(key.0.as_ref()).map_err(|_| Failure::Unavailable)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    nonce,
                    Payload {
                        msg: &bytes[boundary + NONCE..],
                        aad: &bytes[..boundary],
                    },
                )
                .map_err(|_| Failure::WrongPassphrase)?,
        );
        let encoded: BTreeMap<String, SecretText> =
            serde_json::from_slice(&plaintext).map_err(|_| Failure::Unavailable)?;
        if encoded.len() > contract::MAXIMUM_ENTRIES {
            return Err(Failure::Unavailable);
        }
        let mut entries = BTreeMap::new();
        for (name, value) in encoded {
            let name = EntryName::new(name)?;
            let value = value.0;
            let bytes = Secret::new(
                STANDARD
                    .decode(value.as_bytes())
                    .map_err(|_| Failure::Unavailable)?,
            );
            if bytes.len() > contract::MAXIMUM_ENTRY_BYTES {
                return Err(Failure::Unavailable);
            }
            entries.insert(name, bytes);
        }
        Ok(entries)
    }
    fn seal(
        &self,
        salt: &[u8],
        entries: &BTreeMap<EntryName, Secret>,
        key: &VaultKey,
    ) -> Result<Vec<u8>, Failure> {
        if entries.len() > contract::MAXIMUM_ENTRIES
            || entries
                .values()
                .any(|value| value.len() > contract::MAXIMUM_ENTRY_BYTES)
        {
            return Err(Failure::Capacity);
        }
        let encoded: BTreeMap<&str, Zeroizing<String>> = entries
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str(),
                    Zeroizing::new(STANDARD.encode(value.as_slice())),
                )
            })
            .collect();
        // Serialize references; every owned base64 value and plaintext buffer is zeroizing.
        let exposed: BTreeMap<&str, &str> = encoded
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let plaintext =
            Zeroizing::new(serde_json::to_vec(&exposed).map_err(|_| Failure::Unavailable)?);
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(salt);
        let mut nonce = [0; NONCE];
        self.entropy.fill(&mut nonce)?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(key.0.as_ref()).map_err(|_| Failure::Unavailable)?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: &bytes,
                },
            )
            .map_err(|_| Failure::Unavailable)?;
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&ciphertext);
        if bytes.len() > contract::MAXIMUM_FILE_BYTES {
            return Err(Failure::Capacity);
        }
        Ok(bytes)
    }
    fn persist(&self, bytes: &[u8], new: bool) -> Result<(), Failure> {
        let temporary = self.publication.prepare(&self.directory, bytes)?;
        if new {
            self.publication.publish_new(temporary, &self.path())?;
        } else {
            self.publication.replace(temporary, &self.path())?;
        }
        self.publication.sync(&self.directory)?;
        if self.required()? != bytes {
            return Err(Failure::Unavailable);
        }
        Ok(())
    }
    fn prepared_change(
        &self,
        current_key: &VaultKey,
        new_passphrase: &Passphrase,
    ) -> Result<(PrivateTempFile, Vec<u8>), Failure> {
        let previous = self.required()?;
        let entries = Self::entries(&previous, current_key).map_err(|_| Failure::Unavailable)?;
        let mut salt = [0; SALT];
        self.entropy.fill(&mut salt)?;
        let new_key = Self::derive(new_passphrase, &salt)?;
        let bytes = self.seal(&salt, &entries, &new_key)?;
        let mut temporary = self.publication.prepare(&self.directory, &bytes)?;
        temporary
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(io)?;
        let prepared = Self::read_bytes(temporary.as_file_mut())?;
        if prepared != bytes {
            return Err(Failure::Unavailable);
        }
        let verified = Self::entries(&prepared, &new_key).map_err(|_| Failure::Unavailable)?;
        if verified.len() != entries.len()
            || verified.iter().zip(entries.iter()).any(
                |((actual_name, actual), (expected_name, expected))| {
                    actual_name != expected_name || actual.as_slice() != expected.as_slice()
                },
            )
        {
            return Err(Failure::Unavailable);
        }
        Ok((temporary, bytes))
    }
    fn modify(
        &self,
        key: &VaultKey,
        name: &EntryName,
        value: Option<&[u8]>,
    ) -> Result<(), Failure> {
        let _lock = self.lock()?;
        let previous = self.required()?;
        let mut entries = Self::entries(&previous, key).map_err(|_| Failure::Unavailable)?;
        match value {
            Some(value) => {
                entries.insert(name.clone(), Secret::new(value.to_vec()));
            }
            None => {
                if entries.remove(name).is_none() {
                    return Ok(());
                }
            }
        }
        let bytes = self.seal(&previous[MAGIC.len()..MAGIC.len() + SALT], &entries, key)?;
        self.persist(&bytes, false)
    }
}
fn io(_: std::io::Error) -> Failure {
    Failure::Unavailable
}
impl Repository for EncryptedFileVault {
    fn configured(&self) -> Result<bool, Failure> {
        Ok(self.load()?.is_some())
    }
    fn create(
        &self,
        passphrase: &Passphrase,
        initial: &[(EntryName, Secret)],
    ) -> Result<VaultKey, Failure> {
        passphrase.validate_setup()?;
        let _lock = self.lock()?;
        if self.load()?.is_some() {
            return Err(Failure::AlreadyConfigured);
        }
        let mut salt = [0; SALT];
        self.entropy.fill(&mut salt)?;
        let key = Self::derive(passphrase, &salt)?;
        let entries: BTreeMap<_, _> = initial
            .iter()
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect();
        if entries.len() != initial.len() {
            return Err(Failure::InvalidEntry);
        }
        let sealed = self.seal(&salt, &entries, &key)?;
        self.persist(&sealed, true)?;
        Self::entries(&self.required()?, &key)?;
        Ok(key)
    }
    fn unlock(&self, passphrase: &Passphrase) -> Result<VaultKey, Failure> {
        let _lock = self.lock()?;
        let bytes = self.required()?;
        let key = Self::derive(passphrase, &bytes[MAGIC.len()..MAGIC.len() + SALT])?;
        Self::entries(&bytes, &key)?;
        Ok(key)
    }
    fn change_passphrase(
        &self,
        current_key: &VaultKey,
        new_passphrase: &Passphrase,
    ) -> ChangePublication {
        if let Err(problem) = new_passphrase.validate_setup() {
            return ChangePublication::Unchanged(problem);
        }
        let _lock = match self.lock() {
            Ok(lock) => lock,
            Err(problem) => return ChangePublication::Unchanged(problem),
        };
        let (temporary, bytes) = match self.prepared_change(current_key, new_passphrase) {
            Ok(prepared) => prepared,
            Err(problem) => return ChangePublication::Unchanged(problem),
        };
        // From this call onward, an IO failure cannot prove that the old file won.
        let receipt = self
            .publication
            .replace(temporary, &self.path())
            .and_then(|()| self.publication.sync(&self.directory))
            .and_then(|()| self.required())
            .and_then(|actual| {
                if actual == bytes {
                    Ok(())
                } else {
                    Err(Failure::Unavailable)
                }
            });
        match receipt {
            Ok(()) => ChangePublication::Committed,
            Err(problem) => ChangePublication::Uncertain(problem),
        }
    }
    fn read(&self, key: &VaultKey, name: &EntryName) -> Result<Option<Secret>, Failure> {
        let _lock = self.lock()?;
        Ok(Self::entries(&self.required()?, key)
            .map_err(|_| Failure::Unavailable)?
            .remove(name))
    }
    fn write(&self, key: &VaultKey, name: &EntryName, value: &[u8]) -> Result<(), Failure> {
        if value.len() > contract::MAXIMUM_ENTRY_BYTES {
            return Err(Failure::Capacity);
        }
        self.modify(key, name, Some(value))
    }
    fn delete(&self, key: &VaultKey, name: &EntryName) -> Result<(), Failure> {
        self.modify(key, name, None)
    }
}

struct SecretText(Zeroizing<String>);
impl<'de> serde::Deserialize<'de> for SecretText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(
            <String as serde::Deserialize>::deserialize(deserializer)?,
        )))
    }
}
