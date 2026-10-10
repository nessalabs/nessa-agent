//! What this gateway keeps about each gateway it dialed: one private file per
//! peer, beneath one private directory, holding the peer's pinned key, the
//! enrollment it claimed, the credential once issued, and the address that
//! last answered. The key it enrolled with is the gateway's own, read from the
//! gateway key store each time it is needed; a record names that key by its
//! public half and never holds a copy of its private half.
//!
//! ```text
//! PeerRecords --> peer-gateways/<peer key hex>.json   (one per peer)
//!             --> GatewayKeyStore (the gateway's own key, by reference)
//! PeerSlot    --> ClientPendingStore for one enrollment, over PeerRecords
//! ```
//! Arrows are reads and writes. A record is the peer's own: one that cannot be
//! read fails that peer's operations and nothing else
//! (docs/adr/todo/202-versioned-local-datasets.md, record scope).
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_auth::{
    adapters::pairing::NativeIdentity,
    application::{
        pairing::{
            ClientPendingStore, DeviceCredential, GatewayKeyStore, PendingEnrollment,
            PrivateKeyMaterial, PrivateStateError,
        },
        ports::Clock,
    },
    domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntentId, DeviceKey, InvitationId, PublicIntent,
        },
        AudienceId, CredentialId, ResourceId,
    },
};
use nessa_local_storage::{is_unsafe_file, OpenMode, PrivateDirectory, PrivateFileType};
use nessa_protocol::product::generated::MAX_PEERS;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};
use subtle::ConstantTimeEq;

/// The shape of a peer record. Bumped only when that shape changes
/// (docs/adr/todo/202-versioned-local-datasets.md, rule 1).
const SCHEMA_VERSION: u32 = 1;
/// Largest record read: identifiers are bounded by their owners, so a real
/// record is well under this.
const RECORD_BYTES: usize = 4 * 1024;
const SUFFIX: &str = ".json";
/// An Ed25519 SPKI: a fixed 12-byte prefix, then the 32-byte key.
const SPKI_PREFIX_BYTES: usize = 12;

/// Where a peer's enrollment stands, as this gateway last heard it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerPhase {
    /// Claimed and waiting on the peer's owner, or not yet confirmed.
    Pending,
    /// The peer issued this gateway a credential.
    Active {
        /// The issued credential's identifier, not a bearer secret.
        credential: CredentialId,
        /// The receiver the peer paired with that credential.
        receiver: ResourceId,
    },
    /// The peer ended this enrollment: its owner revoked the credential, or
    /// denied, cancelled or let expire the claim, as an authenticated status
    /// said. Nothing more is read from it; only forgetting it helps.
    Revoked,
}

/// One readable peer record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRecord {
    key: DeviceKey,
    pin: [u8; 44],
    address: SocketAddr,
    intent: PublicIntent,
    phase: PeerPhase,
}
impl PeerRecord {
    /// The peer's key: what identifies it, whatever its address.
    pub fn key(&self) -> &DeviceKey {
        &self.key
    }
    /// The peer's pinned public key, as TLS presents it.
    pub fn pin(&self) -> &[u8; 44] {
        &self.pin
    }
    /// The address that last answered as this peer.
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    /// Where the enrollment stands.
    pub fn phase(&self) -> &PeerPhase {
        &self.phase
    }
}

/// A peer as a listing finds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerEntry {
    /// Its record reads.
    Readable(PeerRecord),
    /// Its record is there but cannot be read: another shape, damaged, or made
    /// with a key that is no longer this gateway's. Only forgetting it helps.
    Unreadable(DeviceKey),
}
impl PeerEntry {
    /// The peer's key, readable or not.
    pub fn key(&self) -> &DeviceKey {
        match self {
            Self::Readable(record) => record.key(),
            Self::Unreadable(key) => key,
        }
    }
}

/// Why a slot refused to save a new enrollment, beyond the store's own error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotRefusal {
    /// This gateway already holds a record for that peer.
    Exists,
    /// The address answered with this gateway's own key.
    OwnGateway,
    /// This gateway already keeps as many peers as the list holds (`MAX_PEERS`).
    Capacity,
}

/// What one enrollment's saves did to its peer's record: the record it
/// created, one it already had made pending and replaced on a retry, or a
/// save storage did not confirm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotSave {
    /// No record was there; this enrollment made it.
    Created,
    /// This peer's pending record was there and the save replaced it.
    Replaced,
    /// Storage did not confirm the save: the record may or may not be there.
    Uncertain,
}

