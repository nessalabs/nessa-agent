//! SQLCipher's authenticated page encryption. Keys are raw random bytes, held
//! separately by the caller; SQLite's temporary work stays in memory.
use super::{open_mode, OpenError, Schema};
use nessa_local_storage::{open, sync_directory, OpenMode, PrivateTempFile};
use rusqlite::Connection;
use std::{io::Read, path::Path};
use zeroize::Zeroizing;

/// A random 256-bit key; Debug and serialization deliberately absent.
pub struct DatabaseKey(Zeroizing<[u8; 32]>);
impl DatabaseKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    fn raw(&self) -> Zeroizing<String> {
        use std::fmt::Write;
        let mut value = Zeroizing::new(String::with_capacity(67));
        value.push_str("x'");
        for byte in self.0.iter() {
            write!(value, "{byte:02x}").expect("String write");
        }
        value.push('\'');
        value
    }
}
pub(super) fn apply_key(db: &Connection, raw: &str) -> Result<(), OpenError> {
    // The raw key is constructed from bytes only; never enable SQL tracing here.
    db.pragma_update(None, "key", raw)?;
    db.pragma_update(None, "temp_store", "MEMORY")?;
    let version: String = db.pragma_query_value(None, "cipher_version", |row| row.get(0))?;
    if version.is_empty() {
        return Err(OpenError::Database(rusqlite::Error::InvalidQuery));
    }
    Ok(())
}

/// Open an encrypted database, converting an existing plaintext database once.
/// The caller must supply the existing key; a decryption failure preserves the
/// source. The process-shared lock covers export and replacement, so two new
/// callers do not migrate different snapshots (`plaintext_upgrade_preserves_rows`).
pub fn open_encrypted(
    path: &Path,
    schema: &Schema,
    key: &DatabaseKey,
) -> Result<Connection, OpenError> {
    let parent = path
        .parent()
        .ok_or_else(|| OpenError::File(std::io::ErrorKind::NotFound.into()))?;
    let lock = open(
        &path.with_extension("encryption-lock"),
        OpenMode::OpenOrCreate,
    )
    .map_err(OpenError::File)?;
    lock.lock().map_err(OpenError::File)?;
    let mut header = [0; 16];
    let plaintext = match open(path, OpenMode::Read) {
        Ok(mut file) => file.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0",
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(OpenError::File(e)),
    };
    let raw = key.raw();
    if plaintext {
        let source = super::open_existing(path, schema)?;
        source.pragma_update(None, "temp_store", "MEMORY")?;
        let temporary = PrivateTempFile::new_in(parent).map_err(OpenError::File)?;
        let destination = temporary
            .path()
            .to_str()
            .ok_or_else(|| OpenError::File(std::io::ErrorKind::InvalidInput.into()))?;
        source.execute(
            "ATTACH DATABASE ? AS encrypted KEY ?",
            rusqlite::params![destination, raw.as_str()],
        )?;
        source.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
        source.pragma_update(Some("encrypted"), "user_version", schema.version())?;
        source.execute_batch("DETACH DATABASE encrypted")?;
        drop(source);
        let verified = open_mode(
            temporary.path(),
            schema,
            OpenMode::ReadWrite,
            Some(raw.as_str()),
        )?;
        drop(verified);
        temporary.as_file().sync_all().map_err(OpenError::File)?;
        temporary.persist(path).map_err(OpenError::File)?;
        sync_directory(parent).map_err(OpenError::File)?;
    }
    open_mode(path, schema, OpenMode::OpenOrCreate, Some(raw.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    const SQL: &str = "CREATE TABLE secrets(value TEXT);\nPRAGMA user_version = 1;";
    #[test]
    fn plaintext_upgrade_preserves_rows() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("inbox");
        let schema = Schema::new(SQL).unwrap();
        let db = super::super::open(&path, &schema).unwrap();
        db.execute(
            "INSERT INTO secrets VALUES(?)",
            ["private-repository-and-comment"],
        )
        .unwrap();
        drop(db);
        let key = DatabaseKey::new([37; 32]);
        let db = open_encrypted(&path, &schema, &key).unwrap();
        assert_eq!(
            db.query_row("SELECT value FROM secrets", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "private-repository-and-comment"
        );
        drop(db);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(7).any(|s| s == b"private"));
        assert!(super::super::open_existing(&path, &schema).is_err());
        assert!(open_encrypted(&path, &schema, &DatabaseKey::new([38; 32])).is_err());
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        assert!(open_encrypted(&path, &schema, &key).is_ok());
    }
}
