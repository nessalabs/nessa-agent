//! A real Wry runtime, with only the diagnostic's dependencies assembled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use tauri::plugin::Builder as PluginBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{App, Builder as AppBuilder, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, Wry};

use crate::navigation::{self, Navigation, Serving};
use crate::probe::{self, Observation, EXISTING, MISSING, NORMAL, WINDOWS};
use crate::server::{Server, CONTROL_URL};
use crate::{host_log, page_load};

pub fn run() -> Result<(), String> {
    if navigation::serving() != Serving::DevServer {
        return Err("run this fixture with --no-default-features (the real dev deadline)".into());
    }
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    host_log::install(Some(directory.path()));
    let server = Server::start()?;
    let (sender, receiver) = mpsc::channel();
    let policy_events = sender.clone();
    let pages = sender.clone();
    let setup_events = sender.clone();
    let loads = page_load::Loads::new();
    let observed = loads.clone();
    let app = AppBuilder::<Wry>::default()
        .plugin(
            PluginBuilder::<Wry>::new("load-fixture-navigation")
                .on_navigation(move |webview, url| {
                    let decision = navigation::decide(url, navigation::serving());
                    if decision == Navigation::HandToBrowser {
                        // Record the real decision without opening an OS browser.
                        let _ = policy_events.send(Observation::BrowserDecision(
                            webview.label().to_owned(),
                            url.to_string(),
                        ));
                    }
                    decision == Navigation::Allow
                })
                .build(),
        )
        .register_uri_scheme_protocol(page_load::SCHEME, |_context, request| {
            page_load::respond(
                request.method(),
                request.uri().path(),
                request.uri().query(),
            )
        })
        .on_page_load(move |webview, payload| {
            let label = webview.label().to_owned();
            let url = payload.url().to_string();
            match payload.event() {
                PageLoadEvent::Started => {
                    observed.observe(&label, &url);
                    let _ = pages.send(Observation::Started(label, url));
                }
                PageLoadEvent::Finished => {
                    let _ = pages.send(Observation::Finished(label, url));
                }
            }
        })
        .setup(move |app| {
            // Report setup failure rather than unwinding through Cocoa.
            if let Err(error) = windows(app) {
                let _ = setup_events.send(Observation::SetupFailed(error));
                app.handle().exit(1);
                return Ok(());
            }
            let _ = setup_events.send(Observation::Watching(Instant::now()));
            page_load::watch(app.handle().clone(), loads, WINDOWS);
            Ok(())
        })
        .build(tauri::generate_context!(
            "examples/page_load_acceptance/tauri.conf.json"
        ))
        .map_err(|error| format!("build disposable Tauri app: {error}"))?;

    let completed = Arc::new(AtomicBool::new(false));
    let stopped = completed.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(20));
        if !stopped.load(Ordering::Acquire) {
            eprintln!(
                "::error title=Native load acceptance::native event loop did not settle in 20s"
            );
            std::process::exit(1);
        }
    });
    let handle = app.handle().clone();
    let log = directory.path().join("host.log");
    let served = server.served.clone();
    let check = thread::spawn(move || {
        let result = probe::check(&handle, receiver, served, &log);
        handle.exit(if result.is_ok() { 0 } else { 1 });
        result
    });
    let code = app.run_return(move |_handle, event| {
        if matches!(event, RunEvent::Ready) {
            let _ = sender.send(Observation::Ready);
        }
    });
    let result = check
        .join()
        .map_err(|_| "native probe thread panicked".to_string())?;
    completed.store(true, Ordering::Release);
    result?;
    if code != 0 {
        return Err(format!("Tauri returned unexpected exit code {code}"));
    }
    println!("page_load_acceptance: real Tauri nil-URL/deadline/diagnostic/1420/failure-preservation passed");
    Ok(())
}

fn windows(app: &App) -> Result<(), String> {
    let configured = app
        .config()
        .build
        .dev_url
        .clone()
        .ok_or("fixture dev URL is absent")?;
    if configured.port() != Some(1498) {
        return Err("fixture must retain the refused 1498 origin".into());
    }
    for (label, url) in [
        (MISSING, configured),
        (
            NORMAL,
            Url::parse(CONTROL_URL).map_err(|error| error.to_string())?,
        ),
        (EXISTING, probe::existing_url()),
    ] {
        WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
            .title(format!("Nessa load fixture: {label}"))
            .inner_size(400.0, 180.0)
            .incognito(true)
            .build()
            .map_err(|error| format!("create {label}: {error}"))?;
    }
    Ok(())
}
