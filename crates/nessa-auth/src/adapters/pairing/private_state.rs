//! Bounded private binary state beneath one retained directory and lifetime lock.
use crate::{
    application::pairing::{
        ClientPendingStore, DeviceCredential, GatewayKeyStore, GatewayPublicationState,
        PendingEnrollment, PrivateKeyMaterial, PrivatePublicationEffect, PrivatePublicationError,
        PrivatePublicationStep, PrivateStateError, PrivateStorageFailure,
    },
    application::ports::Clock,
    domain::{
        pairing::{AttemptId, ConsentIntentId, InvitationId, PublicIntent},
        AudienceId, CredentialId, ResourceId, MAX_IDENTIFIER_BYTES,
    },
};
use fs2::FileExt;
use nessa_local_storage::{
    is_unsafe_file, OpenMode, PrivateDirectory, PrivatePublicationFailure, PrivatePublicationStage,
    PublishedPrivateFile,
};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Mutex,
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

const GATEWAY_FILE: &str = "gateway-key";
/// The client's one enrollment record: pending, or the credential that replaced it.
const ENROLLMENT_FILE: &str = "client-enrollment";
const LOCK_FILE: &str = "pairing-state.lock";
const KEY_BYTES: usize = 36;
const PENDING_BYTES: usize = 144;
const KEY_MAGIC: &[u8; 4] = b"NSGK";
const PENDING_MAGIC: &[u8; 4] = b"NSCP";
const CREDENTIAL_MAGIC: &[u8; 4] = b"NSCA";
/// A credential record: the pending layout, then two u16-prefixed identifiers,
/// each bounded by the identifier owner's own limit.
const CREDENTIAL_MAX_BYTES: usize = PENDING_BYTES + 2 * (2 + MAX_IDENTIFIER_BYTES);

