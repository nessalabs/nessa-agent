use std::{collections::BTreeMap, path::Path};

use nessa_local_database::{
    rusqlite::{
        self, params,
        types::{FromSqlError, ValueRef},
        Connection, OptionalExtension, Transaction,
    },
    OpenError, Schema,
};
use nessa_types::app_identity::{ExtensionAppId, MAX_EXTENSION_APP_ID_BYTES};

use crate::{
    parse_manifest, PackageError, PackageManifest, VerifiedPackage, MAX_FILES, MAX_MANIFEST_BYTES,
    MAX_PATH_BYTES,
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
    /// The exact app identity has no installed record to remove.
    NotInstalled,
    /// The store holds an invalid installed app identity.
    InvalidStoredAppId,
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

/// Inventory metadata for one installed app. Asset integrity is checked on load.
#[derive(Debug, Eq, PartialEq)]
pub struct InstalledApp {
    app_id: ExtensionAppId,
    package_version: String,
}

impl InstalledApp {
    /// The exact app identity under which the package is installed.
    pub fn app_id(&self) -> &ExtensionAppId {
        &self.app_id
    }

    /// The package version from the stored manifest.
    pub fn package_version(&self) -> &str {
        &self.package_version
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
        let schema = Schema::new(SCHEMA)
            .map_err(StoreError::Open)?
            .with_validation(validate_shape);
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

    /// Read installed app metadata in app ID order from one database snapshot.
    ///
    /// An empty store returns an empty list. Asset bytes are checked on load.
    ///
    /// # Errors
    ///
    /// Returns a database or metadata validation error if inventory cannot be read.
    pub fn inventory(&self) -> Result<Vec<InstalledApp>, StoreError> {
        // SQLite checks lengths before reading the corresponding column payloads.
        let mut query = self.connection.prepare(
            "SELECT CASE WHEN octet_length(app_id) <= ?1 THEN app_id END,
                        CASE WHEN length(manifest) <= ?2 THEN manifest END
                 FROM installed_apps ORDER BY app_id",
        )?;
        let mut rows = query.query(params![
            MAX_EXTENSION_APP_ID_BYTES as i64,
            MAX_MANIFEST_BYTES as i64
        ])?;
        let mut installed = Vec::new();
        while let Some(row) = rows.next()? {
            let raw_id = row.get_ref(0)?;
            if raw_id == ValueRef::Null {
                return Err(StoreError::InvalidStoredAppId);
            }
            let id = raw_id
                .as_str()
                .map_err(|error| conversion(0, rusqlite::types::Type::Text, error))?;
            // Bound the borrowed column before handing it to the owned ID constructor.
            if id.len() > MAX_EXTENSION_APP_ID_BYTES {
                return Err(StoreError::InvalidStoredAppId);
            }
            // Validate the row key through the canonical identity owner before use.
            let app_id = ExtensionAppId::new(id).map_err(|_| StoreError::InvalidStoredAppId)?;
            let manifest = stored_manifest(row.get_ref(1)?, 1, &app_id)?;
            installed.push(InstalledApp {
                app_id,
                package_version: manifest.package_version().to_owned(),
            });
        }
        Ok(installed)
    }

    /// Remove the exact installed app and its assets in one transaction.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-app outcome or a database failure.
    pub fn remove(&mut self, app: &ExtensionAppId) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        let app_id = app.as_str();
        transaction.execute(
            "DELETE FROM installed_app_assets WHERE app_id = ?1",
            [app_id],
        )?;
        if transaction.execute("DELETE FROM installed_apps WHERE app_id = ?1", [app_id])? == 0 {
            return Err(StoreError::NotInstalled);
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
                "SELECT CASE WHEN length(manifest) <= ?2 THEN manifest END
                 FROM installed_apps WHERE app_id = ?1",
                params![app_id, MAX_MANIFEST_BYTES as i64],
                |row| Ok(stored_manifest(row.get_ref(0)?, 0, app)),
            )
            .optional()?;
        let Some(manifest) = manifest.transpose()? else {
            return Ok(None);
        };
        // Validate metadata first, then fetch only from that same snapshot.
        validate_asset_metadata(&transaction, &manifest)?;
        let assets = {
            let mut query = transaction.prepare(
                "SELECT path, bytes FROM installed_app_assets WHERE app_id = ?1 ORDER BY path",
            )?;
            let rows = query.query_map([app_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?
        };
        transaction.commit()?;
        let package = manifest
            .verify_held(assets)
            .map_err(StoreError::InvalidPackage)?;
        Ok(Some(package))
    }
}

fn validate_asset_metadata(
    transaction: &Transaction<'_>,
    manifest: &PackageManifest,
) -> Result<(), StoreError> {
    // For stored BLOB columns SQLite obtains length from the record header.
    // The query has no asset payload in its result columns.
    let mut query = transaction.prepare(
        "SELECT CASE WHEN octet_length(path) <= ?2 THEN path END, length(bytes)
         FROM installed_app_assets WHERE app_id = ?1 ORDER BY path",
    )?;
    let mut rows = query.query(params![manifest.app_id().as_str(), MAX_PATH_BYTES as i64])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        if count == MAX_FILES {
            return Err(StoreError::InvalidPackage(PackageError::TooManyFiles));
        }
        let value = row.get_ref(0)?;
        if value == ValueRef::Null {
            return Err(StoreError::InvalidPackage(PackageError::InvalidPath));
        }
        let path = value
            .as_str()
            .map_err(|error| conversion(0, rusqlite::types::Type::Text, error))?;
        let length: i64 = row.get(1)?;
        let length = usize::try_from(length)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, length))?;
        manifest
            .check_held_asset_size(path, length)
            .map_err(StoreError::InvalidPackage)?;
        count += 1;
    }
    // The composite primary key keeps paths unique for this app.
    if count != manifest.asset_count() {
        return Err(StoreError::InvalidPackage(PackageError::MissingAsset));
    }
    Ok(())
}

