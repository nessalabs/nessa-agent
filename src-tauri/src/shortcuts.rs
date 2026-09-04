//! Stage-scoped `shortcuts.json` cache ([ADR 0004](../../docs/adr/0004-server-owned-keybindings.md)).
//!
//! Seeded from `protocol/defaults/shortcuts.v1.json` when absent; replaced when
//! the shell pushes a HelloOk document after connect.

use std::fs;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::local_data;

const DEFAULTS_JSON: &str = include_str!("../../protocol/defaults/shortcuts.v1.json");

/// Wire-shaped shortcut document (same as HelloOk.shortcuts / defaults file).
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

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    local_data::config_root(app).map(|root| root.join("shortcuts.json"))
}

fn defaults() -> ShortcutsDocument {
    serde_json::from_str(DEFAULTS_JSON).expect("bundled shortcuts.v1.json must parse")
}

/// Load the cache, seeding from bundled defaults when the file is missing.
pub fn load(app: &AppHandle) -> ShortcutsDocument {
    let Some(path) = path(app) else {
        return defaults();
    };

    match fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<ShortcutsDocument>(&raw) {
            Ok(doc) if doc.version == 1 => doc,
            Ok(_) => {
                eprintln!(
                    "[nessa] {} has unsupported shortcuts version; using defaults",
                    path.display()
                );
                let doc = defaults();
                write(&path, &doc);
                doc
            }
            Err(error) => {
                eprintln!("[nessa] {} is not valid shortcuts: {error}", path.display());
                defaults()
            }
        },
        Err(_) => {
            let doc = defaults();
            write(&path, &doc);
            doc
        }
    }
}

fn write(path: &std::path::Path, doc: &ShortcutsDocument) {
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

/// Persist a HelloOk (or defaults) document and re-register the summon shortcut.
#[tauri::command]
pub fn apply_shortcuts(
    app: AppHandle,
    registration: State<'_, SummonRegistration>,
    document: ShortcutsDocument,
) -> Result<(), String> {
    if document.version != 1 {
        return Err(format!(
            "unsupported shortcuts version {}",
            document.version
        ));
    }
    if let Some(path) = path(&app) {
        write(&path, &document);
    }
    crate::shortcut::reregister_summon(&app, &registration, summon_accelerator(&document));
    Ok(())
}

/// Current on-disk (or seeded) document for shell hydrate before HelloOk.
#[tauri::command]
pub fn load_shortcuts(app: AppHandle) -> ShortcutsDocument {
    load(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_defaults_include_summon() {
        let doc = defaults();
        assert_eq!(doc.version, 1);
        assert_eq!(
            summon_accelerator(&doc),
            Some("CmdOrCtrl+Shift+D")
        );
    }
}