/// One private storage owner. Its native lock and retained directory live through
/// every operation; `private_state_lock_excludes_second_handle` tests exclusion.
pub struct FilePairingState {
    directory: PrivateDirectory,
    lock: File,
    operation: Mutex<()>,
}
impl FilePairingState {
    /// Open an already private directory beneath a trusted absolute root.
    /// Directory creation belongs to composition; unsafe existing storage is refused.
    pub fn open(root: &Path, directory: &Path) -> Result<Self, PrivateStateError> {
        let directory = PrivateDirectory::open_beneath(root, directory).map_err(storage_error)?;
        let lock = directory
            .open_file(OsStr::new(LOCK_FILE), OpenMode::OpenOrCreate)
            .map_err(storage_error)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                PrivateStateError::Locked
            } else {
                storage_error(error)
            }
        })?;
        directory.sync().map_err(storage_error)?;
        Ok(Self {
            directory,
            lock,
            operation: Mutex::new(()),
        })
    }
    fn read(
        &self,
        name: &str,
        length: usize,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, PrivateStateError> {
        let bytes = self.read_bounded(name, length)?;
        if bytes.as_ref().is_some_and(|bytes| bytes.len() != length) {
            return Err(PrivateStateError::Corrupt);
        }
        Ok(bytes)
    }
    fn read_bounded(
        &self,
        name: &str,
        length: usize,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, PrivateStateError> {
        self.verify_lock()?;
        let mut file = match self
            .directory
            .open_file(OsStr::new(name), OpenMode::ReadNonblocking)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        let mut bytes = Zeroizing::new(Vec::with_capacity(length + 1));
        (&mut file)
            .take((length + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() > length {
            return Err(PrivateStateError::Corrupt);
        }
        if !self
            .directory
            .named_file_is(OsStr::new(name), &file)
            .map_err(storage_error)?
        {
            return Err(PrivateStateError::Corrupt);
        }
        self.verify_lock()?;
        Ok(Some(bytes))
    }
    fn verify_lock(&self) -> Result<(), PrivateStateError> {
        if self
            .directory
            .named_file_is(OsStr::new(LOCK_FILE), &self.lock)
            .map_err(storage_error)?
        {
            Ok(())
        } else {
            Err(PrivateStateError::Corrupt)
        }
    }
    fn publish(&self, name: &str, bytes: &[u8], replace: bool) -> Result<(), PrivateStateError> {
        self.verify_lock()?;
        let mut temporary = self.directory.reserve_temp().map_err(storage_error)?;
        temporary
            .as_file_mut()
            .write_all(bytes)
            .map_err(storage_error)?;
        let result = if replace {
            temporary.replace(OsStr::new(name))
        } else {
            temporary.publish_new(OsStr::new(name))
        };
        match result {
            Ok(published) => {
                let acknowledged = self.verify_lock().map_err(|_| PrivateStateError::Uncertain);
                drop(published);
                acknowledged
            }
            Err(failure) => self.publication_failure(name, bytes, failure),
        }
    }
    fn publication_failure(
        &self,
        name: &str,
        expected: &[u8],
        failure: PrivatePublicationFailure,
    ) -> Result<(), PrivateStateError> {
        let (step, primary, published, cleanup) = failure.into_parts();
        let step = publication_step(step);
        let primary = storage_failure(&primary);
        let cleanup = cleanup.as_ref().map(storage_failure);
        let (effect, reconciliation) = match published {
            Some(published) => match self.reconcile_live(name, expected, published) {
                Ok(()) => return Ok(()),
                Err(error) => (PrivatePublicationEffect::Published, Some(error)),
            },
            None => (PrivatePublicationEffect::NotPublished, None),
        };
        Err(PrivateStateError::Publication(
            PrivatePublicationError::new(step, primary, cleanup, effect, reconciliation),
        ))
    }
    fn reconcile_live(
        &self,
        name: &str,
        expected: &[u8],
        published: PublishedPrivateFile,
    ) -> Result<(), PrivateStorageFailure> {
        if published.name() != OsStr::new(name) {
            return Err(PrivateStorageFailure::UnsafeStorage);
        }
        // The original storage-owned object remains alive through acknowledgement.
        self.acknowledge(name, expected, published.as_file())
    }
    fn acknowledge(
        &self,
        name: &str,
        expected: &[u8],
        original: &File,
    ) -> Result<(), PrivateStorageFailure> {
        self.verify_lock().map_err(private_failure)?;
        if !self
            .directory
            .named_file_is(OsStr::new(name), original)
            .map_err(|e| storage_failure(&e))?
        {
            return Err(PrivateStorageFailure::UnsafeStorage);
        }
        let mut file = original.try_clone().map_err(|e| storage_failure(&e))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|e| storage_failure(&e))?;
        let mut actual = Zeroizing::new(Vec::with_capacity(expected.len() + 1));
        (&mut file)
            .take((expected.len() + 1) as u64)
            .read_to_end(&mut actual)
            .map_err(|e| storage_failure(&e))?;
        if !bool::from(actual.as_slice().ct_eq(expected)) {
            return Err(PrivateStorageFailure::UnsafeStorage);
        }
        original.sync_all().map_err(|e| storage_failure(&e))?;
        if !self
            .directory
            .named_file_is(OsStr::new(name), original)
            .map_err(|e| storage_failure(&e))?
        {
            return Err(PrivateStorageFailure::UnsafeStorage);
        }
        self.directory.sync().map_err(|e| storage_failure(&e))?;
        self.verify_lock().map_err(private_failure)
    }
    fn reconcile(&self, name: &str, expected: &[u8]) -> Result<(), PrivateStateError> {
        self.verify_lock()
            .map_err(|_| PrivateStateError::Uncertain)?;
        // Restart has no original Published object; this acquires only current
        // writable existing-file evidence and never creates the canonical name.
        let file = self.reopen_for_acknowledgement(name)?;
        self.acknowledge(name, expected, &file)
            .map_err(|_| PrivateStateError::Uncertain)
    }
    fn reopen_for_acknowledgement(&self, name: &str) -> Result<File, PrivateStateError> {
        self.directory
            .open_file(OsStr::new(name), OpenMode::ReadWrite)
            .map_err(|_| PrivateStateError::Uncertain)
    }
}
impl Drop for FilePairingState {
    fn drop(&mut self) {
        // Explicit unlock releases our authority even if a fork inherited the
        // open description. Failure still closes the owner's handle on drop.
        let _ = self.lock.unlock();
    }
}
/// What the client's enrollment file holds.
enum ClientRecord {
    Pending(PendingEnrollment),
    Credential(DeviceCredential),
}
/// A decoded client record with the exact bytes it was read from.
struct SavedClient {
    record: ClientRecord,
    bytes: Zeroizing<Vec<u8>>,
}
impl FilePairingState {
    fn load_client(&self) -> Result<Option<SavedClient>, PrivateStateError> {
        self.read_bounded(ENROLLMENT_FILE, CREDENTIAL_MAX_BYTES)?
            .map(|bytes| decode_client(&bytes).map(|record| SavedClient { record, bytes }))
            .transpose()
    }
}
impl ClientPendingStore for FilePairingState {
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        self.verify_lock()?;
        let name = OsStr::new(ENROLLMENT_FILE);
        let mut file = match self.directory.open_file(name, OpenMode::ReadNonblocking) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage_error(error)),
        };
        let mut bytes = Zeroizing::new(Vec::with_capacity(CREDENTIAL_MAX_BYTES + 1));
        (&mut file)
            .take((CREDENTIAL_MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() > CREDENTIAL_MAX_BYTES {
            return Err(PrivateStateError::Corrupt);
        }
        let intent = match decode_client(&bytes)? {
            ClientRecord::Pending(pending) => pending.intent(),
            ClientRecord::Credential(credential) => credential.intent(),
        };
        if intent != expected {
            return Err(PrivateStateError::Conflict);
        }
        // The open handle names the file read: a replacement is refused.
        self.directory
            .remove_file(name, &file)
            .map_err(storage_error)?;
        self.directory
            .sync()
            .map_err(|_| PrivateStateError::Uncertain)?;
        self.verify_lock()
    }
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        Ok(match self.load_client()?.map(|saved| saved.record) {
            Some(ClientRecord::Pending(pending)) => Some(pending),
            Some(ClientRecord::Credential(_)) | None => None,
        })
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        Ok(match self.load_client()?.map(|saved| saved.record) {
            Some(ClientRecord::Credential(credential)) => Some(credential),
            Some(ClientRecord::Pending(_)) | None => None,
        })
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        gateway_pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        let bytes = encode_pending(key, gateway_pin, intent);
        let replace = match self.load_client()? {
            None if expected.is_none() => false,
            None => return Err(PrivateStateError::Conflict),
            Some(SavedClient {
                record: ClientRecord::Credential(_),
                ..
            }) => {
                // An issued credential is never replaced by a new enrollment.
                return Err(PrivateStateError::Conflict);
            }
            Some(SavedClient {
                record: ClientRecord::Pending(old),
                bytes: saved,
            }) => {
                if bool::from(saved.as_slice().ct_eq(bytes.as_slice())) {
                    return self.reconcile(ENROLLMENT_FILE, &bytes);
                }
                if expected != Some(old.intent())
                    || !bool::from(old.key().expose_bytes().ct_eq(key.expose_bytes()))
                    || old.gateway_pin() != gateway_pin
                    || old.intent().with_attempt(intent.attempt()) != intent
                {
                    return Err(PrivateStateError::Conflict);
                }
                true
            }
        };
        self.publish(ENROLLMENT_FILE, &bytes, replace)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| PrivateStateError::Unavailable)?;
        match self.load_client()? {
            None => Err(PrivateStateError::Conflict),
            Some(SavedClient {
                record: ClientRecord::Pending(pending),
                ..
            }) => {
                if pending.intent() != expected {
                    return Err(PrivateStateError::Conflict);
                }
                let bytes = encode_credential(&DeviceCredential::new(
                    pending,
                    credential.clone(),
                    receiver.clone(),
                ));
                // One rename replaces the pending record: no state holds both.
                self.publish(ENROLLMENT_FILE, &bytes, true)
            }
            Some(SavedClient {
                record: ClientRecord::Credential(saved),
                bytes,
            }) => {
                if saved.intent() != expected
                    || saved.credential() != credential
                    || saved.receiver() != receiver
                {
                    return Err(PrivateStateError::Conflict);
                }
                self.reconcile(ENROLLMENT_FILE, &bytes)
            }
        }
    }
}
fn encode_pending(
    key: &PrivateKeyMaterial,
    pin: &[u8; 44],
    intent: PublicIntent,
) -> Zeroizing<Vec<u8>> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(PENDING_BYTES));
    bytes.extend_from_slice(PENDING_MAGIC);
    bytes.extend_from_slice(key.expose_bytes());
    bytes.extend_from_slice(pin);
    bytes.extend_from_slice(intent.invitation().bytes());
    bytes.extend_from_slice(intent.attempt().bytes());
    bytes.extend_from_slice(intent.consent().bytes());
    bytes.extend_from_slice(&intent.generation().to_be_bytes());
    bytes.extend_from_slice(&intent.expiry_ms().to_be_bytes());
    bytes
}
fn encode_credential(credential: &DeviceCredential) -> Zeroizing<Vec<u8>> {
    let mut bytes = encode_pending(
        credential.key(),
        credential.gateway_pin(),
        credential.intent(),
    );
    bytes[..4].copy_from_slice(CREDENTIAL_MAGIC);
    for identifier in [
        credential.credential().as_str(),
        credential.receiver().as_str(),
    ] {
        // The identifier owner bounds each value to MAX_IDENTIFIER_BYTES.
        bytes.extend_from_slice(&(identifier.len() as u16).to_be_bytes());
        bytes.extend_from_slice(identifier.as_bytes());
    }
    bytes
}
fn decode_client(bytes: &[u8]) -> Result<ClientRecord, PrivateStateError> {
    if bytes.len() < PENDING_BYTES {
        return Err(PrivateStateError::Corrupt);
    }
    match &bytes[..4] {
        magic if magic == PENDING_MAGIC => decode_pending(bytes).map(ClientRecord::Pending),
        magic if magic == CREDENTIAL_MAGIC => {
            let pending = decode_pending_fields(&bytes[..PENDING_BYTES])?;
            let mut rest = &bytes[PENDING_BYTES..];
            let mut identifier = || -> Result<String, PrivateStateError> {
                let (length, tail) = rest
                    .split_first_chunk::<2>()
                    .ok_or(PrivateStateError::Corrupt)?;
                let length = usize::from(u16::from_be_bytes(*length));
                if tail.len() < length {
                    return Err(PrivateStateError::Corrupt);
                }
                let (value, tail) = tail.split_at(length);
                rest = tail;
                String::from_utf8(value.to_vec()).map_err(|_| PrivateStateError::Corrupt)
            };
            let credential =
                CredentialId::new(identifier()?).map_err(|_| PrivateStateError::Corrupt)?;
            let receiver =
                ResourceId::new(identifier()?).map_err(|_| PrivateStateError::Corrupt)?;
            if !rest.is_empty() {
                return Err(PrivateStateError::Corrupt);
            }
            Ok(ClientRecord::Credential(DeviceCredential::new(
                pending, credential, receiver,
            )))
        }
        _ => Err(PrivateStateError::Corrupt),
    }
}
fn decode_pending(bytes: &[u8]) -> Result<PendingEnrollment, PrivateStateError> {
    if bytes.len() != PENDING_BYTES || &bytes[..4] != PENDING_MAGIC {
        return Err(PrivateStateError::Corrupt);
    }
    decode_pending_fields(bytes)
}
/// The key, pin and correlation shared by both record kinds, after the magic.
fn decode_pending_fields(bytes: &[u8]) -> Result<PendingEnrollment, PrivateStateError> {
    let mut seed = Zeroizing::new([0; 32]);
    seed.copy_from_slice(&bytes[4..36]);
    let mut pin = [0; 44];
    pin.copy_from_slice(&bytes[36..80]);
    let intent = PublicIntent::new(
        InvitationId::new(
            bytes[80..96]
                .try_into()
                .map_err(|_| PrivateStateError::Corrupt)?,
        ),
        AttemptId::new(
            bytes[96..112]
                .try_into()
                .map_err(|_| PrivateStateError::Corrupt)?,
        ),
        ConsentIntentId::new(
            bytes[112..128]
                .try_into()
                .map_err(|_| PrivateStateError::Corrupt)?,
        ),
        u64::from_be_bytes(
            bytes[128..136]
                .try_into()
                .map_err(|_| PrivateStateError::Corrupt)?,
        ),
        u64::from_be_bytes(
            bytes[136..144]
                .try_into()
                .map_err(|_| PrivateStateError::Corrupt)?,
        ),
    )
    .map_err(|_| PrivateStateError::Corrupt)?;
    Ok(PendingEnrollment::new(
        PrivateKeyMaterial::new(seed),
        pin,
        intent,
    ))
}
fn storage_error(error: io::Error) -> PrivateStateError {
    if is_unsafe_file(&error) {
        PrivateStateError::Corrupt
    } else {
        PrivateStateError::Unavailable
    }
}

