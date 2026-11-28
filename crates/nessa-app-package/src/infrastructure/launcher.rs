//! The launcher record: one installed app's verified bytes, written beside the
//! app's own host so that host can open it with Nessa closed.
//!
//! ```text
//! Nessa (install, Open) ── launcher_record(verified package) ──▶ <launcher>/app.json
//! nessa-app-host <launcher> ── read_launcher_record ──▶ verify again ──▶ serve the
//!     package's own files under APP_CONTENT_SECURITY_POLICY, nothing else
//! ```
//!
//! The record is the inline package plus the identity and digest it was
//! written for. Reading it verifies every byte against the manifest and the
//! manifest against that digest, so a torn or changed file opens nothing.
//! It proves only that the files are whole: anyone who can write the
//! launcher's folder can replace both, as with any app on the computer.

use serde::{Deserialize, Serialize};

use crate::{encode_inline_package, verify_inline_package, InlineAsset, InlinePackage};
use crate::{InlineRefusal, VerifiedPackage};

/// The record's file name inside a launcher's folder.
pub const LAUNCHER_RECORD: &str = "app.json";

/// In a launcher's folder, the file its host holds an exclusive lock on for
/// as long as the app runs. Held is running; the lock goes with the process.
pub const RUNNING_LOCK: &str = "running.lock";

/// Beside the lock, the running host's process id, for Stop. Read only while
/// the lock is held, so a stale id is never acted on.
pub const RUNNING_PID: &str = "running.pid";

/// The most a launcher record may hold. An inline package is at most 16 KiB,
/// which base64 and the record's own fields keep well under this.
pub const MAX_LAUNCHER_RECORD_BYTES: usize = 64 * 1024;

/// What an installed app's page may load: its own files only. No network,
/// no frames, no forms, no plugins.
///
/// The scheme is named beside `'self'` because WebKit does not always match
/// a custom scheme's own origin with `'self'`.
pub const APP_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
    script-src 'self' nessa-app: 'unsafe-inline'; \
    style-src 'self' nessa-app: 'unsafe-inline'; \
    img-src 'self' nessa-app: data: blob:; \
    font-src 'self' nessa-app: data:; \
    media-src 'self' nessa-app: data: blob:; \
    connect-src 'self' nessa-app:; \
    frame-src 'none'; object-src 'none'; worker-src 'none'; \
    form-action 'none'; base-uri 'none'";

const RECORD_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    record_version: u32,
    app_id: String,
    digest: String,
    manifest: String,
    assets: Vec<RecordAsset>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordAsset {
    path: String,
    bytes: String,
}

/// Why a launcher record opens nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LauncherRefusal {
    /// Larger than any record Nessa writes.
    TooLarge,
    /// Not a record of this version.
    Malformed,
    /// The bytes do not verify against their manifest.
    Package(InlineRefusal),
    /// They verify, but not as the app and digest the record names.
    NotThePackageNamed,
}

/// The record for `package`, to be written as the launcher's `app.json`.
///
/// # Errors
/// Refuses a package outside the inline profile; such a package cannot have
/// been installed.
pub fn launcher_record(package: &VerifiedPackage) -> Result<Vec<u8>, InlineRefusal> {
    let inline = encode_inline_package(package)?;
    let record = Record {
        record_version: RECORD_VERSION,
        app_id: package.manifest().app_id().as_str().to_owned(),
        digest: package.digest(),
        manifest: inline.manifest_base64,
        assets: inline
            .assets
            .into_iter()
            .map(|asset| RecordAsset {
                path: asset.path,
                bytes: asset.bytes_base64,
            })
            .collect(),
    };
    serde_json::to_vec_pretty(&record).map_err(|_| InlineRefusal::ManifestInvalid)
}

