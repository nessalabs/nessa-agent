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
/// Two facts, and both are read back. Setup runs in a window of its own that is
/// gone before the panel needs either, so what it decided is kept here rather
/// than handed over.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Onboarding {
    /// Whether first-run setup has been completed. A launch with this true
    /// opens straight into the panel instead of the setup window.
    pub completed: bool,
    /// The agent setup chose, by the name the gateway knows it by. Absent where
    /// nobody has chosen one, and a conversation then starts on whichever agent
    /// the gateway is configured to default to — never on a guess made here.
    /// Kept as written rather than parsed: which agents exist is the gateway's
    /// to say, and a desktop that validated the name would have to be rebuilt
    /// to learn a new one.
    pub agent: Option<String>,
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

/// What one read of the settings file found.
///
/// The three outcomes are kept apart because a reader that only wants to *carry
/// on* and a reader that wants to *write back* need different answers from the
/// same read. Both go through [`read_from`]; each decides for itself what an
/// unusable file means.
enum Found {
    /// The file was read and is valid settings.
    Settings(Settings),
    /// There is no file yet. Defaults are a legitimate basis: this is the first
    /// launch, and nothing of the person's can be lost by writing them.
    Absent,
    /// There is a file, but this build cannot make settings of it — malformed
    /// JSON, or a read that failed for a reason other than its absence. The
    /// contents are still on disk and still whatever the person meant, so the
    /// defaults describe nobody's settings.
    Unusable(io::Error),
}

/// The one read. Shared by the lenient [`load`] and the strict [`update`] so
/// there is a single account of what the file says.
fn read_from(path: &std::path::Path, store: &dyn storage::Storage) -> Found {
    match store.read(path) {
        Ok(raw) => match parse(&raw) {
            Ok(settings) => Found::Settings(settings),
            Err(error) => Found::Unusable(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not valid settings: {error}", path.display()),
            )),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Found::Absent,
        Err(error) => Found::Unusable(io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", path.display()),
        )),
    }
}

/// Reads the settings, falling back to the defaults for anything missing — and
/// writes the file when it is absent, so there is something to edit. A
/// malformed file is reported and ignored rather than replaced: overwriting
/// would throw away whatever the person was in the middle of typing.
///
/// This is the *startup* read, and carrying on with defaults is the right
/// answer for it: a launch that cannot read its settings still has to open a
/// panel. It is the wrong basis for changing the file, which is what [`update`]
/// is for.
pub fn load(app: &AppHandle) -> Settings {
    let Some(path) = path(app) else {
        return Settings::default();
    };

    load_from(&path, &storage::FileStorage)
}

fn load_from(path: &std::path::Path, store: &dyn storage::Storage) -> Settings {
    match read_from(path, store) {
        Found::Settings(settings) => settings,
        Found::Absent => {
            let settings = Settings::default();
            if let Err(error) = write(path, &settings, store) {
                eprintln!("[nessa] could not write {}: {error}", path.display());
            }
            settings
        }
        Found::Unusable(error) => {
            eprintln!("[nessa] {error}");
            Settings::default()
        }
    }
}

/// Changes the settings on disk: read, apply `change`, write the result back.
///
/// Refuses — and writes nothing at all — when the file is there but cannot be
/// read or parsed. Defaults are what a *launch* falls back to; saving them over
/// a file that merely failed to parse would replace the person's real settings
/// with a build's opinion of them, silently, because the write itself succeeds.
/// An absent file is the one case where the defaults are nobody's loss, and is
/// the first-launch materialisation this module is built around.
///
/// Returns what was written, for a caller that has to show the new value —
/// a menu item's tick, say — without reading the file a second time and
/// risking a different answer than the one it just saved.
///
/// # Errors
///
/// The typed read failure (`InvalidData` for a malformed file, the underlying
/// kind otherwise) when the file is unusable, the write's own failure when the
/// replacement does not land, and `NotFound` when there is no config root to
/// write into.
pub fn update(app: &AppHandle, change: impl FnOnce(&mut Settings)) -> io::Result<Settings> {
    let path = path(app)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "settings directory unavailable"))?;
    update_in(&path, &storage::FileStorage, change)
}

