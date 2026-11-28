//! `nessa-app-host`: one installed app, run as an app of its own.
//!
//! ```text
//! launcher (macOS: <app>.app in ~/Applications/Nessa Apps; elsewhere a folder
//!   Nessa keeps, with a menu entry on Linux) ── runs ──▶ nessa-app-host
//!     read <launcher>/app.json ── verify every byte (nessa-app-package)
//!     lock <launcher>/running.lock (held = running; a second launch leaves)
//!     one window: the app's own files under nessa-app://, nothing else
//! ```
//!
//! Each launcher holds its own copy of this binary, so each app is its own
//! process with its own Dock or taskbar entry and icon, and runs with Nessa's
//! main window, or Nessa itself, closed. It reads no gateway credential and
//! registers no command: its page can load the package's files and nothing
//! more (`APP_CONTENT_SECURITY_POLICY`), and may navigate nowhere else.
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
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use fs2::FileExt;
    use nessa_app_package::{
        content_type, read_launcher_record, VerifiedPackage, APP_CONTENT_SECURITY_POLICY,
        LAUNCHER_RECORD, MAX_LAUNCHER_RECORD_BYTES, RUNNING_LOCK, RUNNING_PID,
    };
    use tauri::http::{header, Method, Request, Response, StatusCode};
    use tauri::webview::{NewWindowResponse, PageLoadEvent};
    use tauri::{Builder, Manager, Url, WebviewUrl, WebviewWindowBuilder, Wry};

    const SCHEME: &str = "nessa-app";
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
        let origin = origin();
        let entry = origin
            .join(package.manifest().entrypoint())
            .map_err(|error| format!("app entry: {error}"))?;
        let probe = std::env::var(PROBE).ok();

        let served = package.clone();
        let builder = Builder::<Wry>::default()
            .register_uri_scheme_protocol(SCHEME, move |_context, request| {
                respond(&served, &request)
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
            .run(|_, _| {});
        Ok(())
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
    fn respond(package: &VerifiedPackage, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        let uri = request.uri();
        let path = uri.path().strip_prefix('/').unwrap_or_default();
        let path = if path.is_empty() {
            package.manifest().entrypoint()
        } else {
            path
        };
        let host = uri.host().unwrap_or_default();
        let ours = host == "localhost" || host == format!("{SCHEME}.localhost");
        match package
            .asset(path)
            .filter(|_| ours && request.method() == Method::GET)
        {
            Some(bytes) => Response::builder()
                .header(header::CONTENT_TYPE, content_type(path))
                .header(header::CACHE_CONTROL, "no-store")
                .header("content-security-policy", APP_CONTENT_SECURITY_POLICY)
                .header("x-content-type-options", "nosniff")
                .body(bytes.to_vec()),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-security-policy", APP_CONTENT_SECURITY_POLICY)
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
