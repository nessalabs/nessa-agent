use async_trait::async_trait;
use std::net::IpAddr;
use std::time::Duration;

use super::{MacKey, Message};
use crate::webhooks::domain::{Destination, Observation};

/// DNS, connect, TLS, request and the complete bounded response share a budget.
pub const POST_DEADLINE: Duration = Duration::from_secs(15);
pub const RESPONSE_BYTES: usize = 16 * 1024;

/// Errors carry no endpoint, headers, key, body or remote response text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostFailure {
    NotSent,
    IncompleteResponse,
    ResponseTooLarge,
    TimedOut,
}

/// Preparation has no POST effect. Busy leaves the delivery queued, without
/// consuming an attempt or writing Sending. Other preparation failures also
/// happen before admission and are owned by the caller's queue policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareFailure {
    Destination,
    Busy,
    Unavailable,
    TimedOut,
}

#[async_trait]
pub trait AddressResolver: Send + Sync {
    /// At most 16 addresses. No valid address is a destination failure.
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, PrepareFailure>;
}

#[async_trait]
pub trait WebhookHttp: Send + Sync {
    /// Reserve bounded transport capacity and resolve/validate/pin the
    /// destination, without a key or body. Prepare before admitting and
    /// persisting Sending; post consumes the returned reservation once.
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure>;
}

#[async_trait]
pub trait WebhookPost: Send {
    /// One POST observation. A complete response means headers and the body
    /// completed inside the deadline; the body also fits the byte cap. HTTP
    /// parsing has its own finite header bound. No redirect is followed.
    async fn post(self: Box<Self>, key: &MacKey, message: &Message) -> Result<u16, PostFailure>;
}

/// Convert one transport observation to the delivery contract. Unproven failures
/// remain unknown: reqwest cannot distinguish zero response bytes from an
/// incomplete status line or response headers at its send-error boundary.
pub fn observation(result: Result<u16, PostFailure>) -> Observation {
    match result {
        Ok(200..=299) => Observation::Success,
        Ok(408 | 429 | 500..=599) => Observation::Retryable,
        Ok(_) => Observation::Rejected,
        Err(PostFailure::NotSent) => Observation::Retryable,
        Err(PostFailure::IncompleteResponse | PostFailure::ResponseTooLarge) => {
            Observation::Unknown
        }
        Err(PostFailure::TimedOut) => Observation::TimedOut,
    }
}