/// The private directory of peer records, and the gateway key they refer to.
/// One per gateway; every operation holds its lock, so a record is never read
/// half-replaced by this process.
pub struct PeerRecords {
    directory: PrivateDirectory,
    /// The same directory by path, where each peer's retained cache opens.
    path: PathBuf,
    keys: Arc<dyn GatewayKeyStore>,
    gateway: AudienceId,
    clock: Arc<dyn Clock>,
    operation: Mutex<()>,
}
impl PeerRecords {
    /// Open `directory`, already private, beneath the trusted absolute `root`.
    /// The gateway key is read from `keys` under `gateway` when needed.
    pub fn open(
        root: &Path,
        directory: &Path,
        keys: Arc<dyn GatewayKeyStore>,
        gateway: AudienceId,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, PrivateStateError> {
        Ok(Self {
            directory: PrivateDirectory::open_beneath(root, directory).map_err(storage_error)?,
            path: root.join(directory),
            keys,
            gateway,
            clock,
            operation: Mutex::new(()),
        })
    }
    /// A slot for a new enrollment dialed at `address`.
    pub fn enrolling(self: &Arc<Self>, address: SocketAddr) -> PeerSlot {
        PeerSlot {
            records: self.clone(),
            address: Some(address),
            peer: Mutex::new(None),
            refusal: Mutex::new(None),
            existing: Mutex::new(None),
            saved: Mutex::new(None),
        }
    }
    /// A slot for the peer already recorded under `key`, for reading its
    /// enrollment status.
    pub fn slot(self: &Arc<Self>, key: DeviceKey) -> PeerSlot {
        PeerSlot {
            records: self.clone(),
            address: None,
            peer: Mutex::new(Some(key)),
            refusal: Mutex::new(None),
            existing: Mutex::new(None),
            saved: Mutex::new(None),
        }
    }
    /// Every peer, in key order, at most `MAX_PEERS`: saving refuses one
    /// more, so only files put there by hand can pass it. A record that cannot
    /// be read is listed as such rather than failing the list; storage that
    /// fails does fail it.
    pub fn list(&self) -> Result<Vec<PeerEntry>, PrivateStateError> {
        let _guard = self.lock();
        let mut keys = self.keys_present()?;
        keys.sort_by(|left, right| left.bytes().cmp(right.bytes()));
        keys.truncate(MAX_PEERS);
        let own = self.own_spki()?;
        let mut entries = Vec::with_capacity(keys.len());
        for key in keys {
            // Removed since the directory was read: not a peer any more.
            if let Some(entry) = self.entry(&key, &own)? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
    /// One peer, if it has a record.
    pub fn get(&self, key: &DeviceKey) -> Result<Option<PeerEntry>, PrivateStateError> {
        let _guard = self.lock();
        self.entry(key, &self.own_spki()?)
    }
    /// Remove the record for `key`, readable or not, and return what it was.
    /// Local only: the peer still holds the credential it issued until its
    /// owner revokes it there.
    pub fn forget(&self, key: &DeviceKey) -> Result<Option<PeerEntry>, PrivateStateError> {
        let _guard = self.lock();
        let Some(entry) = self.entry(key, &self.own_spki()?)? else {
            return Ok(None);
        };
        // The cache first: a record left by a failure here can be forgotten
        // again, while a cache left without its record would be found by
        // nothing.
        self.remove_cache_locked(key)?;
        let name = file_name(key);
        let file = match self.directory.open_file(&name, OpenMode::ReadNonblocking) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        // The open handle names the file read: a replacement is refused.
        self.directory
            .remove_file(&name, &file)
            .map_err(storage_error)?;
        self.directory
            .sync()
            .map_err(|_| PrivateStateError::Uncertain)?;
        Ok(Some(entry))
    }

    /// Where the retained cache of what `key` granted this gateway lives:
    /// beside its record, as `<peer key hex>.sqlite3`.
    pub fn cache_path(&self, key: &DeviceKey) -> PathBuf {
        self.path.join(cache_name(key))
    }
    /// Remove `key`'s retained cache, and a rollback journal SQLite left
    /// beside it, if there. The caller holds the cache closed.
    pub fn remove_cache(&self, key: &DeviceKey) -> Result<(), PrivateStateError> {
        let _guard = self.lock();
        self.remove_cache_locked(key)
    }
    /// Whether `key` has a retained cache.
    pub fn has_cache(&self, key: &DeviceKey) -> Result<bool, PrivateStateError> {
        let _guard = self.lock();
        match self
            .directory
            .open_file(&cache_name(key), OpenMode::ReadNonblocking)
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(storage_error(error)),
        }
    }
    fn remove_cache_locked(&self, key: &DeviceKey) -> Result<(), PrivateStateError> {
        let cache = cache_name(key);
        let mut journal = cache.clone();
        journal.push("-journal");
        let mut removed = false;
        for name in [journal, cache] {
            let file = match self.directory.open_file(&name, OpenMode::ReadNonblocking) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(storage_error(error)),
            };
            self.directory
                .remove_file(&name, &file)
                .map_err(storage_error)?;
            removed = true;
        }
        if removed {
            self.directory
                .sync()
                .map_err(|_| PrivateStateError::Uncertain)?;
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.operation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
    fn entry(
        &self,
        key: &DeviceKey,
        own: &[u8; 44],
    ) -> Result<Option<PeerEntry>, PrivateStateError> {
        match self.read(key, own) {
            Ok(Some(record)) => Ok(Some(PeerEntry::Readable(record))),
            Ok(None) => Ok(None),
            Err(PrivateStateError::Corrupt) => Ok(Some(PeerEntry::Unreadable(*key))),
            Err(error) => Err(error),
        }
    }
    /// The gateway's own key, restored from its store. Absent means native
    /// pairing never published one, which composition rules out first.
    fn own_key(&self) -> Result<PrivateKeyMaterial, PrivateStateError> {
        self.keys
            .restore_gateway_key(&self.gateway, self.clock.as_ref())?
            .ok_or(PrivateStateError::Unavailable)
    }
    /// The public half of the gateway's own key: how a record names it.
    fn own_spki(&self) -> Result<[u8; 44], PrivateStateError> {
        Ok(NativeIdentity::restore(self.own_key()?)
            .map_err(|_| PrivateStateError::Corrupt)?
            .public_spki())
    }
    fn keys_present(&self) -> Result<Vec<DeviceKey>, PrivateStateError> {
        let mut keys = Vec::new();
        for entry in self.directory.entries().map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            if entry.file_type() != PrivateFileType::RegularFile {
                continue;
            }
            if let Some(key) = key_of(entry.name()) {
                keys.push(key);
            }
        }
        Ok(keys)
    }
    /// The record for `key`: `None` when absent, `Corrupt` when it is there
    /// and cannot be read as a record for that key made with `own`.
    fn read(
        &self,
        key: &DeviceKey,
        own: &[u8; 44],
    ) -> Result<Option<PeerRecord>, PrivateStateError> {
        let Some(bytes) = self.read_bytes(key)? else {
            return Ok(None);
        };
        decode(&bytes, key, own).map(Some)
    }
    fn read_bytes(&self, key: &DeviceKey) -> Result<Option<Vec<u8>>, PrivateStateError> {
        let name = file_name(key);
        let mut file = match self.directory.open_file(&name, OpenMode::ReadNonblocking) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        let mut bytes = Vec::with_capacity(RECORD_BYTES + 1);
        (&mut file)
            .take((RECORD_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() > RECORD_BYTES
            || !self
                .directory
                .named_file_is(&name, &file)
                .map_err(storage_error)?
        {
            return Err(PrivateStateError::Corrupt);
        }
        Ok(Some(bytes))
    }
    fn write(
        &self,
        record: &PeerRecord,
        own: &[u8; 44],
        replace: bool,
    ) -> Result<(), PrivateStateError> {
        let bytes = encode(record, own)?;
        let mut temporary = self.directory.reserve_temp().map_err(storage_error)?;
        temporary
            .as_file_mut()
            .write_all(&bytes)
            .map_err(storage_error)?;
        let name = file_name(&record.key);
        let published = if replace {
            temporary.replace(&name)
        } else {
            temporary.publish_new(&name)
        };
        match published {
            Ok(_) => Ok(()),
            // Renamed, then not acknowledged: the record may be there.
            Err(failure) if failure.published().is_some() => Err(PrivateStateError::Uncertain),
            Err(_) => Err(PrivateStateError::Unavailable),
        }
    }
}

/// One enrollment's view of the peer records, as the enrollment client's
/// store. It enrolls with the gateway's own key and saves the record under the
/// peer key the gateway authenticated, which it learns only then.
pub struct PeerSlot {
    records: Arc<PeerRecords>,
    /// Where a new enrollment dialed; `None` for a slot over an existing peer.
    address: Option<SocketAddr>,
    peer: Mutex<Option<DeviceKey>>,
    refusal: Mutex<Option<SlotRefusal>>,
    /// The record an `Exists` refusal found, as it found it.
    existing: Mutex<Option<PeerEntry>>,
    /// The peer and what the first save that touched its record did.
    saved: Mutex<Option<(DeviceKey, SlotSave)>>,
}
impl PeerSlot {
    /// The peer this slot saved or was opened for, once known.
    pub fn peer(&self) -> Option<DeviceKey> {
        *self.peer.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Why the last save was refused, when the refusal was about the peer
    /// rather than storage.
    pub fn refusal(&self) -> Option<SlotRefusal> {
        *self.refusal.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// The record that refused this slot's save `Exists`, as it was read
    /// under the records' lock; that save left it untouched.
    pub fn existing(&self) -> Option<PeerEntry> {
        self.existing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// The peer whose record this slot's saves touched, and how the first
    /// of them did; `None` when no save reached storage.
    pub fn saved(&self) -> Option<(DeviceKey, SlotSave)> {
        *self.saved.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn note_save(&self, peer: DeviceKey, save: SlotSave) {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert((peer, save));
    }
    /// Refuse `Exists`, keeping the record that refused it.
    fn refuse_existing(&self, existing: PeerEntry) -> PrivateStateError {
        *self.existing.lock().unwrap_or_else(PoisonError::into_inner) = Some(existing);
        self.refuse(SlotRefusal::Exists)
    }
    fn refuse(&self, refusal: SlotRefusal) -> PrivateStateError {
        *self.refusal.lock().unwrap_or_else(PoisonError::into_inner) = Some(refusal);
        PrivateStateError::Conflict
    }
    /// The record this slot names, with the gateway key restored for the
    /// client to sign with.
    fn current(&self) -> Result<Option<(PeerRecord, PrivateKeyMaterial)>, PrivateStateError> {
        let Some(peer) = self.peer() else {
            return Ok(None);
        };
        let _guard = self.records.lock();
        let own = self.records.own_spki()?;
        let Some(record) = self.records.read(&peer, &own)? else {
            return Ok(None);
        };
        Ok(Some((record, self.records.own_key()?)))
    }
}
impl ClientPendingStore for PeerSlot {
    fn enrollment_key(&self) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        let _guard = self.records.lock();
        self.records.own_key().map(Some)
    }
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        Ok(match self.current()? {
            Some((record, key)) if record.phase == PeerPhase::Pending => {
                Some(PendingEnrollment::new(key, record.pin, record.intent))
            }
            _ => None,
        })
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        Ok(match self.current()? {
            Some((
                PeerRecord {
                    pin,
                    intent,
                    phase:
                        PeerPhase::Active {
                            credential,
                            receiver,
                        },
                    ..
                },
                key,
            )) => Some(DeviceCredential::new(
                PendingEnrollment::new(key, pin, intent),
                credential,
                receiver,
            )),
            _ => None,
        })
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        gateway_pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        let _guard = self.records.lock();
        if intent.class() != ConsentClass::PeerRead {
            return Err(PrivateStateError::Conflict);
        }
        // Only the gateway's own key is ever enrolled, so only a reference to
        // it is ever written.
        if !bool::from(
            self.records
                .own_key()?
                .expose_bytes()
                .ct_eq(key.expose_bytes()),
        ) {
            return Err(PrivateStateError::Conflict);
        }
        let own = self.records.own_spki()?;
        if gateway_pin == &own {
            return Err(self.refuse(SlotRefusal::OwnGateway));
        }
        let peer = peer_key(gateway_pin);
        if self.peer().is_some_and(|known| known != peer) {
            return Err(PrivateStateError::Conflict);
        }
        let existing = match self.records.read(&peer, &own) {
            // A record this build cannot read is still that peer's.
            Err(PrivateStateError::Corrupt) => {
                return Err(self.refuse_existing(PeerEntry::Unreadable(peer)))
            }
            read => read?,
        };
        let replace = match &existing {
            None if expected.is_none() => {
                if self.records.keys_present()?.len() >= MAX_PEERS {
                    return Err(self.refuse(SlotRefusal::Capacity));
                }
                false
            }
            None => return Err(PrivateStateError::Conflict),
            Some(old) => {
                let retry = old.phase == PeerPhase::Pending
                    && old.pin == *gateway_pin
                    && (old.intent == intent
                        || (expected == Some(old.intent)
                            && old.intent.with_attempt(intent.attempt()) == intent));
                if !retry {
                    return Err(self.refuse_existing(PeerEntry::Readable(old.clone())));
                }
                true
            }
        };
        let address = match (self.address, &existing) {
            (Some(address), _) => address,
            (None, Some(old)) => old.address,
            (None, None) => return Err(PrivateStateError::Conflict),
        };
        let record = PeerRecord {
            key: peer,
            pin: *gateway_pin,
            address,
            intent,
            phase: PeerPhase::Pending,
        };
        if existing.as_ref() != Some(&record) {
            if let Err(error) = self.records.write(&record, &own, replace) {
                if matches!(error, PrivateStateError::Uncertain) {
                    self.note_save(peer, SlotSave::Uncertain);
                }
                return Err(error);
            }
        }
        self.note_save(
            peer,
            if replace {
                SlotSave::Replaced
            } else {
                SlotSave::Created
            },
        );
        *self.peer.lock().unwrap_or_else(PoisonError::into_inner) = Some(peer);
        Ok(())
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        let peer = self.peer().ok_or(PrivateStateError::Conflict)?;
        let _guard = self.records.lock();
        let own = self.records.own_spki()?;
        let record = self
            .records
            .read(&peer, &own)?
            .ok_or(PrivateStateError::Conflict)?;
        if record.intent != expected {
            return Err(PrivateStateError::Conflict);
        }
        let active = PeerPhase::Active {
            credential: credential.clone(),
            receiver: receiver.clone(),
        };
        match &record.phase {
            PeerPhase::Active { .. } if record.phase == active => Ok(()),
            PeerPhase::Active { .. } | PeerPhase::Revoked => Err(PrivateStateError::Conflict),
            PeerPhase::Pending => self.records.write(
                &PeerRecord {
                    phase: active,
                    ..record
                },
                &own,
                true,
            ),
        }
    }
    /// The peer's authenticated end of this enrollment. A device's record
    /// goes so it can enroll again; a peer's stays, marked revoked, so the
    /// owner sees what happened and forgets it. Its credential goes with it.
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        let Some(peer) = self.peer() else {
            return Ok(());
        };
        let _guard = self.records.lock();
        let own = self.records.own_spki()?;
        let Some(record) = self.records.read(&peer, &own)? else {
            return Ok(());
        };
        if record.intent != expected {
            return Err(PrivateStateError::Conflict);
        }
        if record.phase == PeerPhase::Revoked {
            return Ok(());
        }
        self.records.write(
            &PeerRecord {
                phase: PeerPhase::Revoked,
                ..record
            },
            &own,
            true,
        )
    }
}

/// The record as written: every byte value base64, the enrollment's class
/// implied (a peer record is always `PeerRead`).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    schema_version: u32,
    /// The public half of the key this gateway enrolled with: its own.
    gateway_key: String,
    pin: String,
    address: String,
    invitation: String,
    attempt: String,
    consent: String,
    generation: u64,
    expiry_ms: u64,
    credential: Option<StoredCredential>,
    /// The peer ended the enrollment; never with a credential.
    #[serde(default, skip_serializing_if = "is_false")]
    revoked: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredCredential {
    credential_id: String,
    receiver_id: String,
}

fn encode(record: &PeerRecord, own: &[u8; 44]) -> Result<Vec<u8>, PrivateStateError> {
    let intent = record.intent;
    let stored = Stored {
        schema_version: SCHEMA_VERSION,
        gateway_key: STANDARD.encode(own),
        pin: STANDARD.encode(record.pin),
        address: record.address.to_string(),
        invitation: STANDARD.encode(intent.invitation().bytes()),
        attempt: STANDARD.encode(intent.attempt().bytes()),
        consent: STANDARD.encode(intent.consent().bytes()),
        generation: intent.generation(),
        expiry_ms: intent.expiry_ms(),
        revoked: record.phase == PeerPhase::Revoked,
        credential: match &record.phase {
            PeerPhase::Pending | PeerPhase::Revoked => None,
            PeerPhase::Active {
                credential,
                receiver,
            } => Some(StoredCredential {
                credential_id: credential.as_str().to_owned(),
                receiver_id: receiver.as_str().to_owned(),
            }),
        },
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| PrivateStateError::Corrupt)?;
    if bytes.len() > RECORD_BYTES {
        return Err(PrivateStateError::Corrupt);
    }
    Ok(bytes)
}

/// A record for `key`, made with `own`, at this shape; anything else is
/// `Corrupt`.
fn decode(bytes: &[u8], key: &DeviceKey, own: &[u8; 44]) -> Result<PeerRecord, PrivateStateError> {
    let stored: Stored = serde_json::from_slice(bytes).map_err(|_| PrivateStateError::Corrupt)?;
    if stored.schema_version != SCHEMA_VERSION {
        return Err(PrivateStateError::Corrupt);
    }
    // Made with another key than this gateway's own: unusable here.
    if fixed::<44>(&stored.gateway_key)? != *own {
        return Err(PrivateStateError::Corrupt);
    }
    let pin = fixed::<44>(&stored.pin)?;
    if peer_key(&pin) != *key {
        return Err(PrivateStateError::Corrupt);
    }
    let intent = PublicIntent::new(
        InvitationId::new(fixed(&stored.invitation)?),
        AttemptId::new(fixed(&stored.attempt)?),
        ConsentIntentId::new(fixed(&stored.consent)?),
        stored.generation,
        stored.expiry_ms,
        ConsentClass::PeerRead,
    )
    .map_err(|_| PrivateStateError::Corrupt)?;
    let phase = match (stored.revoked, stored.credential) {
        (true, None) => PeerPhase::Revoked,
        (true, Some(_)) => return Err(PrivateStateError::Corrupt),
        (false, None) => PeerPhase::Pending,
        (false, Some(credential)) => PeerPhase::Active {
            credential: CredentialId::new(credential.credential_id)
                .map_err(|_| PrivateStateError::Corrupt)?,
            receiver: ResourceId::new(credential.receiver_id)
                .map_err(|_| PrivateStateError::Corrupt)?,
        },
    };
    Ok(PeerRecord {
        key: *key,
        pin,
        address: stored
            .address
            .parse()
            .map_err(|_| PrivateStateError::Corrupt)?,
        intent,
        phase,
    })
}

fn fixed<const N: usize>(text: &str) -> Result<[u8; N], PrivateStateError> {
    STANDARD
        .decode(text)
        .ok()
        .and_then(|bytes| <[u8; N]>::try_from(bytes).ok())
        .ok_or(PrivateStateError::Corrupt)
}

/// The peer key a pin carries: the SPKI without its fixed prefix.
fn peer_key(pin: &[u8; 44]) -> DeviceKey {
    let mut key = [0; DeviceKey::LENGTH];
    key.copy_from_slice(&pin[SPKI_PREFIX_BYTES..]);
    DeviceKey::new(key)
}

/// `<64 lowercase hex>.sqlite3`: the peer's retained cache, which the
/// listing passes over.
fn cache_name(key: &DeviceKey) -> OsString {
    let mut name = file_name(key).into_string().unwrap_or_default();
    name.truncate(2 * DeviceKey::LENGTH);
    name.push_str(".sqlite3");
    OsString::from(name)
}

/// `<64 lowercase hex>.json`.
fn file_name(key: &DeviceKey) -> OsString {
    let mut name = String::with_capacity(2 * DeviceKey::LENGTH + SUFFIX.len());
    for byte in key.bytes() {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(SUFFIX);
    OsString::from(name)
}

/// The key a record's file name spells, if it spells one exactly.
fn key_of(name: &OsStr) -> Option<DeviceKey> {
    let hex = name.to_str()?.strip_suffix(SUFFIX)?;
    if hex.len() != 2 * DeviceKey::LENGTH || hex.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    let mut key = [0; DeviceKey::LENGTH];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * index..2 * index + 2)?, 16).ok()?;
    }
    Some(DeviceKey::new(key))
}

fn storage_error(error: io::Error) -> PrivateStateError {
    if is_unsafe_file(&error) {
        PrivateStateError::Corrupt
    } else {
        PrivateStateError::Unavailable
    }
}
