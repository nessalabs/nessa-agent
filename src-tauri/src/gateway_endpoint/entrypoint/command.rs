use crate::{
    composition::HostDependencies,
    gateway::{application::Gateway, infrastructure::GatewayReader},
    gateway_endpoint::application::GatewayEndpointAccess,
};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

async fn load_for(
    label: &str,
    gateway: Option<&Gateway>,
    endpoint: Arc<GatewayEndpointAccess>,
    stage: &str,
) -> Result<Option<String>, String> {
    let reader = GatewayReader::of_window(label)?;
    if let Some(gateway) = gateway {
        reader.ready(gateway).await?;
    }
    let stage = stage.to_owned();
    tauri::async_runtime::spawn_blocking(move || endpoint.resolve(&stage))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn load_gateway_endpoint(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    stage: String,
) -> Result<Option<String>, String> {
    let gateway = deps.gateway.clone();
    let endpoint = Arc::clone(&deps.endpoint);
    load_for(window.label(), gateway.as_deref(), endpoint, &stage).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_window::DESKTOP_WINDOW;
    use crate::gateway::application::testing::{reconciled_registration, recording_gateway};
    use crate::panel;
    use nessa_gateway_endpoint::application::EndpointDiscovery;
    use nessa_gateway_endpoint::domain::{EndpointIdentity, GatewayEndpoint};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Discovery {
        calls: AtomicUsize,
    }
    impl EndpointDiscovery for Discovery {
        fn discover(&self) -> std::io::Result<Option<GatewayEndpoint>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(
                GatewayEndpoint::new(
                    "ws://127.0.0.1:9137".into(),
                    EndpointIdentity::new("5485b918-1eeb-4a4a-ad1d-9fdc70dfa231".into(), 4711)
                        .unwrap(),
                )
                .unwrap(),
            ))
        }
    }

    #[test]
    fn setup_can_discover_its_gateway_but_other_windows_and_stages_cannot() {
        let discovery = Arc::new(Discovery {
            calls: AtomicUsize::new(0),
        });
        let endpoint = Arc::new(GatewayEndpointAccess::new("ci".into(), discovery.clone()));
        assert_eq!(
            tauri::async_runtime::block_on(load_for(
                panel::SETUP_WINDOW,
                None,
                endpoint.clone(),
                "ci"
            ))
            .unwrap(),
            Some("ws://127.0.0.1:9137".into())
        );
        assert_eq!(discovery.calls.load(Ordering::SeqCst), 1);
        for (label, stage) in [("untrusted", "ci"), (panel::SETUP_WINDOW, "dev")] {
            assert!(
                tauri::async_runtime::block_on(load_for(label, None, endpoint.clone(), stage))
                    .is_err()
            );
            assert_eq!(discovery.calls.load(Ordering::SeqCst), 1);
        }
    }

    /// The desktop window finds a gateway startup brought up, once it is
    /// ready, and its asking starts nothing (H4′, #419).
    #[test]
    fn the_desktop_window_discovers_a_ready_gateway_and_starts_none() {
        let discovery = Arc::new(Discovery {
            calls: AtomicUsize::new(0),
        });
        let endpoint = Arc::new(GatewayEndpointAccess::new("ci".into(), discovery.clone()));
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));

        let refused = tauri::async_runtime::block_on(load_for(
            DESKTOP_WINDOW,
            Some(&gateway),
            endpoint.clone(),
            "ci",
        ));
        assert_eq!(refused, Err("The local server isn't ready yet".to_string()));
        assert_eq!(discovery.calls.load(Ordering::SeqCst), 0);
        assert_eq!(*host.registrations.lock().unwrap(), 0);

        tauri::async_runtime::block_on(load_for(
            panel::MAIN_WINDOW,
            Some(&gateway),
            endpoint.clone(),
            "ci",
        ))
        .unwrap();
        let registered = *host.registrations.lock().unwrap();
        assert_eq!(
            tauri::async_runtime::block_on(load_for(
                DESKTOP_WINDOW,
                Some(&gateway),
                endpoint,
                "ci"
            ))
            .unwrap(),
            Some("ws://127.0.0.1:9137".into())
        );
        assert_eq!(*host.registrations.lock().unwrap(), registered);
    }
}
