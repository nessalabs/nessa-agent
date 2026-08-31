//! The webview fills the window. Size is still reported, because the page
//! draws the panel from `--nessa-window-*` rather than from its own viewport.

use tauri::Emitter;

use crate::host::{self, PanelSize};

pub fn panel_size(window: &tauri::WebviewWindow) -> Result<PanelSize, String> {
    logical_inner_size(window)
}

pub fn fit(window: &tauri::WebviewWindow) -> Result<(), String> {
    let _ = window.emit(host::PANEL_SIZED, logical_inner_size(window)?);
    Ok(())
}

pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    let target = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Resized(_) = event {
            if let Ok(size) = logical_inner_size(&target) {
                let _ = target.emit(host::PANEL_SIZED, size);
            }
        }
    });
    Ok(())
}

fn logical_inner_size(window: &tauri::WebviewWindow) -> Result<PanelSize, String> {
    let size = window.inner_size().map_err(|error| error.to_string())?;
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    PanelSize::from_logical(
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    )
    .ok_or_else(|| String::from("the window has no size"))
}
