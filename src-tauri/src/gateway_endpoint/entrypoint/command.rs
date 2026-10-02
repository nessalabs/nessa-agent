use crate::{
    composition::HostDependencies,
    gateway::{application::Gateway, infrastructure::bundled_window},
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
    let surface = bundled_window(label)?;
    if let Some(gateway) = gateway {
        gateway
            .wait_ready(surface)
            .await
            .map_err(|error| error.to_string())?;
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
}
