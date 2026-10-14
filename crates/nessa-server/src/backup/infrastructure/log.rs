//! The deletion inventory on disk. The mac key sits beside it and is not
//! part of an export bundle.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nessa_local_storage::sync_directory;

use async_trait::async_trait;

use crate::backup::application::{Authenticator, DeletionLogStore, MacKey};
use crate::backup::domain::{
    CredentialKind, DeletionEntry, DeletionFence, DeletionLog, Fingerprint, GrantRole,
};

/// `directory/deletion-inventory.txt` plus `directory/mac.key`.
pub struct FileDeletionLog {
    directory: PathBuf,
    key: MacKey,
    gate: Mutex<()>,
}

impl FileDeletionLog {
    /// Open or create `directory`. The mac key is created when absent.
    pub fn open(directory: impl Into<PathBuf>) -> std::io::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        let key_path = directory.join("mac.key");
        let key = if key_path.is_file() {
            let bytes = fs::read(&key_path)?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "mac key"))?;
            MacKey(bytes)
        } else {
            create_key(&directory, &key_path)?
        };
        Ok(Self {
            directory,
            key,
            gate: Mutex::new(()),
        })
    }

    pub fn key(&self) -> MacKey {
        self.key
    }

    fn path(&self) -> PathBuf {
        self.directory.join("deletion-inventory.txt")
    }
}

#[async_trait]
impl DeletionLogStore for FileDeletionLog {
    async fn load(&self) -> Result<DeletionLog, ()> {
        let path = self.path();
        let key = self.key;
        let gate = self.gate.lock().map_err(|_| ())?;
        let log = read_log(&path, key)?;
        drop(gate);
        Ok(log)
    }

    async fn store(&self, next: &DeletionLog) -> Result<(), ()> {
        let path = self.path();
        let key = self.key;
        let text = sealed(next, key);
        let gate = self.gate.lock().map_err(|_| ())?;
        let stored = persist(&self.directory, &path, &text);
        drop(gate);
        stored.map_err(|_| ())
    }
}

/// Write a new mac key and sync the file and its directory before the key
/// is returned. A crash after `open` returns cannot leave a sealed inventory
/// whose key is missing.
fn create_key(directory: &Path, key_path: &Path) -> std::io::Result<MacKey> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(key_path)?;
    let written = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(key_path);
        return Err(error);
    }
    drop(file);
    sync_directory(directory)?;
    Ok(MacKey(bytes))
}

/// Write `text` to a temporary file, sync that file, rename it over `path`,
/// then sync `directory`. Success means the rename is durable. A failure
/// before the rename removes the temporary file and leaves `path` as it was.
fn persist(directory: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("txt.partial");
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    sync_directory(directory)
}

fn sealed(log: &DeletionLog, key: MacKey) -> String {
    let canonical = log.canonical();
    let mac = key.mac(&canonical).expect("a 32-byte key macs");
    let mut text = canonical;
    text.push_str(&format!("mac {}\n", mac.hex()));
    text
}

fn read_log(path: &Path, key: MacKey) -> Result<DeletionLog, ()> {
    if !path.exists() {
        return Ok(DeletionLog::default());
    }
    let text = fs::read_to_string(path).map_err(|_| ())?;
    let (canonical, mac_hex) = split_mac(&text)?;
    let expected = key.mac(canonical).map_err(|_| ())?;
    if expected.hex() != mac_hex {
        return Err(());
    }
    parse_log(canonical)
}

fn split_mac(text: &str) -> Result<(&str, &str), ()> {
    let mac_at = text.rfind("\nmac ").ok_or(())?;
    let canonical = &text[..mac_at + 1];
    let mac_line = text[mac_at + 1..].trim_end_matches('\n');
    let hex = mac_line.strip_prefix("mac ").ok_or(())?;
    Ok((canonical, hex))
}

fn parse_log(canonical: &str) -> Result<DeletionLog, ()> {
    let mut lines = canonical.lines();
    if lines.next() != Some("nessa-deletion-inventory/1") {
        return Err(());
    }
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let sequence = (index as u64) + 1;
        let fence = parse_entry(line, sequence)?;
        entries.push(DeletionEntry::from_persisted(sequence, fence).map_err(|_| ())?);
    }
    DeletionLog::new(entries).map_err(|_| ())
}

fn parse_entry(line: &str, sequence: u64) -> Result<DeletionFence, ()> {
    let mut parts = line.split(' ');
    let got = parts.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    if got != sequence {
        return Err(());
    }
    match parts.next().ok_or(())? {
        "conversation" => {
            let id = decode(parts.next().ok_or(())?)?;
            if parts.next().is_some() {
                return Err(());
            }
            DeletionFence::conversation(id).map_err(|_| ())
        }
        "grant" => {
            let conversation_id = decode(parts.next().ok_or(())?)?;
            let receiver_id = decode(parts.next().ok_or(())?)?;
            let credential_id = decode(parts.next().ok_or(())?)?;
            let role = GrantRole::parse(parts.next().ok_or(())?).ok_or(())?;
            let grant_sequence = parts.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
            if parts.next().is_some() {
                return Err(());
            }
            DeletionFence::share_grant(
                conversation_id,
                receiver_id,
                credential_id,
                role,
                grant_sequence,
            )
            .map_err(|_| ())
        }
        "credential" => {
            let kind = CredentialKind::parse(parts.next().ok_or(())?).ok_or(())?;
            let credential_id = decode(parts.next().ok_or(())?)?;
            let hex = parts.next().ok_or(())?;
            if parts.next().is_some() {
                return Err(());
            }
            DeletionFence::credential(kind, credential_id, fingerprint_hex(hex)?).map_err(|_| ())
        }
        _ => Err(()),
    }
}

fn fingerprint_hex(hex: &str) -> Result<Fingerprint, ()> {
    if hex.len() != 64 {
        return Err(());
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(Fingerprint::from_digest(bytes))
}

fn decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(());
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).map_err(|_| ())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}
