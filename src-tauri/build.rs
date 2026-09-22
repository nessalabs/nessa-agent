fn main() {
    let attributes = tauri_build::Attributes::new().plugin(
        "dev-console",
        tauri_build::InlinedPlugin::new().commands(&["forward_webview_console"]),
    );
    tauri_build::try_build(attributes).expect("failed to build Tauri application metadata")
}
