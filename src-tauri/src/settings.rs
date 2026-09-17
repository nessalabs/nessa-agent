//! On-disk settings.
//!
//! There is no settings UI yet, so the file is the interface: it is written
//! with its defaults on first launch, which is what makes the keys
//! discoverable. A later settings surface reads and writes the same shape.
//! Path is stage-scoped via [`crate::local_data`] (ADR 0005).
//!
//! Global summon lives in `shortcuts.json` (ADR 0004), not here.

mod storage;

use std::io;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::local_data;

/// `serde(default)` so a file written by an older build — or one a person has
/// hand-edited down to a single key — still loads, with the missing keys
/// filled from the defaults rather than failing the launch.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub panel: Panel,
    /// Keep background agents running after quitting the desktop by default.
    pub stop_agents_on_quit: bool,
    /// How far first-run setup got. A file written before this key existed
    /// loads as "not done", which is the same answer a first launch gives.
    pub onboarding: Onboarding,
}

/// What first-run setup has settled.
///
/// Only whether it finished: that is the one fact a launch acts on. The agent
/// setup chose is deliberately not kept here — nothing on either side of the
/// boundary reads it back yet, and a written key nobody reads is a promise the
/// build cannot keep. It becomes another field on this struct on the day
/// something honours it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Onboarding {
    /// Whether first-run setup has been completed. A launch with this true
    /// opens straight into the panel instead of the setup window.
    pub completed: bool,
}

/// The panel's geometry, in logical pixels. It opens in the lower right of the
/// screen it is summoned on, so these describe the size, not the corner.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Panel {
    /// The width the panel opens at on first launch. After that the window's
    /// own width wins, so a drag on the resize edge is not thrown away.
    pub width: f64,
    /// The height the panel opens at. `null` — the default — fills whatever
    /// the work area leaves once the menu bar and the Dock have taken theirs,
    /// and keeps re-filling it as the panel moves between displays.
    pub height: Option<f64>,
    /// How narrow the resize edge may drag the panel.
    pub min_width: f64,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            width: 420.0,
            height: None,
            min_width: 420.0,
        }
    }
}

/// `settings.json` under the stage-scoped config root ([`local_data`]).
fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    local_data::config_root(app).map(|root| root.join("settings.json"))
}

/// Reads the settings, falling back to the defaults for anything missing — and
/// writes the file when it is absent, so there is something to edit. A
/// malformed file is reported and ignored rather than replaced: overwriting
/// would throw away whatever the person was in the middle of typing.
pub fn load(app: &AppHandle) -> Settings {
    let Some(path) = path(app) else {
        return Settings::default();
    };

    load_from(&path, &storage::FileStorage)
}

fn load_from(path: &std::path::Path, store: &dyn storage::Storage) -> Settings {
    match store.read(path) {
        Ok(raw) => match parse(&raw) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("[nessa] {} is not valid settings: {error}", path.display());
                Settings::default()
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let settings = Settings::default();
            if let Err(error) = write(path, &settings, store) {
                eprintln!("[nessa] could not write {}: {error}", path.display());
            }
            settings
        }
        Err(error) => {
            eprintln!("[nessa] could not read {}: {error}", path.display());
            Settings::default()
        }
    }
}

pub fn save(app: &AppHandle, settings: &Settings) -> io::Result<()> {
    let path = path(app)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "settings directory unavailable"))?;
    write(&path, settings, &storage::FileStorage)
}

fn parse(raw: &str) -> Result<Settings, serde_json::Error> {
    serde_json::from_str(raw)
}

