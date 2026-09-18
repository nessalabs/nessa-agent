//! Stage-scoped `shortcuts.json` cache ([ADR 0004](../../docs/adr/done/0004-server-owned-keybindings.md)).
//!
//! Seeded from `protocol/defaults/shortcuts.v1.json` when absent. The shell loads
//! this host-owned cache independently of the authenticated gateway session.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::composition::HostDependencies;

const DEFAULTS_JSON: &str = include_str!("../../protocol/defaults/shortcuts.v1.json");

/// Shortcut document shared with the bundled defaults file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutsDocument {
    pub version: i64,
    pub bindings: Vec<ShortcutBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutBinding {
    pub keys: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    pub scope: String,
    pub surface: String,
}

/// Currently registered global summon accelerator (for unregister on refresh).
pub struct SummonRegistration(pub Mutex<Option<String>>);

fn defaults() -> ShortcutsDocument {
    serde_json::from_str(DEFAULTS_JSON).expect("bundled shortcuts.v1.json must parse")
}

/// Where the host-owned shortcut cache is kept.
///
/// The host's own port: a document in, a document out. The real implementation
/// is [`ShortcutsFile`], built once in composition; a test substitutes one that
/// keeps the document in memory, which is what lets [`accept`] — the rule for a
/// document the shell hands back — be exercised without a disk.
pub trait ShortcutStore: Send + Sync {
    /// The current cache, seeded from the bundled defaults when it is missing
    /// or unusable.
    fn load(&self) -> ShortcutsDocument;

    /// Replaces the cache. A refused write costs the next launch its customised
    /// bindings and nothing else, so there is no failure for a caller to act on.
    fn save(&self, document: &ShortcutsDocument);
}

/// `shortcuts.json` under the stage-scoped config root ([`crate::local_data`]).
///
/// The path is resolved once, in composition: it comes from the process
/// environment, which does not change while the app runs. `None` is a launch
/// with no config root — the bundled defaults are all it can have.
pub struct ShortcutsFile {
    path: Option<PathBuf>,
}

impl ShortcutsFile {
    /// The real file, under the config root composition resolved.
    pub fn at(config_root: Option<PathBuf>) -> Self {
        Self {
            path: config_root.map(|root| root.join("shortcuts.json")),
        }
    }
}

impl ShortcutStore for ShortcutsFile {
    fn load(&self) -> ShortcutsDocument {
        match &self.path {
            Some(path) => load_from(path),
            None => defaults(),
        }
    }

    fn save(&self, document: &ShortcutsDocument) {
        if let Some(path) = &self.path {
            write(path, document);
        }
    }
}

/// Load the cache, seeding from bundled defaults when the file is missing.
fn load_from(path: &Path) -> ShortcutsDocument {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<ShortcutsDocument>(&raw) {
            Ok(doc) if doc.version == 1 => doc,
            Ok(_) => {
                eprintln!(
                    "[nessa] {} has unsupported shortcuts version; using defaults",
                    path.display()
                );
                let doc = defaults();
                write(path, &doc);
                doc
            }
            Err(error) => {
                eprintln!("[nessa] {} is not valid shortcuts: {error}", path.display());
                defaults()
            }
        },
        Err(_) => {
            let doc = defaults();
            write(path, &doc);
            doc
        }
    }
}

fn write(path: &Path, doc: &ShortcutsDocument) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string_pretty(doc) {
        let _ = fs::write(path, format!("{raw}\n"));
    }
}

/// First global `panel.summon` binding (any surface).
pub fn summon_accelerator(doc: &ShortcutsDocument) -> Option<&str> {
    doc.bindings.iter().find_map(|binding| {
        if binding.action == "panel.summon" && binding.scope == "global" {
            Some(binding.keys.as_str())
        } else {
            None
        }
    })
}

