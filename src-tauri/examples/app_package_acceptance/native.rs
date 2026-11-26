//! Owns the two native surfaces and the Stop acknowledgement.

use std::{
    path::Path,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use gtk::prelude::WidgetExt;
use nessa_app_package::VerifiedPackage;
use tauri::{
    http::{header, Method, Request, Response, StatusCode},
    webview::PageLoadEvent,
    AppHandle, Builder, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent, Wry,
};

use crate::packages::{self, CALENDAR, NOTES};
use crate::stop_phase::StopPhase;

const SCHEME: &str = "nessa-package-fixture";
const EVENT_WAIT: Duration = Duration::from_secs(12);
const OBSERVE_RENDER: &str = r#"(() => {
    const heading = document.getElementById("fixture-heading");
    if (!heading) return null;
    const rect = heading.getBoundingClientRect();
    return [heading.textContent, rect.width, rect.height > 0];
})()"#;

#[derive(Clone, Copy, Debug, Default)]
struct ServedAssets {
    document: bool,
    stylesheet: bool,
}

#[derive(Debug, Default)]
struct RouteReceipts {
    calendar: ServedAssets,
    notes: ServedAssets,
}

impl RouteReceipts {
    fn for_window_mut(&mut self, label: &str) -> Option<&mut ServedAssets> {
        match label {
            CALENDAR => Some(&mut self.calendar),
            NOTES => Some(&mut self.notes),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum Observation {
    Loaded(String),
    Stopped,
    UnexpectedClose(String),
}

pub fn run(directory: &Path) -> Result<(), String> {
    let private = directory.join("installed");
    nessa_local_storage::create_directory_exclusive(&private)
        .map_err(|error| format!("create fixture store directory: {error}"))?;
    let database = private.join("apps.sqlite3");
    let installed = Arc::new(packages::install(&database)?);
    let assets = installed.clone();
    let surfaces = installed.clone();
    let (events, observed) = mpsc::channel();
    let pages = events.clone();
    let receipts = Arc::new(Mutex::new(RouteReceipts::default()));
    let route_receipts = receipts.clone();
    let phase = Arc::new(Mutex::new(StopPhase::default()));
    let native_phase = phase.clone();
    let loop_phase = phase.clone();
    let app = Builder::<Wry>::default()
        .register_uri_scheme_protocol(SCHEME, move |context, request| {
            respond(
                &assets[..],
                context.webview_label(),
                &request,
                &route_receipts,
            )
        })
        .on_page_load(move |webview, payload| {
            let url = payload.url();
            if payload.event() == PageLoadEvent::Finished
                && url.scheme() == SCHEME
                && url.host_str() == Some("localhost")
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path().strip_prefix('/') == Some(packages::ENTRYPOINT)
                && url.query().is_none()
                && url.fragment().is_none()
            {
                let _ = pages.send(Observation::Loaded(webview.label().to_owned()));
            }
        })
        .setup(move |app| {
            if !app.webview_windows().is_empty() {
                return Err("fixture must start with the main window closed".into());
            }
            for package in surfaces.iter() {
                let manifest = package.manifest();
                let label = manifest.app_id().as_str();
                // The Tauri runtime deduplicates schemes by data directory,
                // but GTK Wry creates a new ephemeral context for each
                // incognito view. Distinct disposable keys register both.
                let context_key = private.join(label);
                nessa_local_storage::create_directory_exclusive(&context_key).map_err(|error| {
                    format!("create fixture webview context for {label}: {error}")
                })?;
                let url = Url::parse(&format!("{SCHEME}://localhost/{}", manifest.entrypoint()))?;
                let window =
                    WebviewWindowBuilder::new(app, label, WebviewUrl::CustomProtocol(url.clone()))
                        .on_navigation(move |next| *next == url)
                        .title(label)
                        .inner_size(480.0, 240.0)
                        .data_directory(context_key)
                        .incognito(true)
                        .build()?;
                if label == CALENDAR {
                    // GTK emits this before the queued Tauri Destroyed event.
                    let native_phase = native_phase.clone();
                    window.gtk_window()?.connect_destroy(move |_| {
                        native_phase.lock().unwrap().native_destroyed();
                    });
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!(
            "examples/app_package_acceptance/tauri.conf.json"
        ))
        .map_err(|error| format!("build disposable Tauri host: {error}"))?;

    let handle = app.handle().clone();
    let check = thread::spawn(move || {
        let result = exercise(&handle, &observed, &database, &installed, &receipts, phase);
        handle.exit(i32::from(result.is_err()));
        result
    });
    let code = app.run_return(move |_handle, event| {
        if let RunEvent::WindowEvent { label, event, .. } = event {
            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    if label == CALENDAR {
                        loop_phase.lock().unwrap().invalidate();
                    }
                    let _ = events.send(Observation::UnexpectedClose(label));
                }
                WindowEvent::Destroyed => {
                    let observation = if label == CALENDAR && loop_phase.lock().unwrap().confirmed()
                    {
                        Observation::Stopped
                    } else {
                        Observation::UnexpectedClose(label)
                    };
                    let _ = events.send(observation);
                }
                _ => (),
            }
        }
    });
    check
        .join()
        .map_err(|_| "native package observer panicked".to_owned())??;
    if code != 0 {
        return Err(format!("unexpected native exit code {code}"));
    }
    println!("app_package_acceptance: fixture-calendar rendered at 137px, stopped and uninstalled; fixture-notes still renders at 173px from unchanged stored bytes");
    Ok(())
}

fn exercise(
    handle: &AppHandle,
    observed: &mpsc::Receiver<Observation>,
    database: &Path,
    installed: &[VerifiedPackage; 2],
    receipts: &Mutex<RouteReceipts>,
    phase: Arc<Mutex<StopPhase>>,
) -> Result<(), String> {
    let mut loaded = [false; 2];
    for _ in 0..loaded.len() {
        match observed.recv_timeout(EVENT_WAIT) {
            Ok(Observation::Loaded(label)) if label == CALENDAR && !loaded[0] => loaded[0] = true,
            Ok(Observation::Loaded(label)) if label == NOTES && !loaded[1] => loaded[1] = true,
            other => {
                let routes = receipts.lock().unwrap();
                return Err(format!(
                    "initial navigation: expected both package loads, got {other:?}; served={routes:?}"
                ));
            }
        }
    }
    let calendar = visible_window(handle, CALENDAR)?;
    let notes = visible_window(handle, NOTES)?;
    {
        let routes = receipts.lock().unwrap();
        for (label, served) in [(CALENDAR, routes.calendar), (NOTES, routes.notes)] {
            if !served.document || !served.stylesheet {
                return Err(format!(
                    "initial route {label}: expected held HTML and CSS, got {served:?}"
                ));
            }
            println!("app_package_acceptance: initial route {label}: held HTML and CSS served");
        }
    }
    assert_render(
        "initial render",
        &calendar,
        ("Calendar from installed bytes", 137, true),
    )?;
    assert_render(
        "initial render",
        &notes,
        ("Notes from installed bytes", 173, true),
    )?;
    stop(handle, observed, phase)?;
    println!("app_package_acceptance: Stop confirmed exact {CALENDAR} destruction");
    let [target, sibling] = installed;
    packages::uninstall(database, target, sibling)
        .map_err(|error| format!("uninstall/reopen: {error}"))?;
    println!(
        "app_package_acceptance: uninstall/reopen confirmed {CALENDAR} absent and {NOTES} bytes unchanged"
    );
    let sibling =
        visible_window(handle, NOTES).map_err(|error| format!("post-uninstall: {error}"))?;
    assert_render(
        "post-uninstall render",
        &sibling,
        ("Notes from installed bytes", 173, true),
    )
}

fn stop(
    handle: &AppHandle,
    observed: &mpsc::Receiver<Observation>,
    phase: Arc<Mutex<StopPhase>>,
) -> Result<(), String> {
    let app = handle.clone();
    let (admitted, admission) = mpsc::channel();
    handle
        .run_on_main_thread(move || {
            let result = (|| {
                // The fixture creates this label once. Both the lookup and
                // arming run on the loop that receives its GTK destruction.
                let target = visible_window(&app, CALENDAR)?;
                if !phase.lock().unwrap().arm() {
                    return Err(format!(
                        "surface {CALENDAR} was already closed or destroyed"
                    ));
                }
                target.destroy().map_err(|error| {
                    phase.lock().unwrap().invalidate();
                    format!("request Stop for {CALENDAR}: {error}")
                })
            })();
            let _ = admitted.send(result);
        })
        .map_err(|error| format!("schedule Stop for {CALENDAR}: {error}"))?;
    admission
        .recv_timeout(EVENT_WAIT)
        .map_err(|error| format!("await Stop admission for {CALENDAR}: {error}"))??;
    match observed.recv_timeout(EVENT_WAIT) {
        Ok(Observation::Stopped) => (),
        Ok(Observation::UnexpectedClose(label)) => {
            return Err(format!(
                "unexpected close of {label} during Stop for {CALENDAR}"
            ));
        }
        other => return Err(format!("Stop awaited destruction of {CALENDAR}: {other:?}")),
    }
    if handle.get_webview_window(CALENDAR).is_some() {
        return Err(format!("stopped surface {CALENDAR} is still registered"));
    }
    Ok(())
}

fn visible_window(handle: &AppHandle, label: &str) -> Result<WebviewWindow, String> {
    let window = handle
        .get_webview_window(label)
        .ok_or_else(|| format!("missing native surface {label}"))?;
    if !window.is_visible().map_err(|error| error.to_string())? {
        return Err(format!("native surface {label} is hidden"));
    }
    Ok(window)
}

fn assert_render(
    phase: &str,
    window: &WebviewWindow,
    expected: (&str, u32, bool),
) -> Result<(), String> {
    let label = window.label();
    let (result, receiver) = mpsc::channel();
    window
        .eval_with_callback(OBSERVE_RENDER, move |value| {
            let _ = result.send(value);
        })
        .map_err(|error| format!("{phase} {label}: schedule DOM probe: {error}"))?;
    let value = receiver
        .recv_timeout(EVENT_WAIT)
        .map_err(|error| format!("{phase} {label}: await DOM probe: {error}"))?;
    let expected = serde_json::json!(expected);
    let value: serde_json::Value = serde_json::from_str(&value)
        .map_err(|error| format!("{phase} {label}: decode DOM probe: {error}"))?;
    if value != expected {
        return Err(format!(
            "{phase} {label}: rendered {value}, expected {expected}"
        ));
    }
    println!("app_package_acceptance: {phase} {label}: exact DOM and stylesheet layout verified");
    Ok(())
}

fn respond(
    packages: &[VerifiedPackage],
    label: &str,
    request: &Request<Vec<u8>>,
    receipts: &Mutex<RouteReceipts>,
) -> Response<Vec<u8>> {
    let asset = (|| {
        let uri = request.uri();
        if request.method() != Method::GET
            || uri.scheme_str() != Some(SCHEME)
            || uri.authority().map(|authority| authority.as_str()) != Some("localhost")
            || uri.query().is_some()
        {
            return None;
        }
        let path = uri.path().strip_prefix('/')?;
        let package = packages
            .iter()
            .find(|package| package.manifest().app_id().as_str() == label)?;
        let document = path == package.manifest().entrypoint();
        let mime = if document {
            "text/html; charset=utf-8"
        } else if path == packages::STYLESHEET {
            "text/css; charset=utf-8"
        } else {
            return None;
        };
        Some((document, mime, package.asset(path)?))
    })();
    if let Some((document, mime, bytes)) = asset {
        let response = Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-store")
            .header(
                "content-security-policy",
                format!("default-src 'none'; style-src {SCHEME}:"),
            )
            .body(bytes.to_vec())
            .expect("fixed fixture response headers");
        if let Some(served) = receipts.lock().unwrap().for_window_mut(label) {
            if document {
                served.document = true;
            } else {
                served.stylesheet = true;
            }
        }
        response
    } else {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .expect("fixed fixture missing response")
    }
}
