//! Opens the local gateway's `config.json` in the person's editor.
//!
//! Settings › MCP servers does not draw a server's command for editing: a
//! command is rarely changed, and when it is, the file is where it lives. The
//! page asks for "the gateway's config file" and names no path; the host
//! opens the one it composed from its own service namespace
//! ([`crate::surface_credential::ServiceNamespace::config_file`]), and only
//! when it is a regular file, as the gateway itself only reads one. So no path
//! the page or an agent chose reaches the OS opener. The openers themselves
//! (`open -t`, `xdg-open`) are the platform adapters and are not exercised by
//! these tests.

use std::path::Path;

use tauri::State;

use crate::composition::HostDependencies;
use crate::platform::Host;

/// Opens the gateway's `config.json`. Refused when the service namespace was
/// not usable, or the file is not there yet: the gateway writes it on its
/// first start.
#[tauri::command]
pub fn open_gateway_config(deps: State<'_, HostDependencies>) -> Result<(), String> {
    open_config(deps.gateway_config.as_deref(), crate::platform::current())
}

/// The decision behind [`open_gateway_config`], with the host injected.
fn open_config(path: Option<&Path>, host: &dyn Host) -> Result<(), String> {
    let path =
        path.ok_or_else(|| "The gateway's data folder isn't known to this app.".to_string())?;
    // Not followed through a link: the gateway refuses a config.json that is
    // not a regular file, and a link could point the opener anywhere.
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Err("config.json isn't a regular file, so it wasn't opened.".into()),
        Err(_) => return Err("The gateway hasn't written its configuration file yet.".into()),
    }
    host.open_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A host that records what it was asked to open.
    #[derive(Default)]
    struct Recording(Mutex<Vec<std::path::PathBuf>>);

    impl Host for Recording {
        fn panel_size(&self, _: &tauri::WebviewWindow) -> Result<crate::host::PanelSize, String> {
            unreachable!("not a window host")
        }
        fn fit_viewport(&self, _: &tauri::WebviewWindow) -> Result<(), String> {
            unreachable!("not a window host")
        }
        fn watch_viewport(&self, _: &tauri::WebviewWindow) -> Result<(), String> {
            unreachable!("not a window host")
        }
        fn open_file(&self, path: &Path) -> Result<(), String> {
            self.0.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    fn opened(host: &Recording) -> Vec<std::path::PathBuf> {
        host.0.lock().unwrap().clone()
    }

    #[test]
    fn a_written_config_is_opened() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        std::fs::write(&file, "{}").unwrap();
        let host = Recording::default();
        assert_eq!(open_config(Some(&file), &host), Ok(()));
        assert_eq!(opened(&host), vec![file]);
    }

    #[test]
    fn no_namespace_or_no_file_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let host = Recording::default();
        let unwritten = Err("The gateway hasn't written its configuration file yet.".into());
        let irregular = Err("config.json isn't a regular file, so it wasn't opened.".into());
        assert!(open_config(None, &host).is_err());
        assert_eq!(
            open_config(Some(&dir.path().join("config.json")), &host),
            unwritten
        );
        // A folder by that name is not the file either.
        std::fs::create_dir(dir.path().join("config.json")).unwrap();
        assert_eq!(
            open_config(Some(&dir.path().join("config.json")), &host),
            irregular
        );
        assert!(opened(&host).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_its_place_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("config.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let host = Recording::default();
        assert_eq!(
            open_config(Some(&link), &host),
            Err("config.json isn't a regular file, so it wasn't opened.".into())
        );
        assert!(opened(&host).is_empty());
    }
}
