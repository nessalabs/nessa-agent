//! Setup's linger commands. The JSON is what the screen is allowed to say.
//!
//! The calls block on logind, so they run on the blocking pool. The window
//! thread only waits for the join.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::composition::HostDependencies;
use crate::linger::{application::LingerOffer, domain::LingerShown};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ShownWire {
    #[cfg(any(test, not(target_os = "linux")))]
    NotApplicable,
    Offer,
    Enabled,
    Refused,
    Failed,
    Unsupported,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LingerResponse {
    shown: ShownWire,
}

fn response(shown: LingerShown) -> LingerResponse {
    LingerResponse {
        shown: match shown {
            #[cfg(any(test, not(target_os = "linux")))]
            LingerShown::NotApplicable => ShownWire::NotApplicable,
            LingerShown::Offer => ShownWire::Offer,
            LingerShown::Enabled => ShownWire::Enabled,
            LingerShown::Refused => ShownWire::Refused,
            LingerShown::Failed => ShownWire::Failed,
            LingerShown::Unsupported => ShownWire::Unsupported,
        },
    }
}

async fn ask(
    offer: Arc<dyn LingerOffer>,
    call: impl FnOnce(&dyn LingerOffer) -> LingerShown + Send + 'static,
) -> Result<LingerResponse, String> {
    let shown = tauri::async_runtime::spawn_blocking(move || call(offer.as_ref()))
        .await
        .map_err(|error| format!("linger task stopped: {error}"))?;
    Ok(response(shown))
}

#[tauri::command]
pub(crate) async fn linger_status(
    deps: State<'_, HostDependencies>,
) -> Result<LingerResponse, String> {
    let offer = Arc::clone(&deps.linger);
    ask(offer, |offer| offer.status()).await
}

#[tauri::command]
pub(crate) async fn linger_accept(
    deps: State<'_, HostDependencies>,
) -> Result<LingerResponse, String> {
    let offer = Arc::clone(&deps.linger);
    ask(offer, |offer| offer.accept()).await
}

#[cfg(test)]
mod tests {
    use super::response;
    use crate::linger::domain::LingerShown;

    #[test]
    fn the_wire_is_the_shown_tag() {
        let json = serde_json::to_string(&response(LingerShown::Offer)).unwrap();
        assert_eq!(json, r#"{"shown":"offer"}"#);
        let failed = serde_json::to_value(response(LingerShown::Failed)).unwrap();
        assert_eq!(failed["shown"], "failed");
    }
}
