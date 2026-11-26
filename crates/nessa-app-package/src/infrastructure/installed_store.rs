use std::{collections::BTreeMap, path::Path};

use nessa_local_database::{
    rusqlite::{self, params, types::FromSqlError, Connection, OptionalExtension},
    OpenError, Schema,
};
use nessa_types::app_identity::ExtensionAppId;

use crate::{
    parse_manifest, PackageError, VerifiedPackage, MAX_FILES, MAX_MANIFEST_BYTES, MAX_PATH_BYTES,
};

const SCHEMA: &str = include_str!("installed_store.sql");

/// A private database operation or revalidation failed.
#[derive(Debug)]
pub enum StoreError {
    /// The database could not be opened in its expected schema.
    Open(OpenError),
    /// The database operation failed before it could report success.
    Database(rusqlite::Error),
    /// The app identity already has an installed record.
    AlreadyInstalled,
    /// The stored manifest and asset bytes failed package validation.
    InvalidPackage(PackageError),
    /// The stored manifest describes a different app than the lookup key.
    AppIdentityMismatch,
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// Private static package storage for a disposable local database.
///
/// The caller supplies a database path inside its private
/// fixture directory. No lifecycle route or authorization is attached here.
pub struct InstalledAppStore {
    connection: Connection,
}

impl InstalledAppStore {
    /// Open a disposable store in a private directory.
    ///
    /// # Errors
    ///
    /// Returns a typed database open failure for an unusable path or schema.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let schema = Schema::new(SCHEMA).map_err(StoreError::Open)?;
        let connection = nessa_local_database::open(path, &schema).map_err(StoreError::Open)?;
        Ok(Self { connection })
    }

    /// Atomically retain a verified package under its canonical app identity.
    ///
    /// # Errors
    ///
    /// Refuses an existing app identity or returns the database failure.
    pub fn install(&mut self, package: &VerifiedPackage) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        let app_id = package.manifest().app_id().as_str();
        let inserted = transaction.execute(
            "INSERT INTO installed_apps (app_id, manifest) VALUES (?1, ?2)
             ON CONFLICT(app_id) DO NOTHING",
            params![app_id, package.manifest().json()],
        )?;
        if inserted == 0 {
            return Err(StoreError::AlreadyInstalled);
        }
        {
            let mut insert = transaction.prepare(
                "INSERT INTO installed_app_assets (app_id, path, bytes) VALUES (?1, ?2, ?3)",
            )?;
            for (path, bytes) in package.assets() {
                insert.execute(params![app_id, path, bytes])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Load a package from one snapshot and rerun the headless verifier.
    ///
    /// Returns an empty result when the requested app has no installed record.
    ///
    /// # Errors
    ///
    /// Returns a typed database failure or a stored-package validation failure.
    pub fn load(&mut self, app: &ExtensionAppId) -> Result<Option<VerifiedPackage>, StoreError> {
        let transaction = self.connection.transaction()?;
        let app_id = app.as_str();
        let manifest = transaction
            .query_row(
                "SELECT manifest FROM installed_apps WHERE app_id = ?1",
                [app_id],
                |row| {
                    let blob = row
                        .get_ref(0)?
                        .as_blob()
                        .map_err(|error| conversion(0, rusqlite::types::Type::Blob, error))?;
                    Ok((blob.len() <= MAX_MANIFEST_BYTES).then(|| blob.to_vec()))
                },
            )
            .optional()?;
        let manifest = match manifest {
            None => return Ok(None),
            Some(None) => return Err(StoreError::InvalidPackage(PackageError::ManifestTooLarge)),
            Some(Some(manifest)) => manifest,
        };
        let manifest = parse_manifest(manifest).map_err(StoreError::InvalidPackage)?;
        if manifest.app_id() != app {
            return Err(StoreError::AppIdentityMismatch);
        }
        let assets = {
            let mut query = transaction.prepare(
                "SELECT path, bytes FROM installed_app_assets WHERE app_id = ?1 ORDER BY path",
            )?;
            let mut rows = query.query([app_id])?;
            let mut assets = BTreeMap::new();
            while let Some(row) = rows.next()? {
                if assets.len() == MAX_FILES {
                    return Err(StoreError::InvalidPackage(PackageError::TooManyFiles));
                }
                let path = row
                    .get_ref(0)?
                    .as_str()
                    .map_err(|error| conversion(0, rusqlite::types::Type::Text, error))?;
                let bytes = row
                    .get_ref(1)?
                    .as_blob()
                    .map_err(|error| conversion(1, rusqlite::types::Type::Blob, error))?;
                if path.len() > MAX_PATH_BYTES {
                    return Err(StoreError::InvalidPackage(PackageError::InvalidPath));
                }
                manifest
                    .check_held_asset_size(path, bytes.len())
                    .map_err(StoreError::InvalidPackage)?;
                assets.insert(path.to_owned(), bytes.to_vec());
            }
            assets
        };
        transaction.commit()?;
        let package = manifest
            .verify_held(assets)
            .map_err(StoreError::InvalidPackage)?;
        Ok(Some(package))
    }
}

fn conversion(index: usize, kind: rusqlite::types::Type, error: FromSqlError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, kind, Box::new(error))
}
