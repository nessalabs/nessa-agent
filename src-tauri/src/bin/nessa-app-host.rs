//! `nessa-app-host`: one installed app, run as an app of its own.
//!
//! ```text
//! launcher (macOS: <app>.app in ~/Applications/Nessa Apps; elsewhere a folder
//!   Nessa keeps, with a menu entry on Linux) ── runs ──▶ nessa-app-host
//!     read <launcher>/app.json ── verify every byte (nessa-app-package)
//!     lock <launcher>/running.lock (held = running; a second launch leaves)
//!     one window: the app's own files under nessa-app://, nothing else
//!     a dynamic app: its server first (nessa_app_package::start_server),
//!       on 127.0.0.1, the one other place its page may reach, with this
//!       launch's token in `window.nessaApp`; it ends with this process
//! ```
//!
//! Each launcher holds its own copy of this binary, so each app is its own
//! process with its own Dock or taskbar entry and icon, and runs with Nessa's
//! main window, or Nessa itself, closed. Its page holds no gateway credential
//! and has no command to call: it can load the package's files and nothing
//! more (`APP_CONTENT_SECURITY_POLICY`), and may navigate nowhere else. A
//! dynamic app's server holds the app's own credential, which Nessa kept in
//! the launcher at install; its page reaches that server and nothing more.
//!
//! On macOS the launcher's folder is the bundle's `Contents/Resources/
//! nessa-app`; elsewhere it is named with `--app <folder>`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        match host::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("nessa-app-host: {error}");
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        ExitCode::FAILURE
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod host {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use fs2::FileExt;
    use nessa_app_package::{
        content_type, read_launcher_record, start_server, VerifiedPackage,
        APP_CONTENT_SECURITY_POLICY, LAUNCHER_RECORD, MAX_LAUNCHER_RECORD_BYTES, RUNNING_LOCK,
        RUNNING_PID,
    };
    use tauri::http::{header, Method, Request, Response, StatusCode};
    use tauri::webview::{NewWindowResponse, PageLoadEvent};
    use tauri::{Builder, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, Wry};

    const SCHEME: &str = "nessa-app";

    /// `@nessa/client` as one module, built with this host
    /// (`scripts/desktop/app-client-bundle.mjs` from `build.rs`): what a
    /// dynamic app's server imports as `@nessa/client`.
    const APP_CLIENT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app-client.mjs"));
    const WINDOW: &str = "app";

    /// Set to a URL by the Linux native smoke: once the page has loaded, the
    /// host reports what it drew, whether the page could read its own file,
    /// whether it has WebRTC or could open a window, and whether a request
    /// to that URL got out, then quits. It reads the page and nothing else.
    const PROBE: &str = "NESSA_APP_HOST_PROBE";

    /// How long a launch waits for the lock before deciding the app is
    /// already running. Nessa probes the lock to list running apps, and a
    /// launch must not lose to that probe.
    const LOCK_TRIES: u32 = 40;
    const LOCK_WAIT: Duration = Duration::from_millis(25);

    /// Run before the page's own scripts, in every frame: WebRTC reaches
    /// peers and STUN servers that no content security policy names, so the
    /// app gets none of it, and cannot put it back.
    const NO_PEER_CONNECTIONS: &str = r#"(() => {
        for (const name of ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel",
            "RTCSessionDescription", "RTCIceCandidate", "RTCRtpSender", "RTCRtpReceiver"]) {
            try { delete window[name] } catch (_) {}
            try {
                Object.defineProperty(window, name, { value: undefined, writable: false, configurable: false })
            } catch (_) {}
        }
    })()"#;

    /// WebView2's own switches: wry's defaults, then no WebRTC over UDP
    /// that no proxy carries, and no DNS lookups ahead of a link.
    #[cfg(windows)]
    const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
        --force-webrtc-ip-handling-policy=disable_non_proxied_udp \
        --webrtc-ip-handling-policy=disable_non_proxied_udp --dns-prefetch-disable";

    pub fn run() -> Result<(), String> {
        let folder = launcher_folder()?;
        let package = Arc::new(read_package(&folder)?);
        let Some(_running) = hold_running(&folder)? else {
            // Already open: on macOS the system brings that one forward, and
            // elsewhere the window already on screen is the app.
            return Ok(());
        };
        let title = package.manifest().app_id().as_str().to_owned();
        // A dynamic app's server runs before its page loads; one that does
        // not start is said in the window, in place of the page.
        let (server, page) = match package.manifest().server() {
            None => (None, Page::Files),
            Some(_) => match start_server(&package, &folder, APP_CLIENT, None) {
                Ok(server) => match server_policy(&server.origin()) {
                    Some(policy) => {
                        let page = Page::WithServer {
                            policy,
                            script: server_script(&server.origin(), server.token()),
                        };
                        (Some(server), page)
                    }
                    None => (
                        None,
                        Page::Failed(
                            "this Nessa cannot let the app's page reach its server".into(),
                        ),
                    ),
                },
                Err(refusal) => (None, Page::Failed(refusal.to_string())),
            },
        };
        let page = Arc::new(page);
        let server = Arc::new(Mutex::new(server));
        let origin = origin();
        let entry = origin
            .join(package.manifest().entrypoint())
            .map_err(|error| format!("app entry: {error}"))?;
        let probe = std::env::var(PROBE).ok();

        let served = package.clone();
        let shown = page.clone();
        let builder = Builder::<Wry>::default()
            .register_uri_scheme_protocol(SCHEME, move |_context, request| {
                respond(&served, &shown, &request)
            })
            .on_page_load(move |webview, payload| {
                if payload.event() != PageLoadEvent::Finished {
                    return;
                }
                if let Some(target) = &probe {
                    report_probe(webview, target);
                }
            });
        #[cfg(target_os = "macos")]
        let builder = {
            let title = title.clone();
            builder.menu(move |app| menu(app, &title))
        };
        let icon = package
            .manifest()
            .icon()
            .filter(|path| path.ends_with(".png"))
            .and_then(|path| package.asset(path))
            .map(<[u8]>::to_vec);
        let webview_data = folder.join("webview");
        builder
            .setup(move |app| {
                let allowed = origin.clone();
                let window = WebviewWindowBuilder::new(
                    app,
                    WINDOW,
                    WebviewUrl::CustomProtocol(entry.clone()),
                )
                .title(&title)
                .inner_size(900.0, 640.0)
                .min_inner_size(320.0, 240.0)
                .center()
                .incognito(true)
                .data_directory(webview_data.clone())
                .initialization_script_for_all_frames(NO_PEER_CONNECTIONS)
                .initialization_script(match page.as_ref() {
                    Page::WithServer { script, .. } => script.as_str(),
                    _ => "",
                })
                .on_navigation(move |next| same_origin(next, &allowed))
                // `window.open` and target=_blank open nothing.
                .on_new_window(|_, _| NewWindowResponse::Deny);
                #[cfg(windows)]
                let window = window.additional_browser_args(BROWSER_ARGS);
                let window = match icon
                    .as_deref()
                    .and_then(|png| tauri::image::Image::from_bytes(png).ok())
                {
                    Some(image) => window.icon(image)?,
                    None => window,
                };
                window.build()?;
                Ok(())
            })
            .build(tauri::generate_context!("app-host/tauri.conf.json"))
            .map_err(|error| format!("could not start the app: {error}"))?
            .run(move |_, event| {
                // The server ends with the app: its stdin closes however
                // this process ends, and here it is also asked to stop.
                if let RunEvent::Exit = event {
                    drop(server.lock().map(|mut held| held.take()));
                }
            });
        Ok(())
    }

    /// What the window shows.
    enum Page {
        /// A static app's files.
        Files,
        /// A dynamic app's files, which may reach its server and nothing else.
        WithServer { policy: String, script: String },
        /// Why a dynamic app's server did not start.
        Failed(String),
    }

    /// The app's policy, with its server the one more place it may connect.
    pub(super) fn server_policy(origin: &str) -> Option<String> {
        const CONNECT: &str = "connect-src 'self' nessa-app:;";
        APP_CONTENT_SECURITY_POLICY.contains(CONNECT).then(|| {
            APP_CONTENT_SECURITY_POLICY
                .replace(CONNECT, &format!("connect-src 'self' nessa-app: {origin};"))
        })
    }

    /// Hands the page its server's address and this launch's token, before
    /// any of its own scripts run; it cannot be changed afterwards.
    pub(super) fn server_script(origin: &str, token: &str) -> String {
        let value = serde_json::json!({ "server": origin, "token": token });
        format!(
            "Object.defineProperty(window, \"nessaApp\", {{ value: Object.freeze({value}), writable: false, configurable: false }})"
        )
    }

    /// The window's page when a dynamic app's server did not start.
    fn failure_page(title: &str, words: &str) -> Vec<u8> {
        let escape = |text: &str| {
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };
        format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>{title}</title>\
             <style>:root{{color-scheme:light dark;font:14px/1.5 system-ui,sans-serif}}\
             body{{margin:0;padding:32px;max-width:560px}}h1{{font-size:18px;margin:0 0 8px}}\
             pre{{white-space:pre-wrap;color:GrayText;font-size:12px}}</style>\
             <h1>{title} could not start</h1><pre>{words}</pre></html>",
            title = escape(title),
            words = escape(words),
        )
        .into_bytes()
    }

    /// The launcher's folder: named with `--app`, or, in a macOS bundle, its
    /// own `Contents/Resources/nessa-app`.
    fn launcher_folder() -> Result<PathBuf, String> {
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--app" {
                return args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "--app names no folder".to_owned());
            }
        }
        let executable = std::env::current_exe().map_err(|error| format!("own path: {error}"))?;
        executable
            .parent()
            .and_then(Path::parent)
            .map(|contents| contents.join("Resources").join("nessa-app"))
            .filter(|folder| folder.join(LAUNCHER_RECORD).is_file())
            .ok_or_else(|| "no app to open: pass --app <folder>".to_owned())
    }

    fn read_package(folder: &Path) -> Result<VerifiedPackage, String> {
        let file = File::open(folder.join(LAUNCHER_RECORD))
            .map_err(|error| format!("read the app: {error}"))?;
        let mut bytes = Vec::new();
        file.take(MAX_LAUNCHER_RECORD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("read the app: {error}"))?;
        read_launcher_record(&bytes)
            .map_err(|refusal| format!("the app does not verify: {refusal:?}"))
    }

    /// Hold the running lock, writing this process's id beside it. None when
    /// another process holds it: the app is already open. The id is written
    /// whole and swapped in, so Stop never reads half of it.
    fn hold_running(folder: &Path) -> Result<Option<File>, String> {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(folder.join(RUNNING_LOCK))
            .map_err(|error| format!("running lock: {error}"))?;
        for _ in 0..LOCK_TRIES {
            match FileExt::try_lock_exclusive(&lock) {
                Ok(()) => {
                    let id = std::process::id();
                    let written = folder.join(format!(".{RUNNING_PID}.{id}"));
                    File::create(&written)
                        .and_then(|mut pid| write!(pid, "{id}"))
                        .and_then(|()| std::fs::rename(&written, folder.join(RUNNING_PID)))
                        .map_err(|error| format!("running pid: {error}"))?;
                    return Ok(Some(lock));
                }
                Err(error) if error.kind() == fs2::lock_contended_error().kind() => {
                    thread::sleep(LOCK_WAIT)
                }
                Err(error) => return Err(format!("running lock: {error}")),
            }
        }
        Ok(None)
    }

    /// The origin the app's files are served from. WebView2 reaches a custom
    /// scheme only as `http://<scheme>.localhost`.
    fn origin() -> Url {
        let origin = if cfg!(windows) {
            format!("http://{SCHEME}.localhost/")
        } else {
            format!("{SCHEME}://localhost/")
        };
        Url::parse(&origin).expect("a fixed origin parses")
    }

    fn same_origin(next: &Url, origin: &Url) -> bool {
        next.scheme() == origin.scheme()
            && next.host_str() == origin.host_str()
            && next.port() == origin.port()
            && next.username().is_empty()
            && next.password().is_none()
    }

    /// The package's file at the request's path, under the app's policy, or
    /// nothing.
    fn respond(
        package: &VerifiedPackage,
        page: &Page,
        request: &Request<Vec<u8>>,
    ) -> Response<Vec<u8>> {
        let uri = request.uri();
        let path = uri.path().strip_prefix('/').unwrap_or_default();
        let path = if path.is_empty() {
            package.manifest().entrypoint()
        } else {
            path
        };
        let host = uri.host().unwrap_or_default();
        let ours = host == "localhost" || host == format!("{SCHEME}.localhost");
        let policy = match page {
            Page::WithServer { policy, .. } => policy.as_str(),
            _ => APP_CONTENT_SECURITY_POLICY,
        };
        let failed = match page {
            Page::Failed(words) if path == package.manifest().entrypoint() => {
                Some(failure_page(package.manifest().app_id().as_str(), words))
            }
            _ => None,
        };
        let bytes = match failed {
            Some(bytes) => Some(bytes),
            None => package.asset(path).map(<[u8]>::to_vec),
        };
        match bytes.filter(|_| ours && request.method() == Method::GET) {
            Some(bytes) => Response::builder()
                .header(header::CONTENT_TYPE, content_type(path))
                .header(header::CACHE_CONTROL, "no-store")
                .header("content-security-policy", policy)
                .header("x-content-type-options", "nosniff")
                .body(bytes),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-security-policy", policy)
                .body(Vec::new()),
        }
        .unwrap_or_else(|_| Response::new(Vec::new()))
    }

    fn report_probe(webview: &tauri::Webview<Wry>, target: &str) {
        let Ok(target) = serde_json::to_string(target) else {
            return;
        };
        let script = format!(
            r#"(() => {{
                let reached = false
                try {{
                    const request = new XMLHttpRequest()
                    request.open("GET", {target}, false)
                    request.send()
                    reached = true
                }} catch (_) {{}}
                let own = false
                try {{
                    const request = new XMLHttpRequest()
                    request.open("GET", location.href, false)
                    request.send()
                    own = request.status === 200
                }} catch (_) {{}}
                const image = new Image()
                image.src = {target} + "?image"
                const rtc = typeof RTCPeerConnection !== "undefined"
                    || typeof webkitRTCPeerConnection !== "undefined"
                let opened = null
                try {{ opened = window.open({target}) }} catch (_) {{}}
                const heading = document.querySelector("h1")
                return {{ title: document.title, heading: heading && heading.textContent, reached, own, rtc, opened: !!opened }}
            }})()"#
        );
        let app = webview.app_handle().clone();
        let Some(window) = app.get_webview_window(WINDOW) else {
            return;
        };
        let reported = window.eval_with_callback(&script, move |value| {
            println!("nessa-app-host probe: {value}");
            app.exit(0);
        });
        if reported.is_err() {
            webview.app_handle().exit(1);
        }
    }

    #[cfg(target_os = "macos")]
    fn menu(app: &tauri::AppHandle, title: &str) -> tauri::Result<tauri::menu::Menu<Wry>> {
        use tauri::menu::{Menu, PredefinedMenuItem, Submenu};
        let quit = format!("Quit {title}");
        let hide = format!("Hide {title}");
        Menu::with_items(
            app,
            &[
                &Submenu::with_items(
                    app,
                    title,
                    true,
                    &[
                        &PredefinedMenuItem::hide(app, Some(&hide))?,
                        &PredefinedMenuItem::hide_others(app, None)?,
                        &PredefinedMenuItem::show_all(app, None)?,
                        &PredefinedMenuItem::separator(app)?,
                        &PredefinedMenuItem::quit(app, Some(&quit))?,
                    ],
                )?,
                &Submenu::with_items(
                    app,
                    "Edit",
                    true,
                    &[
                        &PredefinedMenuItem::undo(app, None)?,
                        &PredefinedMenuItem::redo(app, None)?,
                        &PredefinedMenuItem::separator(app)?,
                        &PredefinedMenuItem::cut(app, None)?,
                        &PredefinedMenuItem::copy(app, None)?,
                        &PredefinedMenuItem::paste(app, None)?,
                        &PredefinedMenuItem::select_all(app, None)?,
                    ],
                )?,
                &Submenu::with_items(
                    app,
                    "Window",
                    true,
                    &[
                        &PredefinedMenuItem::minimize(app, None)?,
                        &PredefinedMenuItem::close_window(app, None)?,
                    ],
                )?,
            ],
        )
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::host::{server_policy, server_script};

    #[test]
    fn a_dynamic_page_may_reach_its_own_server_and_nothing_else() {
        let policy = server_policy("http://127.0.0.1:4100").expect("the policy names connect-src");
        let connect = policy
            .split(';')
            .find(|directive| directive.trim().starts_with("connect-src"))
            .unwrap();
        assert_eq!(
            connect.trim(),
            "connect-src 'self' nessa-app: http://127.0.0.1:4100"
        );
        let script = server_script("http://127.0.0.1:4100", "t\"</script>");
        assert!(script.contains(r#""token":"t\"</script>""#), "{script}");
    }
}
