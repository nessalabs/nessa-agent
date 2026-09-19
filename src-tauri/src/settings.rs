//! On-disk settings.
//!
//! There is no settings UI yet, so the file is the interface: it is written
//! with its defaults on first launch, which is what makes the keys
//! discoverable. A later settings surface reads and writes the same shape.
//! Path is stage-scoped via [`crate::local_data`] (ADR 0005).
//!
//! Global summon lives in `shortcuts.json` (ADR 0004), not here.

pub(crate) mod storage;

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

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

/// Where the desktop's settings are kept, as the rest of the host asks about
/// them.
///
/// The host's own port, in the host's vocabulary: settings in, settings out,
/// and no path, file, or serializer visible to a caller. The real
/// implementation is [`SettingsFile`], built once in composition; a test
/// substitutes one holding the file in memory, which is what lets the
/// decisions that read and write settings — the first-run flag, the quit
/// policy — be exercised without a disk or a running app.
pub trait SettingsStore: Send + Sync {
    /// The startup read: the defaults stand in for anything missing or
    /// unreadable, because a launch that cannot read its settings still has to
    /// open a panel. See [`load_from`].
    fn load(&self) -> Settings;

    /// Read, apply `change`, write the result back, and answer with what was
    /// written. See [`update_in`] for what an unusable file does here.
    ///
    /// # Errors
    ///
    /// The typed read failure when the file is there but unusable, the write's
    /// own failure when the replacement does not land, and `NotFound` when
    /// there is no config root to write into.
    fn update(&self, change: &mut dyn FnMut(&mut Settings)) -> io::Result<Settings>;
}

/// `settings.json` under the stage-scoped config root ([`crate::local_data`]).
///
/// The path is resolved once, in composition, rather than per call: it is
/// derived from the process environment, which does not change while the app
/// runs. `None` is a launch with no config root at all — there is nothing to
/// read and nowhere to write, and the defaults are all it can have.
pub struct SettingsFile {
    path: Option<PathBuf>,
    storage: Arc<dyn storage::Storage>,
}

impl SettingsFile {
    /// The real file, under the config root composition resolved.
    pub fn at(config_root: Option<PathBuf>) -> Self {
        Self {
            path: config_root.map(|root| root.join("settings.json")),
            storage: Arc::new(storage::FileStorage),
        }
    }
}

impl SettingsStore for SettingsFile {
    fn load(&self) -> Settings {
        match &self.path {
            Some(path) => load_from(path, &*self.storage),
            None => Settings::default(),
        }
    }

    fn update(&self, change: &mut dyn FnMut(&mut Settings)) -> io::Result<Settings> {
        let path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "settings directory unavailable")
        })?;
        update_in(path, &*self.storage, change)
    }
}

/// A [`SettingsFile`] whose file is a map in this process.
///
/// Built here, next to the rules it stands in for, so every module that makes a
/// decision from settings substitutes the same one.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) struct InMemorySettings {
        /// Hand this to whatever is under test.
        pub(crate) store: SettingsFile,
        /// The "disk", for seeding a file or reading back what landed.
        pub(crate) storage: Arc<storage::MemoryStorage>,
        /// The one path `store` reads and writes.
        pub(crate) path: PathBuf,
    }

    pub(crate) fn in_memory() -> InMemorySettings {
        let storage = Arc::new(storage::MemoryStorage::default());
        let path = PathBuf::from("settings.json");
        InMemorySettings {
            store: SettingsFile {
                path: Some(path.clone()),
                storage: storage.clone(),
            },
            storage,
            path,
        }
    }
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
fn read_from(path: &Path, store: &dyn storage::Storage) -> Found {
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
/// panel. It is the wrong basis for changing the file, which is what
/// [`update_in`] is for.
fn load_from(path: &Path, store: &dyn storage::Storage) -> Settings {
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
fn update_in(
    path: &Path,
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

fn write(path: &Path, settings: &Settings, store: &dyn storage::Storage) -> io::Result<()> {
    let raw = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    store.write(path, format!("{raw}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage::MemoryStorage as FakeStorage;

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

    /// The port over a file: what a caller gets is the same lenient load and
    /// strict update the functions above describe, reached without a path.
    #[test]
    fn the_store_loads_and_updates_the_file_under_its_root() {
        let settings = testing::in_memory();

        assert!(!settings.store.load().stop_agents_on_quit);
        let written = settings
            .store
            .update(&mut |chosen| chosen.stop_agents_on_quit = true)
            .expect("an absent file takes the change");

        assert!(written.stop_agents_on_quit);
        assert!(settings.store.load().stop_agents_on_quit);
    }

    /// No config root is not an empty settings file: there is nothing to read
    /// and nowhere to write, and a launch still has to open a panel.
    #[test]
    fn a_launch_with_no_config_root_gets_the_defaults_and_refuses_to_write() {
        let store = SettingsFile::at(None);

        assert_eq!(store.load().panel.width, Panel::default().width);
        assert_eq!(
            store
                .update(&mut |chosen| chosen.onboarding.completed = true)
                .expect_err("there is nowhere to write")
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
