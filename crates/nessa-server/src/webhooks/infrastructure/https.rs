use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use super::signature;
use crate::agent_install::infrastructure::https_archives::install_tls_backend;
use crate::webhooks::application::{
    AddressResolver, MacKey, Message, PostFailure, PrepareFailure, WebhookHttp, WebhookPost,
    POST_DEADLINE, RESPONSE_BYTES,
};
use crate::webhooks::domain::Destination;

/// A bounded set of active attempts, with no unbounded waiter queue.
pub const MAX_IN_FLIGHT: usize = 4;
const MAX_ADDRESSES: usize = 16;

pub struct SystemResolver {
    capacity: Arc<Semaphore>,
}

impl SystemResolver {
    pub fn new() -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }
}

impl Default for SystemResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AddressResolver for SystemResolver {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, PrepareFailure> {
        // libc DNS may outlive cancellation. Its permit moves into the worker
        // and stays there until that physical operation returns. A timeout
        // cannot admit an unbounded succession of stuck resolver workers.
        let host = host.to_owned();
        lookup_worker(self.capacity.clone(), move || {
            let addresses = (host.as_str(), 443)
                .to_socket_addrs()
                .map_err(|_| PrepareFailure::Unavailable)?;
            let mut result = Vec::new();
            for address in addresses {
                if result.len() == MAX_ADDRESSES {
                    return Err(PrepareFailure::Destination);
                }
                result.push(address.ip());
            }
            Ok(result)
        })?
        .await
        .map_err(|_| PrepareFailure::Unavailable)?
    }
}

fn lookup_worker(
    capacity: Arc<Semaphore>,
    lookup: impl FnOnce() -> Result<Vec<IpAddr>, PrepareFailure> + Send + 'static,
) -> Result<tokio::task::JoinHandle<Result<Vec<IpAddr>, PrepareFailure>>, PrepareFailure> {
    let permit = capacity
        .try_acquire_owned()
        .map_err(|_| PrepareFailure::Busy)?;
    Ok(tokio::task::spawn_blocking(move || {
        let _permit = permit;
        lookup()
    }))
}

pub struct HttpsWebhook {
    resolver: Arc<dyn AddressResolver>,
    capacity: Arc<Semaphore>,
}

impl HttpsWebhook {
    /// Composition supplies the DNS seam. Client creation per attempt prevents
    /// cached connections from bypassing this attempt's validated DNS answer.
    pub fn new(resolver: Arc<dyn AddressResolver>) -> Self {
        install_tls_backend();
        Self {
            resolver,
            capacity: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }

    async fn pinned_client(
        &self,
        destination: &Destination,
    ) -> Result<reqwest::Client, PrepareFailure> {
        let host = destination
            .expose_url()
            .host_str()
            .ok_or(PrepareFailure::Destination)?;
        let addresses = self.resolver.resolve(host).await?;
        if addresses.is_empty()
            || addresses.len() > MAX_ADDRESSES
            || addresses.iter().any(|ip| !public_address(*ip))
        {
            return Err(PrepareFailure::Destination);
        }
        let addresses: Vec<_> = addresses
            .into_iter()
            .map(|ip| SocketAddr::new(ip, 443))
            .collect();
        client_builder(host, &addresses)
            .build()
            .map_err(|_| PrepareFailure::Unavailable)
    }
}

fn client_builder(host: &str, addresses: &[SocketAddr]) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .https_only(true)
        .http1_only()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(POST_DEADLINE)
        .timeout(POST_DEADLINE)
        .resolve_to_addrs(host, addresses)
}

#[async_trait]
impl WebhookHttp for HttpsWebhook {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| PrepareFailure::Busy)?;
        let deadline = Instant::now() + POST_DEADLINE;
        let client = tokio::time::timeout_at(deadline, self.pinned_client(destination))
            .await
            .map_err(|_| PrepareFailure::TimedOut)??;
        Ok(Box::new(PreparedPost {
            client,
            url: destination.expose_url().clone(),
            deadline,
            _permit: permit,
        }))
    }
}

struct PreparedPost {
    client: reqwest::Client,
    url: url::Url,
    deadline: Instant,
    _permit: OwnedSemaphorePermit,
}

#[async_trait]
impl WebhookPost for PreparedPost {
    async fn post(self: Box<Self>, key: &MacKey, message: &Message) -> Result<u16, PostFailure> {
        // timeout_at may poll its inner future once even when already expired.
        // Refuse before constructing/polling the request in that case.
        if Instant::now() >= self.deadline {
            return Err(PostFailure::TimedOut);
        }
        tokio::time::timeout_at(self.deadline, call(&self.client, &self.url, key, message))
            .await
            .map_err(|_| PostFailure::TimedOut)?
    }
}

async fn call(
    client: &reqwest::Client,
    url: &url::Url,
    key: &MacKey,
    message: &Message,
) -> Result<u16, PostFailure> {
    let mut response = client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            "Nessa-Webhook-Timestamp",
            message.timestamp_ms().to_string(),
        )
        .header("Nessa-Webhook-Id", message.id().to_string())
        .header("Nessa-Webhook-Signature", signature(key, message))
        .body(message.body().to_vec())
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                PostFailure::TimedOut
            } else if error.is_builder() || error.is_connect() {
                PostFailure::NotSent
            } else {
                // send() may have read a partial status line or headers. Its
                // error predicates do not prove zero response bytes. Preserve
                // uncertainty rather than admitting a duplicate remote effect.
                PostFailure::IncompleteResponse
            }
        })?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|len| len > RESPONSE_BYTES as u64)
    {
        return Err(PostFailure::ResponseTooLarge);
    }
    let mut size = 0usize;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            PostFailure::TimedOut
        } else {
            PostFailure::IncompleteResponse
        }
    })? {
        size = size
            .checked_add(chunk.len())
            .ok_or(PostFailure::ResponseTooLarge)?;
        if size > RESPONSE_BYTES {
            return Err(PostFailure::ResponseTooLarge);
        }
        // Response content is deliberately discarded, without logging it.
    }
    Ok(status)
}

/// Accept only globally routed unicast space. IPv4 uses positive allocation
/// ranges with special-use holes; IPv6 is restricted to 2000::/3 with its
/// special-purpose /23 excluded. Mapped IPv4 is evaluated as IPv4. The exact
/// table is tested in destination_policy; there is no private-address opt-in.
fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => public_v4(ip),
            None => public_v6(ip),
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    (1..224).contains(&a)
        && a != 10
        && a != 127
        && !(a == 100 && (64..128).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..32).contains(&b))
        && !(a == 192 && (b == 0 || b == 168 || (b == 88 && c == 99)))
        && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    (s[0] & 0xe000) == 0x2000
        && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
        && s[0] != 0x2002
        && !(s[0] == 0x3fff && s[1] < 0x1000)
}

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/https.rs"]
mod https_tests;