fn write(
    path: &std::path::Path,
    settings: &Settings,
    store: &dyn storage::Storage,
) -> io::Result<()> {
    let raw = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    store.write(path, format!("{raw}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, path::PathBuf, sync::Mutex};

    #[derive(Default)]
    struct FakeStorage {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
        read_error: Mutex<Option<io::ErrorKind>>,
        write_error: Mutex<Option<io::ErrorKind>>,
    }
    impl storage::Storage for FakeStorage {
        fn read(&self, path: &std::path::Path) -> io::Result<String> {
            if let Some(kind) = self.read_error.lock().unwrap().take() {
                return Err(io::Error::from(kind));
            }
            let files = self.files.lock().unwrap();
            let bytes = files
                .get(path)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            String::from_utf8(bytes.clone())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        fn write(&self, path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
            if let Some(kind) = self.write_error.lock().unwrap().take() {
                return Err(io::Error::from(kind));
            }
            self.files
                .lock()
                .unwrap()
                .insert(path.to_owned(), bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn missing_keys_take_their_defaults() {
        let settings = parse("{}").unwrap();
        assert_eq!(settings.panel.width, 420.0);
        assert!(!settings.stop_agents_on_quit);
        assert!(settings.panel.height.is_none());
        // A settings file written before first-run setup was persisted has no
        // such key, and says the same thing a first launch does: not done.
        assert!(!settings.onboarding.completed);
    }

    #[test]
    fn a_completed_first_run_survives_a_reload() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        assert!(!load_from(&path, &store).onboarding.completed);

        let mut settings = load_from(&path, &store);
        settings.onboarding.completed = true;
        write(&path, &settings, &store).unwrap();

        assert!(load_from(&path, &store).onboarding.completed);
        // Written under the same camelCase convention as every other key, so
        // the file stays the editable interface it is meant to be.
        let written = store.files.lock().unwrap().get(&path).unwrap().clone();
        let raw = String::from_utf8(written).unwrap();
        assert!(raw.contains(r#""completed": true"#), "{raw}");
        assert!(raw.contains(r#""onboarding""#), "{raw}");
    }

    #[test]
    fn camel_case_keys_round_trip() {
        let settings = parse(r#"{ "panel": { "width": 480, "minWidth": 400 } }"#).unwrap();
        assert_eq!(settings.panel.width, 480.0);
        assert_eq!(settings.panel.min_width, 400.0);
        assert!(settings.panel.height.is_none());
    }

    #[test]
    fn legacy_toggle_shortcut_is_ignored() {
        let settings =
            parse(r#"{ "toggleShortcut": "Alt+Space", "panel": { "width": 480 } }"#).unwrap();
        assert_eq!(settings.panel.width, 480.0);
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_default() {
        assert!(parse("{").is_err());
    }

    #[test]
    fn unreadable_or_invalid_settings_are_never_replaced() {
        let path = PathBuf::from("settings.json");
        for bytes in [b"not utf8 \xff".to_vec(), b"{".to_vec()] {
            let store = FakeStorage::default();
            store
                .files
                .lock()
                .unwrap()
                .insert(path.clone(), bytes.clone());
            assert_eq!(load_from(&path, &store).panel.width, 420.0);
            assert_eq!(store.files.lock().unwrap().get(&path), Some(&bytes));
        }
        let store = FakeStorage::default();
        store
            .files
            .lock()
            .unwrap()
            .insert(path.clone(), b"original".to_vec());
        *store.read_error.lock().unwrap() = Some(io::ErrorKind::PermissionDenied);
        load_from(&path, &store);
        assert_eq!(store.files.lock().unwrap().get(&path).unwrap(), b"original");
    }

    #[test]
    fn missing_settings_initialize_but_failed_replacement_preserves_prior_file() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        load_from(&path, &store);
        assert!(store.files.lock().unwrap().contains_key(&path));
        let original = store.files.lock().unwrap().get(&path).unwrap().clone();
        *store.write_error.lock().unwrap() = Some(io::ErrorKind::Interrupted);
        let changed = Settings {
            stop_agents_on_quit: true,
            ..Settings::default()
        };
        assert!(write(&path, &changed, &store).is_err());
        assert_eq!(store.files.lock().unwrap().get(&path), Some(&original));
        assert!(!load_from(&path, &store).stop_agents_on_quit);
    }

    #[test]
    fn loading_valid_settings_does_not_rewrite_unchanged_content() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        let original = br#"{"stopAgentsOnQuit":true}"#.to_vec();
        store
            .files
            .lock()
            .unwrap()
            .insert(path.clone(), original.clone());
        *store.write_error.lock().unwrap() = Some(io::ErrorKind::Other);
        assert!(load_from(&path, &store).stop_agents_on_quit);
        assert_eq!(store.files.lock().unwrap().get(&path), Some(&original));
        assert_eq!(
            *store.write_error.lock().unwrap(),
            Some(io::ErrorKind::Other)
        );
    }
}