/// What applying a document does apart from the window server: refuse a version
/// this build does not speak, otherwise persist it and answer with the global
/// summon it wants registered.
///
/// Split from [`apply_shortcuts`] because the refusal is the whole rule and the
/// registration is the only part that needs a running app. A refused document
/// must not reach the store, which is a thing a test can hold.
fn accept(
    store: &dyn ShortcutStore,
    document: &ShortcutsDocument,
) -> Result<Option<String>, String> {
    if document.version != 1 {
        return Err(format!(
            "unsupported shortcuts version {}",
            document.version
        ));
    }
    store.save(document);
    Ok(summon_accelerator(document).map(str::to_string))
}

/// Persist a shortcut document and re-register the summon shortcut.
#[tauri::command]
pub fn apply_shortcuts(
    app: AppHandle,
    deps: State<'_, HostDependencies>,
    registration: State<'_, SummonRegistration>,
    document: ShortcutsDocument,
) -> Result<(), String> {
    let accelerator = accept(&*deps.shortcuts, &document)?;
    crate::shortcut::reregister_summon(&app, &registration, accelerator.as_deref());
    Ok(())
}

/// Current on-disk (or seeded) document for shell hydration.
#[tauri::command]
pub fn load_shortcuts(deps: State<'_, HostDependencies>) -> ShortcutsDocument {
    deps.shortcuts.load()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache, in memory. It also records what it was asked to save, which
    /// is how a refused document is shown never to have reached the file.
    #[derive(Default)]
    struct MemoryShortcuts(Mutex<Option<ShortcutsDocument>>);

    impl ShortcutStore for MemoryShortcuts {
        fn load(&self) -> ShortcutsDocument {
            self.0.lock().unwrap().clone().unwrap_or_else(defaults)
        }

        fn save(&self, document: &ShortcutsDocument) {
            *self.0.lock().unwrap() = Some(document.clone());
        }
    }

    impl MemoryShortcuts {
        fn saved(&self) -> Option<ShortcutsDocument> {
            self.0.lock().unwrap().clone()
        }
    }

    fn document(version: i64, keys: &str) -> ShortcutsDocument {
        ShortcutsDocument {
            version,
            bindings: vec![ShortcutBinding {
                keys: keys.to_string(),
                action: "panel.summon".to_string(),
                args: None,
                scope: "global".to_string(),
                surface: "panel".to_string(),
            }],
        }
    }

    #[test]
    fn bundled_defaults_include_summon() {
        let doc = defaults();
        assert_eq!(doc.version, 1);
        assert_eq!(summon_accelerator(&doc), Some("CmdOrCtrl+Shift+D"));
    }

    #[test]
    fn an_accepted_document_is_saved_and_names_the_summon_to_register() {
        let store = MemoryShortcuts::default();

        let accelerator = accept(&store, &document(1, "Alt+Space")).expect("version 1 is spoken");

        assert_eq!(accelerator.as_deref(), Some("Alt+Space"));
        assert_eq!(
            store.saved().map(|doc| doc.bindings[0].keys.clone()),
            Some("Alt+Space".to_string())
        );
    }

    /// The refusal has to happen before the write: a document this build cannot
    /// read back is not something to leave on disk for the next launch.
    #[test]
    fn a_version_this_build_does_not_speak_is_refused_and_never_saved() {
        let store = MemoryShortcuts::default();

        let refused = accept(&store, &document(2, "Alt+Space"));

        assert_eq!(
            refused.err().as_deref(),
            Some("unsupported shortcuts version 2")
        );
        assert!(store.saved().is_none());
    }

    /// A document with nothing global to summon with is still a document: it is
    /// saved, and the previous accelerator is dropped rather than kept.
    #[test]
    fn a_document_with_no_global_summon_is_saved_with_nothing_to_register() {
        let store = MemoryShortcuts::default();
        let mut doc = document(1, "Alt+Space");
        doc.bindings[0].scope = "panel".to_string();

        assert_eq!(accept(&store, &doc).expect("version 1 is spoken"), None);
        assert!(store.saved().is_some());
    }
}
