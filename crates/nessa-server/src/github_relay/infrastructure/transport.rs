use crate::{
    env::Stage,
    github_relay::{
        application::{RelayLiveSource, RelayNotifications, RelaySource},
        domain::{Binding, Event, Failure, Page},
    },
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio_tungstenite::{tungstenite::client::IntoClientRequest, MaybeTlsStream, WebSocketStream};
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayConfiguration {
    pub origin: String,
    pub app_slug: String,
    pub app_id: u64,
    pub client_id: String,
}
/// The same compiled public table is used by every gateway. The packaged host
/// supplies its embedded stage; no runtime URL override or stage fallback exists.
pub fn relay_configuration(stage: Stage) -> Result<RelayConfiguration, Failure> {
    let table: std::collections::BTreeMap<String, Option<RelayConfiguration>> =
        serde_json::from_str(include_str!(
            "../../../../../protocol/defaults/github-relays.json"
        ))
        .map_err(|_| Failure::UnconfiguredStage)?;
    table
        .get(stage.as_str())
        .and_then(Clone::clone)
        .ok_or(Failure::UnconfiguredStage)
}
pub struct HttpRelaySource {
    http: reqwest::Client,
    configuration: RelayConfiguration,
    stage: Stage,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEvent {
    sequence: i64,
    delivery_id: String,
    event: String,
    payload: String,
    received_at: String,
    repository_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePage {
    kind: String,
    stage: String,
    user_id: String,
    events: Vec<WireEvent>,
    next: i64,
    has_more: bool,
    ticket: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRefusal {
    kind: String,
}
#[derive(Deserialize)]
struct WireGap {
    kind: String,
    stage: String,
    expired_through: i64,
}
impl HttpRelaySource {
    pub fn new(
        http: reqwest::Client,
        configuration: RelayConfiguration,
        stage: Stage,
    ) -> Result<Self, Failure> {
        let url = url::Url::parse(&configuration.origin).map_err(|_| Failure::UnconfiguredStage)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Failure::UnconfiguredStage);
        }
        Ok(Self {
            http,
            configuration,
            stage,
        })
    }
    async fn open_watch(&self, ticket: &str) -> Result<RelayWatch, Failure> {
        let mut url =
            url::Url::parse(&self.configuration.origin).map_err(|_| Failure::UnconfiguredStage)?;
        url.set_scheme("wss")
            .map_err(|_| Failure::UnconfiguredStage)?;
        url.set_path("/relay/v1/watch");

        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| Failure::InvalidResponse)?;
        request.headers_mut().insert(
            "x-nessa-watch-ticket",
            ticket.parse().map_err(|_| Failure::InvalidResponse)?,
        );
        request.headers_mut().insert(
            "x-nessa-stage",
            self.stage
                .as_str()
                .parse()
                .map_err(|_| Failure::InvalidResponse)?,
        );
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| Failure::Offline)?
        .map_err(|_| Failure::Offline)?;
        tracing::debug!("GitHub relay watch connected");
        Ok(RelayWatch(socket))
    }
}
pub struct RelayWatch(WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>);
#[async_trait::async_trait]
impl RelayNotifications for RelayWatch {
    async fn changed(&mut self) -> Result<(), Failure> {
        loop {
            match self.0.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))
                    if text == "{\"type\":\"changed\"}" =>
                {
                    return Ok(())
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(bytes))) => {
                    self.0
                        .send(tokio_tungstenite::tungstenite::Message::Pong(bytes))
                        .await
                        .map_err(|_| Failure::Offline)?;
                }
                _ => return Err(Failure::Offline),
            }
        }
    }
}
#[async_trait::async_trait]
impl RelaySource for HttpRelaySource {
    async fn page(&self, token: &str, binding: &Binding, after: i64) -> Result<Page, Failure> {
        let mut url =
            url::Url::parse(&self.configuration.origin).map_err(|_| Failure::UnconfiguredStage)?;
        url.set_path("/relay/v1/events");
        url.query_pairs_mut()
            .append_pair("repository", binding.repository_id())
            .append_pair("after", &after.to_string());
        let mut response = self
            .http
            .get(url)
            .header("x-nessa-stage", self.stage.as_str())
            .header("x-github-user-id", binding.user_id())
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| Failure::Offline)?;
        match response.status().as_u16() {
            200 => (),
            401 => return Err(Failure::CredentialsUnavailable),
            403 => return Err(Failure::Forbidden),
            409 => {
                let refusal: WireRefusal = response
                    .json()
                    .await
                    .map_err(|_| Failure::InvalidResponse)?;
                return Err(match refusal.kind.as_str() {
                    "identity_changed" => Failure::IdentityChanged,
                    "stage_mismatch" => Failure::StageMismatch,
                    _ => Failure::InvalidResponse,
                });
            }
            410 => {
                let gap: WireGap = response
                    .json()
                    .await
                    .map_err(|_| Failure::InvalidResponse)?;
                if gap.stage != self.stage.as_str() {
                    return Err(Failure::StageMismatch);
                }
                if gap.kind != "gap" {
                    return Err(Failure::InvalidResponse);
                }
                return Err(Failure::Gap(gap.expired_through));
            }
            _ => return Err(Failure::Offline),
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Offline)? {
            if bytes.len() + chunk.len()
                > 2 * crate::github_relay::domain::contract::PAYLOAD_BYTES + 16_384
            {
                return Err(Failure::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        let page: WirePage =
            serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)?;
        if page.stage != self.stage.as_str() {
            return Err(Failure::StageMismatch);
        }
        if page.kind != "page" {
            return Err(Failure::InvalidResponse);
        }
        Ok(Page {
            user_id: page.user_id,
            next: page.next,
            has_more: page.has_more,
            ticket: page.ticket,
            events: page
                .events
                .into_iter()
                .map(|event| Event {
                    sequence: event.sequence,
                    delivery_id: event.delivery_id,
                    event: event.event,
                    payload: event.payload,
                    received_at: event.received_at,
                    repository_id: event.repository_id,
                })
                .collect(),
        })
    }
}

#[async_trait::async_trait]
impl RelayLiveSource for HttpRelaySource {
    async fn watch(&self, ticket: &str) -> Result<Box<dyn RelayNotifications>, Failure> {
        Ok(Box::new(self.open_watch(ticket).await?))
    }
}