fn validate_shape(connection: &Connection) -> Result<(), OpenError> {
    let mut query = connection.prepare(
        "SELECT sql FROM sqlite_schema
         WHERE name NOT GLOB 'sqlite_*'
         ORDER BY sql",
    )?;
    let mut rows = query.query([])?;
    // The same definition owns creation and validation. SQLite stores each
    // CREATE statement without its terminating semicolon.
    let mut expected = SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|statement| statement.starts_with("CREATE TABLE "))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    for definition in expected {
        let Some(row) = rows.next()? else {
            return Err(OpenError::Damaged(
                "installed app schema is incomplete".into(),
            ));
        };
        let found: Option<String> = row.get(0)?;
        if found.as_deref() != Some(definition) {
            return Err(OpenError::Damaged(
                "installed app schema does not match its definition".into(),
            ));
        }
    }
    if rows.next()?.is_some() {
        return Err(OpenError::Damaged(
            "installed app schema has unexpected entries".into(),
        ));
    }
    Ok(())
}

fn stored_manifest(
    value: ValueRef<'_>,
    index: usize,
    app: &ExtensionAppId,
) -> Result<PackageManifest, StoreError> {
    if value == ValueRef::Null {
        return Err(StoreError::InvalidPackage(PackageError::ManifestTooLarge));
    }
    let bytes = value
        .as_blob()
        .map_err(|error| conversion(index, rusqlite::types::Type::Blob, error))?;
    let manifest = parse_manifest(bytes.to_vec()).map_err(StoreError::InvalidPackage)?;
    if manifest.app_id() != app {
        return Err(StoreError::AppIdentityMismatch);
    }
    Ok(manifest)
}

fn conversion(index: usize, kind: rusqlite::types::Type, error: FromSqlError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, kind, Box::new(error))
}
