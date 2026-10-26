use crate::{
    application::{Entropy, Repository},
    contract,
    domain::{EntryName, Failure, Passphrase, Secret, VaultKey},
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use nessa_local_storage::{create_directory, open, OpenMode, PrivateTempFile};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
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
/// One bounded encrypted file. All mutations reload durable truth under a shared
/// private lock before atomic replacement, so competing processes cannot lose updates.
pub struct EncryptedFileVault {
    directory: PathBuf,
    entropy: Arc<dyn Entropy>,
}
impl EncryptedFileVault {
    pub fn new(directory: PathBuf, entropy: Arc<dyn Entropy>) -> Self {
        Self { directory, entropy }
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
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((contract::MAXIMUM_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > contract::MAXIMUM_FILE_BYTES
            || bytes.len() < MAGIC.len() + SALT + NONCE + 16
            || !bytes.starts_with(MAGIC)
        {
            return Err(Failure::Unavailable);
        }
        Ok(Some(bytes))
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
        let mut key = Zeroizing::new([0; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.bytes(), salt, &mut *key)
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
            let bytes = Zeroizing::new(
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
        let mut temporary = PrivateTempFile::new_in(&self.directory).map_err(io)?;
        temporary.as_file_mut().write_all(bytes).map_err(io)?;
        temporary.as_file().sync_all().map_err(io)?;
        if new {
            temporary.publish(&self.path()).map_err(io)?;
        } else {
            temporary.persist(&self.path()).map_err(io)?;
        }
        nessa_local_storage::sync_directory(&self.directory).map_err(io)?;
        if self.required()? != bytes {
            return Err(Failure::Unavailable);
        }
        Ok(())
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
                entries.insert(name.clone(), Zeroizing::new(value.to_vec()));
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
