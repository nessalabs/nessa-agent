//! Observe native views and callbacks; never publish substitute load facts.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2_web_kit::WKWebView;
use tauri::{AppHandle, Manager, Url};

use crate::server::CONTROL_URL;
use crate::{host_log, host_refusal, page_load};

pub const MISSING: &str = "missing";
pub const NORMAL: &str = "normal";
pub const EXISTING: &str = "existing";
pub const WINDOWS: &[&str] = &[MISSING, NORMAL, EXISTING];

pub enum Observation {
    Ready,
    Watching(Instant),
    SetupFailed(String),
    BrowserDecision(String, String),
    Started(String, String),
    Finished(String, String),
}

pub fn existing_url() -> Url {
    page_load::load_failure_url("http://localhost:1420/original.html?cause=retained#page")
}

pub fn check(
    app: &AppHandle,
    events: Receiver<Observation>,
    served: Arc<AtomicBool>,
    log: &Path,
) -> Result<(), String> {
    let limit = Instant::now() + Duration::from_secs(15);
    let configured = app
        .config()
        .build
        .dev_url
        .as_ref()
        .ok_or("fixture dev URL is absent")?
        .to_string();
    let diagnostic = page_load::load_failure_url(&configured).to_string();
    let existing = existing_url().to_string();
    let mut watching = None;
    let mut ready = false;
    let mut denied = false;
    let mut nil_checked = false;
    let mut normal_finished = false;
    let mut existing_finished = false;
    let mut missing_finished = false;
    let mut normal_starts = 0;
    let mut existing_starts = 0;
    loop {
        let remaining = limit.saturating_duration_since(Instant::now());
        let event = events
            .recv_timeout(remaining)
            .map_err(|error| format!("waiting for real native callbacks: {error}"))?;
        match event {
            Observation::SetupFailed(error) => return Err(error),
            Observation::Ready => ready = true,
            Observation::Watching(at) => watching = Some(at),
            Observation::BrowserDecision(label, url) => {
                if label == MISSING && url == configured {
                    denied = true;
                }
            }
            Observation::Started(label, url) => {
                if label == NORMAL && url == CONTROL_URL {
                    normal_starts += 1;
                }
                if label == EXISTING && url == existing {
                    existing_starts += 1;
                }
            }
            Observation::Finished(label, url) => {
                if label == NORMAL && url == CONTROL_URL {
                    normal_finished = true;
                }
                if label == EXISTING && url == existing {
                    existing_finished = true;
                }
                if label == MISSING && url == diagnostic {
                    let since = watching.ok_or("deadline start was not observed")?.elapsed();
                    if since < Duration::from_secs(4) {
                        return Err(format!(
                            "diagnostic arrived before its real deadline: {since:?}"
                        ));
                    }
                    println!("page_load_acceptance: real diagnostic finished after {since:?}");
                    missing_finished = true;
                }
            }
        }
        if ready && denied && !nil_checked {
            if watching.ok_or("deadline start was not observed")?.elapsed()
                >= Duration::from_secs(4)
            {
                return Err("could not establish native nil URL before the deadline".into());
            }
            if native_url(app, MISSING)?.is_some() {
                return Err("refused initial navigation did not leave WKWebView.URL nil".into());
            }
            nil_checked = true;
            println!("page_load_acceptance: policy handed 1498 out; real WKWebView.URL=nil");
        }
        if nil_checked && missing_finished && normal_finished && existing_finished {
            expect_url(app, MISSING, &diagnostic)?;
            expect_url(app, NORMAL, CONTROL_URL)?;
            expect_url(app, EXISTING, &existing)?;
            if !served.load(Ordering::Acquire) {
                return Err(
                    "normal-origin control did not reach the real 1420 HTTP fixture".into(),
                );
            }
            if normal_starts != 1 || existing_starts != 1 {
                return Err(format!(
                    "preserved windows navigated again: normal={normal_starts}, existing={existing_starts}"
                ));
            }
            host_log::flush();
            let logged = std::fs::read_to_string(log).map_err(|error| error.to_string())?;
            let expected = host_refusal::fill("document-unserved", &[("page", &configured)]);
            if !logged.contains(&expected) {
                return Err(
                    "configured diagnostic URL was not retained in the real host log".into(),
                );
            }
            println!("page_load_acceptance: configured URL logged; real 1420 page and original failure query preserved");
            return Ok(());
        }
    }
}

fn expect_url(app: &AppHandle, label: &str, expected: &str) -> Result<(), String> {
    let found = native_url(app, label)?;
    if found.as_deref() != Some(expected) {
        return Err(format!("{label}: expected {expected}, found {found:?}"));
    }
    Ok(())
}

fn native_url(app: &AppHandle, label: &str) -> Result<Option<String>, String> {
    let view = app
        .get_webview_window(label)
        .ok_or_else(|| format!("native window {label} disappeared"))?;
    let (sender, receiver) = mpsc::channel();
    view.with_webview(move |platform| {
        // The closure runs on Tauri's owning main thread. The typed getter
        // represents nil explicitly, unlike the Wry accessor under regression.
        let result = unsafe { platform.inner().cast::<WKWebView>().as_ref() }
            .ok_or_else(|| "Tauri returned a null WKWebView pointer".to_string())
            .and_then(|webview| match unsafe { webview.URL() } {
                None => Ok(None),
                Some(url) => url
                    .absoluteString()
                    .map(|text| Some(text.to_string()))
                    .ok_or_else(|| "native URL has no absolute string".to_string()),
            });
        let _ = sender.send(result);
    })
    .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|error| format!("native URL callback: {error}"))?
}
