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

    /// `LINGER_SHOWN` in `src/onboarding/model/linger.ts` is the list the shell
    /// accepts. A tag added or renamed on only one side fails here.
    #[test]
    fn every_shown_tag_matches_the_published_vocabulary() {
        let published = published_shown_tags();
        let wired = [
            LingerShown::NotApplicable,
            LingerShown::Offer,
            LingerShown::Enabled,
            LingerShown::Refused,
            LingerShown::Failed,
            LingerShown::Unsupported,
        ]
        .map(wire_tag);
        assert_eq!(wired.as_slice(), published.as_slice());
    }

    fn wire_tag(shown: LingerShown) -> String {
        serde_json::to_value(response(shown))
            .unwrap()
            .get("shown")
            .and_then(|tag| tag.as_str())
            .expect("shown tag")
            .to_string()
    }

    fn published_shown_tags() -> Vec<String> {
        let source = include_str!("../../../../src/onboarding/model/linger.ts");
        let marker = "export const LINGER_SHOWN = [";
        let start = source.find(marker).expect("LINGER_SHOWN");
        let body = &source[start + marker.len()..];
        let end = body.find("] as const").expect("LINGER_SHOWN close");
        body[..end]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let line = line.strip_suffix(',').unwrap_or(line);
                line.strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .expect("quoted tag")
                    .to_string()
            })
            .collect()
    }
}
