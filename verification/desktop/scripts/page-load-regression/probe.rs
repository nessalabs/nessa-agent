#[cfg(test)]
mod synthetic_native_boundary {
    use super::*;
    use crate::{AppHandle, Build, Config, Window};
    use std::sync::{Arc, Mutex};
    fn app(native_url: Option<&str>, refuse: bool) -> (AppHandle, Arc<Mutex<Vec<Url>>>) {
        let navigated = Arc::new(Mutex::new(Vec::new()));
        let app = AppHandle {
            config: Arc::new(Config {
                build: Build {
                    dev_url: Some(Url::parse("http://localhost:1498/desktop.html").unwrap()),
                },
            }),
            windows: Arc::new(vec![Window {
                label: "desktop",
                url: native_url.map(|value| Url::parse(value).unwrap()),
                navigated: navigated.clone(),
                refuse,
            }]),
        };
        (app, navigated)
    }
    fn diagnose(app: &AppHandle, loads: &Loads) {
        explain(app, loads/* WINDOW_ARGUMENT */);
    }
    #[test]
    fn nil_native_url_produces_an_explanation_with_the_configured_page() {
        let (app, navigated) = app(None, false);
        diagnose(&app, &Loads::new());
        let urls = navigated.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(is_load_failure_page(&urls[0], true));
        assert_eq!(
            urls[0]
                .query_pairs()
                .find(|(key, _)| key == "page")
                .unwrap()
                .1,
            "http://localhost:1498/desktop.html"
        );
    }
    #[test]
    fn a_failure_page_and_an_app_page_are_preserved() {
        for page in [
            "nessa-status://localhost/load-failure?page=first",
            "http://localhost:1420/desktop.html",
        ] {
            let (app, navigated) = app(Some(page), false);
            let loads = Loads::new();
            loads.observe("desktop", page);
            diagnose(&app, &loads);
            assert!(navigated.lock().unwrap().is_empty());
        }
    }
    #[test]
    fn navigation_failure_remains_survivable_when_native_url_is_nil() {
        let (app, navigated) = app(None, true);
        diagnose(&app, &Loads::new());
        assert!(navigated.lock().unwrap().is_empty());
    }
}
