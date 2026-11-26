//! Owns the two native surfaces and the Stop acknowledgement.

use std::{
    path::Path,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use nessa_app_package::VerifiedPackage;
use tauri::{
    http::{header, Method, Request, Response, StatusCode},
    webview::PageLoadEvent,
    AppHandle, Builder, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent, Wry,
};

use crate::packages::{self, CALENDAR, NOTES};

const SCHEME: &str = "nessa-package-fixture";
const EVENT_WAIT: Duration = Duration::from_secs(12);
const OBSERVE_RENDER: &str = r#"(() => {
    const heading = document.getElementById("fixture-heading");
    if (!heading) return null;
    const rect = heading.getBoundingClientRect();
    return [heading.textContent, rect.width, rect.height > 0];
})()"#;

#[derive(Debug)]
enum Observation {
    Loaded(String),
    Destroyed(String),
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
    let app = Builder::<Wry>::default()
        .register_uri_scheme_protocol(SCHEME, move |context, request| {
            respond(&assets, context.webview_label(), &request)
        })
        .on_page_load(move |webview, payload| {
            if payload.event() == PageLoadEvent::Finished && payload.url().scheme() == SCHEME {
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
                let url = Url::parse(&format!("{SCHEME}://localhost/{}", manifest.entrypoint()))?;
                WebviewWindowBuilder::new(app, label, WebviewUrl::CustomProtocol(url.clone()))
                    .on_navigation(move |next| *next == url)
                    .title(label)
                    .inner_size(480.0, 240.0)
                    .incognito(true)
                    .build()?;
            }
            Ok(())
        })
        .build(tauri::generate_context!(
            "examples/app_package_acceptance/tauri.conf.json"
        ))
        .map_err(|error| format!("build disposable Tauri host: {error}"))?;

    let handle = app.handle().clone();
    let check = thread::spawn(move || {
        let result =
            exercise(&handle, &observed).and_then(|()| packages::reopen(&database, &installed[1]));
        handle.exit(i32::from(result.is_err()));
        result
    });
    let code = app.run_return(move |_handle, event| {
        if let RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } = event
        {
            let _ = events.send(Observation::Destroyed(label));
        }
    });
    check
        .join()
        .map_err(|_| "native package observer panicked".to_owned())??;
    if code != 0 {
        return Err(format!("unexpected native exit code {code}"));
    }
    println!("app_package_acceptance: two installed packages rendered; matching native Stop confirmed; second app and its stored bytes preserved");
    Ok(())
}

fn exercise(handle: &AppHandle, observed: &mpsc::Receiver<Observation>) -> Result<(), String> {
    let mut loaded = [false; 2];
    for _ in 0..loaded.len() {
        match observed.recv_timeout(EVENT_WAIT) {
            Ok(Observation::Loaded(label)) if label == CALENDAR && !loaded[0] => loaded[0] = true,
            Ok(Observation::Loaded(label)) if label == NOTES && !loaded[1] => loaded[1] = true,
            other => return Err(format!("expected both package loads: {other:?}")),
        }
    }
    let calendar = visible_window(handle, CALENDAR)?;
    let notes = visible_window(handle, NOTES)?;
    assert_render(&calendar, ("Calendar from installed bytes", 137, true))?;
    assert_render(&notes, ("Notes from installed bytes", 173, true))?;
    stop(handle, &calendar, observed)?;
    assert_render(
        &visible_window(handle, NOTES)?,
        ("Notes from installed bytes", 173, true),
    )
}

fn stop(
    handle: &AppHandle,
    target: &WebviewWindow,
    observed: &mpsc::Receiver<Observation>,
) -> Result<(), String> {
    let label = target.label();
    // The enqueue receipt is separate from the native event for this window.
    target
        .destroy()
        .map_err(|error| format!("request Stop for {label}: {error}"))?;
    match observed.recv_timeout(EVENT_WAIT) {
        Ok(Observation::Destroyed(destroyed)) if destroyed == label => (),
        other => return Err(format!("Stop awaited destruction of {label}: {other:?}")),
    }
    if handle.get_webview_window(label).is_some() {
        return Err(format!("stopped surface {label} is still registered"));
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

fn assert_render(window: &WebviewWindow, expected: (&str, u32, bool)) -> Result<(), String> {
    let (result, receiver) = mpsc::channel();
    window
        .eval_with_callback(OBSERVE_RENDER, move |value| {
            let _ = result.send(value);
        })
        .map_err(|error| format!("probe {}: {error}", window.label()))?;
    let value = receiver
        .recv_timeout(EVENT_WAIT)
        .map_err(|error| format!("await render of {}: {error}", window.label()))?;
    let expected = serde_json::json!(expected);
    let value: serde_json::Value =
        serde_json::from_str(&value).map_err(|error| format!("decode render evidence: {error}"))?;
    if value != expected {
        return Err(format!(
            "{} rendered {value}, expected {expected}",
            window.label()
        ));
    }
    Ok(())
}

fn respond(
    packages: &[VerifiedPackage],
    label: &str,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let asset = (|| {
        if request.method() != Method::GET || request.uri().query().is_some() {
            return None;
        }
        let path = request.uri().path().strip_prefix('/')?;
        let package = packages
            .iter()
            .find(|package| package.manifest().app_id().as_str() == label)?;
        let mime = if path == package.manifest().entrypoint() {
            "text/html; charset=utf-8"
        } else if path.ends_with(".css") {
            "text/css; charset=utf-8"
        } else {
            return None;
        };
        Some((mime, package.asset(path)?))
    })();
    if let Some((mime, bytes)) = asset {
        Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-store")
            .header(
                "content-security-policy",
                format!("default-src 'none'; style-src {SCHEME}:"),
            )
            .body(bytes.to_vec())
            .expect("fixed fixture response headers")
    } else {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .expect("fixed fixture missing response")
    }
}