#[cfg(test)]
mod tests;

mod gateway_audit;

fn storage_failure(error: &io::Error) -> PrivateStorageFailure {
    if is_unsafe_file(error) {
        PrivateStorageFailure::UnsafeStorage
    } else {
        PrivateStorageFailure::Unavailable
    }
}
fn private_failure(error: PrivateStateError) -> PrivateStorageFailure {
    match error {
        PrivateStateError::Corrupt | PrivateStateError::Conflict => {
            PrivateStorageFailure::UnsafeStorage
        }
        _ => PrivateStorageFailure::Unavailable,
    }
}
fn publication_step(step: PrivatePublicationStage) -> PrivatePublicationStep {
    match step {
        PrivatePublicationStage::ValidateDestination => PrivatePublicationStep::ValidateDestination,
        PrivatePublicationStage::VerifyOriginBinding => PrivatePublicationStep::VerifyOriginBinding,
        PrivatePublicationStage::ValidateReservation => PrivatePublicationStep::ValidateReservation,
        PrivatePublicationStage::FlushBeforeRename => PrivatePublicationStep::FlushBeforeRename,
        PrivatePublicationStage::Rename => PrivatePublicationStep::Rename,
        PrivatePublicationStage::FlushAfterRename => PrivatePublicationStep::FlushAfterRename,
        PrivatePublicationStage::ValidatePublishedDestination => {
            PrivatePublicationStep::ValidatePublishedDestination
        }
        PrivatePublicationStage::VerifyPublishedBinding => {
            PrivatePublicationStep::VerifyPublishedBinding
        }
        PrivatePublicationStage::SyncDirectory => PrivatePublicationStep::SyncDirectory,
    }
}
