//! Opens the local gateway's `config.json` in the person's text editor.
//!
//! Settings › MCP servers does not draw a server's command for editing: a
//! command is rarely changed, and when it is, the file is where it lives. The
//! page asks for "the gateway's config file" and names no path; the host
//! opens the one it composed from its own service namespace
//! ([`crate::surface_credential::ServiceNamespace::config_file`]), so nothing
//! the page or an agent wrote reaches the OS opener.

use tauri::State;

use crate::composition::HostDependencies;

/// Opens the gateway's `config.json`. Refused when the service namespace was
/// not usable, or the file is not there yet: the gateway writes it on its
/// first start.
#[tauri::command]
pub fn open_gateway_config(deps: State<'_, HostDependencies>) -> Result<(), String> {
    let path = deps
        .gateway_config
        .as_ref()
        .ok_or_else(|| "The gateway's data folder isn't known to this app.".to_string())?;
    if !path.is_file() {
        return Err("The gateway hasn't written its configuration file yet.".into());
    }
    crate::platform::current().open_file(path)
}
