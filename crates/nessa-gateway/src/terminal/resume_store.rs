//! Where resume settings and saved records are kept across restarts: a
//! folder only the person can read, one file for the settings and one per
//! terminal holding a record, each written whole when it changes.
use super::ResumeSettings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A terminal whose program asked to come back, with what it needs to start
/// again where it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedTerminal {
    /// The terminal's id; a resumed terminal keeps it, so a pane showing it
    /// before the restart shows it after.
    pub id: String,
    pub request_id: String,
    pub title: String,
    pub organization: String,
    pub principal: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// The terminal's own folder (the channel's or the agent's).
    pub folder: String,
    pub cols: u16,
    pub rows: u16,
    pub record: SavedRecord,
}

/// The record a program reported last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedRecord {
    pub pid: u32,
    /// When that process started, to tell it from a later one with its pid.
    #[serde(default)]
    pub started: Option<u64>,
    /// The account that accepted it; another account never resumes it.
    pub uid: u32,
    pub argv: Vec<String>,
    /// The folder the program named, if it named one.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The program's folder as the system said when the report was taken.
    #[serde(default)]
    pub owner_cwd: Option<String>,
    pub time: u64,
    pub saved_at_ms: u64,
    /// Started once by a restart, and not reported since: never started
    /// again, so a command that fails or crashes is not run on every start.
    #[serde(default)]
    pub resumed: bool,
}

impl SavedRecord {
    /// Where it runs: the folder it named, else where the system saw it.
    pub fn folder(&self) -> Option<&str> {
        self.cwd.as_deref().or(self.owner_cwd.as_deref())
    }
}

/// What keeps settings and records.
pub trait ResumeStore: Send + Sync {
    /// The kept settings; off when none were kept or they cannot be read.
    fn settings(&self) -> ResumeSettings;
    fn save_settings(&self, settings: &ResumeSettings) -> Result<(), String>;
    /// Every saved terminal that can be read.
    fn saved(&self) -> Vec<SavedTerminal>;
    fn save(&self, terminal: &SavedTerminal) -> Result<(), String>;
    /// Forget `id`'s record; forgetting one not kept is not an error.
    fn remove(&self, id: &str) -> Result<(), String>;
}

/// Settings and records in `dir`.
pub struct FileResumeStore {
    dir: PathBuf,
    /// One write at a time, so two changes never interleave a file.
    writing: Mutex<()>,
}

impl FileResumeStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            writing: Mutex::new(()),
        }
    }

    fn records(&self) -> PathBuf {
        self.dir.join("saved")
    }

    fn record(&self, id: &str) -> Option<PathBuf> {
        let valid = !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        valid.then(|| self.records().join(format!("{id}.json")))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let _held = self
            .writing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let parent = path.parent().ok_or("no folder")?;
        private_dir(&self.dir)?;
        private_dir(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Make `dir` if needed, readable by its owner only.
fn private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl ResumeStore for FileResumeStore {
    fn settings(&self) -> ResumeSettings {
        std::fs::read(self.dir.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ResumeSettings>(&bytes).ok())
            .and_then(|settings| settings.checked().ok())
            .unwrap_or_default()
    }

    fn save_settings(&self, settings: &ResumeSettings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        self.write(&self.dir.join("settings.json"), &bytes)
    }

    fn saved(&self) -> Vec<SavedTerminal> {
        let Ok(entries) = std::fs::read_dir(self.records()) else {
            return Vec::new();
        };
        let mut saved: Vec<SavedTerminal> = entries
            .flatten()
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<SavedTerminal>(&bytes).ok())
            .filter(|terminal| self.record(&terminal.id).is_some())
            .collect();
        saved.sort_by_key(|terminal| terminal.record.saved_at_ms);
        saved
    }

    fn save(&self, terminal: &SavedTerminal) -> Result<(), String> {
        let path = self.record(&terminal.id).ok_or("invalid id")?;
        let bytes = serde_json::to_vec_pretty(terminal).map_err(|e| e.to_string())?;
        self.write(&path, &bytes)
    }

    fn remove(&self, id: &str) -> Result<(), String> {
        let path = self.record(id).ok_or("invalid id")?;
        let _held = self
            .writing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Settings and records in memory, for tests and a gateway with no folder.
#[derive(Default)]
pub struct MemoryResumeStore {
    settings: Mutex<ResumeSettings>,
    saved: Mutex<Vec<SavedTerminal>>,
}

impl ResumeStore for MemoryResumeStore {
    fn settings(&self) -> ResumeSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn save_settings(&self, settings: &ResumeSettings) -> Result<(), String> {
        *self.settings.lock().map_err(|e| e.to_string())? = settings.clone();
        Ok(())
    }

    fn saved(&self) -> Vec<SavedTerminal> {
        self.saved.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn save(&self, terminal: &SavedTerminal) -> Result<(), String> {
        let mut saved = self.saved.lock().map_err(|e| e.to_string())?;
        saved.retain(|each| each.id != terminal.id);
        saved.push(terminal.clone());
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), String> {
        self.saved
            .lock()
            .map_err(|e| e.to_string())?
            .retain(|each| each.id != id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{ResumeRule, RuleAction};

    fn terminal(id: &str) -> SavedTerminal {
        SavedTerminal {
            id: id.into(),
            request_id: "r".into(),
            title: "zsh".into(),
            organization: "o".into(),
            principal: "p".into(),
            channel_id: None,
            conversation_id: None,
            folder: "/tmp".into(),
            cols: 80,
            rows: 24,
            record: SavedRecord {
                pid: 7,
                started: Some(1),
                uid: 501,
                argv: vec!["nvim".into()],
                cwd: None,
                owner_cwd: Some("/tmp".into()),
                time: 1,
                saved_at_ms: 1,
                resumed: false,
            },
        }
    }

    #[test]
    fn keeps_settings_and_records_in_private_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileResumeStore::new(dir.path().join("terminals"));
        assert_eq!(store.settings(), ResumeSettings::default());
        assert!(store.saved().is_empty());
        let settings = ResumeSettings {
            enabled: true,
            rules: vec![ResumeRule {
                program: "nvim".into(),
                folder: None,
                args: Vec::new(),
                action: RuleAction::Allow,
            }],
        };
        store.save_settings(&settings).unwrap();
        store.save(&terminal("t_1")).unwrap();
        store.save(&terminal("t_2")).unwrap();
        let again = FileResumeStore::new(dir.path().join("terminals"));
        assert_eq!(again.settings(), settings);
        assert_eq!(again.saved().len(), 2);
        again.remove("t_1").unwrap();
        again.remove("t_1").unwrap();
        assert_eq!(again.saved(), vec![terminal("t_2")]);
        assert!(again.save(&terminal("../escape")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.path().join("terminals")), 0o700);
            assert_eq!(mode(&dir.path().join("terminals/saved")), 0o700);
            assert_eq!(mode(&dir.path().join("terminals/saved/t_2.json")), 0o600);
            assert_eq!(mode(&dir.path().join("terminals/settings.json")), 0o600);
        }
    }
}