/// The verified package a launcher record holds.
///
/// # Errors
/// Refuses a record that is oversized, malformed, does not verify, or holds
/// another app or digest than it names.
pub fn read_launcher_record(bytes: &[u8]) -> Result<VerifiedPackage, LauncherRefusal> {
    if bytes.len() > MAX_LAUNCHER_RECORD_BYTES {
        return Err(LauncherRefusal::TooLarge);
    }
    let record: Record = serde_json::from_slice(bytes).map_err(|_| LauncherRefusal::Malformed)?;
    if record.record_version != RECORD_VERSION {
        return Err(LauncherRefusal::Malformed);
    }
    let package = verify_inline_package(&InlinePackage {
        manifest_base64: record.manifest,
        assets: record
            .assets
            .into_iter()
            .map(|asset| InlineAsset {
                path: asset.path,
                bytes_base64: asset.bytes,
            })
            .collect(),
    })
    .map_err(LauncherRefusal::Package)?;
    if package.manifest().app_id().as_str() != record.app_id || package.digest() != record.digest {
        return Err(LauncherRefusal::NotThePackageNamed);
    }
    Ok(package)
}

/// The content type an app's file is served with, by its extension. A file
/// of no known kind is served as bytes, which a page cannot run.
pub fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or_default();
    match extension {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaffold_package;

    fn starter(app: &str) -> VerifiedPackage {
        scaffold_package(app.to_owned(), "1.0.0".to_owned()).unwrap()
    }

    #[test]
    fn a_record_reads_back_as_the_package_it_was_written_for() {
        let package = starter("notes");
        let record = launcher_record(&package).unwrap();
        let read = read_launcher_record(&record).unwrap();
        assert_eq!(read.digest(), package.digest());
        assert_eq!(read.manifest().app_id().as_str(), "notes");
        assert_eq!(
            read.assets().collect::<Vec<_>>(),
            package.assets().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_changed_record_opens_nothing() {
        let record = launcher_record(&starter("notes")).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&record).unwrap();

        // Another identity named over the same bytes.
        let mut renamed = value.clone();
        renamed["appId"] = "calendar".into();
        assert_eq!(
            read_launcher_record(&serde_json::to_vec(&renamed).unwrap()).err(),
            Some(LauncherRefusal::NotThePackageNamed)
        );

        // Another digest named.
        let mut redigested = value.clone();
        redigested["digest"] = "0".repeat(64).into();
        assert_eq!(
            read_launcher_record(&serde_json::to_vec(&redigested).unwrap()).err(),
            Some(LauncherRefusal::NotThePackageNamed)
        );

        // A file's bytes changed.
        value["assets"][0]["bytes"] = "PGgxPmhpPC9oMT4=".into();
        assert!(matches!(
            read_launcher_record(&serde_json::to_vec(&value).unwrap()),
            Err(LauncherRefusal::Package(_))
        ));

        // Torn, unknown or oversized.
        assert_eq!(
            read_launcher_record(&record[..record.len() / 2]).err(),
            Some(LauncherRefusal::Malformed)
        );
        assert_eq!(
            read_launcher_record(br#"{"recordVersion":2}"#).err(),
            Some(LauncherRefusal::Malformed)
        );
        assert_eq!(
            read_launcher_record(&vec![b' '; MAX_LAUNCHER_RECORD_BYTES + 1]).err(),
            Some(LauncherRefusal::TooLarge)
        );
    }

    #[test]
    fn files_are_served_as_their_kind_and_unknown_ones_as_bytes() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("static/app.mjs"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("icon.png"), "image/png");
        assert_eq!(content_type("notes"), "application/octet-stream");
        assert_eq!(content_type("run.exe"), "application/octet-stream");
    }

    #[test]
    fn the_policy_reaches_no_network() {
        assert!(APP_CONTENT_SECURITY_POLICY.starts_with("default-src 'none'"));
        for source in ["http:", "https:", "ws:", "wss:", "*"] {
            assert!(
                !APP_CONTENT_SECURITY_POLICY.contains(source),
                "the policy names {source}"
            );
        }
    }
}