fn update_in(
    path: &std::path::Path,
    store: &dyn storage::Storage,
    change: impl FnOnce(&mut Settings),
) -> io::Result<Settings> {
    let mut settings = match read_from(path, store) {
        Found::Settings(settings) => settings,
        Found::Absent => Settings::default(),
        Found::Unusable(error) => return Err(error),
    };
    change(&mut settings);
    write(path, &settings, store)?;
    Ok(settings)
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
        // Nor does it name an agent, which is why the gateway's own default is
        // what a conversation starts on rather than a guess made here.
        assert_eq!(settings.onboarding.agent, None);
    }

    #[test]
    fn the_agent_setup_chose_survives_a_reload() {
        // Setup runs in a window that is gone by the time the panel asks, so
        // this is the whole of how the choice reaches it.
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        let mut settings = load_from(&path, &store);
        settings.onboarding = Onboarding {
            completed: true,
            agent: Some("codex".into()),
        };
        write(&path, &settings, &store).unwrap();

        assert_eq!(
            load_from(&path, &store).onboarding.agent.as_deref(),
            Some("codex")
        );
        let written = store.files.lock().unwrap().get(&path).unwrap().clone();
        let raw = String::from_utf8(written).unwrap();
        assert!(raw.contains(r#""agent": "codex""#), "{raw}");
    }

    #[test]
    fn an_agent_name_this_build_does_not_know_is_kept_as_written() {
        // Which agents exist is the gateway's to say. A desktop that rejected
        // an unfamiliar name would have to be rebuilt to learn a new one, and
        // would meanwhile throw away a choice somebody actually made.
        let settings = parse(r#"{ "onboarding": { "agent": "gemini" } }"#).unwrap();
        assert_eq!(settings.onboarding.agent.as_deref(), Some("gemini"));
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

    /// The case the bug destroyed: a file that cannot be parsed must not be
    /// replaced by an update, and the original bytes must still be on disk —
    /// atomic replacement only promises the file is never torn, not that it is
    /// replaced with the right contents.
    #[test]
    fn an_update_refuses_a_malformed_file_and_leaves_its_bytes_alone() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        let original = br#"{ "panel": { "width": 640 }, "#.to_vec();
        store
            .files
            .lock()
            .unwrap()
            .insert(path.clone(), original.clone());

        let error = update_in(&path, &store, |settings| {
            settings.onboarding.completed = true;
        })
        .expect_err("defaults are not a basis for replacing a file that failed to parse");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(store.files.lock().unwrap().get(&path), Some(&original));
    }

    /// A transient read failure with a writer that would happily succeed: the
    /// update is still refused, because the defaults describe nobody's settings.
    #[test]
    fn an_update_refuses_an_unreadable_file_even_when_the_write_would_succeed() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        let original = br#"{"stopAgentsOnQuit":true}"#.to_vec();
        store
            .files
            .lock()
            .unwrap()
            .insert(path.clone(), original.clone());
        *store.read_error.lock().unwrap() = Some(io::ErrorKind::PermissionDenied);

        let error = update_in(&path, &store, |settings| {
            settings.onboarding.completed = true;
        })
        .expect_err("a read that failed is not permission to write defaults");

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(store.files.lock().unwrap().get(&path), Some(&original));
    }

    /// The loss the defect actually caused: settings that are readable and not
    /// the defaults keep every key the change did not touch.
    #[test]
    fn an_update_keeps_the_keys_it_did_not_change() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        write(
            &path,
            &Settings {
                panel: Panel {
                    width: 640.0,
                    height: Some(800.0),
                    min_width: 500.0,
                },
                stop_agents_on_quit: true,
                onboarding: Onboarding::default(),
            },
            &store,
        )
        .unwrap();

        update_in(&path, &store, |settings| {
            settings.onboarding.completed = true;
        })
        .expect("a readable file takes the change");

        let saved = load_from(&path, &store);
        assert!(saved.onboarding.completed);
        assert_eq!(saved.panel.width, 640.0);
        assert_eq!(saved.panel.height, Some(800.0));
        assert_eq!(saved.panel.min_width, 500.0);
        assert!(saved.stop_agents_on_quit);
    }

    /// What the tray's quit-policy item ticks has to be what reached the disk,
    /// not what the caller assumed it flipped: the value it toggled came from
    /// the file this same call read.
    #[test]
    fn an_update_reports_the_value_it_wrote() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();
        write(
            &path,
            &Settings {
                stop_agents_on_quit: true,
                ..Settings::default()
            },
            &store,
        )
        .unwrap();

        let written = update_in(&path, &store, |settings| {
            settings.stop_agents_on_quit = !settings.stop_agents_on_quit;
        })
        .expect("a readable file takes the change");

        assert!(!written.stop_agents_on_quit);
        assert!(!load_from(&path, &store).stop_agents_on_quit);
    }

    /// No file yet is the one case where the defaults are nobody's loss.
    #[test]
    fn an_update_writes_over_an_absent_file() {
        let path = PathBuf::from("settings.json");
        let store = FakeStorage::default();

        update_in(&path, &store, |settings| {
            settings.onboarding.completed = true;
        })
        .expect("defaults are a legitimate basis when there is nothing on disk");

        assert!(load_from(&path, &store).onboarding.completed);
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
