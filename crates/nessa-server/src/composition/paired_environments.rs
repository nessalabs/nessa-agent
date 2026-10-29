//! Paired environments, composed (issue #706): the conversation side's
//! [`PeerEnvironments`] behind the native listener's `openEnvironment`, and
//! the one answer to whether a key may serve, over the registry and the
//! policy.
//!
//! ```text
//! listener ──▶ ListenerEnvironments::admits / serve(stream)
//!   ──▶ PeerEnvironments (the stream split; closing it ends the TLS stream)
//! PeerEnvironments ──▶ RegistryServing::serves(proof)
//!   ──▶ each credential bound to the proved key ──▶ AuthenticateSession
//!         ──▶ AuthorizeAction(environment.serve, this gateway) ──▶ allowed?
//! ```
//! Arrows are calls. A key serves while some credential bound to it
//! authenticates and is allowed `environment.serve` on this gateway now: the
//! same session and policy every other action passes, asked at admission,
//! before every lease and every keepalive interval while connected, so a
//! revoked credential or a disabled membership ends the connection and its
//! leases without a copy kept anywhere.
use crate::conversation::application::EnvironmentFuture;
use crate::conversation::infrastructure::ssh_environment::{PeerEnvironments, ServingCredentials};
use crate::device_pairing::infrastructure::{EnvironmentPeers, TlsCloser, TlsStream};
use nessa_auth::{
    adapters::local::LocalCredentialStore,
    application::{
        authorization::AuthorizeAction,
        pairing::DeviceConnectionProof,
        ports::{AccessError, Clock, CredentialEvidence, Decision, PolicyEvaluator},
        session::AuthenticateSession,
    },
    domain::{Action, AudienceId, Resource},
};
use std::{future::Future, pin::Pin, sync::Arc};

/// The action a paired environment's credential must be allowed.
const SERVE: &str = "environment.serve";

/// Whether a key may serve, answered by the registry and the policy.
pub(super) struct RegistryServing {
    pub(super) store: Arc<LocalCredentialStore>,
    pub(super) policy: Arc<dyn PolicyEvaluator>,
    pub(super) clock: Arc<dyn Clock>,
    /// This gateway, the resource serving is allowed on.
    pub(super) gateway: Resource,
    pub(super) audience: AudienceId,
}

impl ServingCredentials for RegistryServing {
    fn serves<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
    ) -> EnvironmentFuture<'a, Result<bool, AccessError>> {
        Box::pin(async move {
            let verifier = self.store.device_verifier(proof);
            let action = Action::new(SERVE).map_err(|_| AccessError::Unavailable)?;
            for credential in verifier.bound_credentials(&self.audience)? {
                let evidence = CredentialEvidence::new(credential.as_str().as_bytes().to_vec())
                    .map_err(|_| AccessError::Unavailable)?;
                let authenticated = AuthenticateSession {
                    policy: self.policy.as_ref(),
                    verifier: &verifier,
                    access: self.store.as_ref(),
                    clock: self.clock.as_ref(),
                }
                .execute(&evidence, &self.audience)
                .await;
                let session = match authenticated {
                    Ok(session) => session,
                    // Unreadable is no answer; any other refusal is this
                    // credential's alone.
                    Err(AccessError::Unavailable) => return Err(AccessError::Unavailable),
                    Err(_) => continue,
                };
                let decided = AuthorizeAction {
                    access: self.store.as_ref(),
                    clock: self.clock.as_ref(),
                    policy: self.policy.as_ref(),
                }
                .execute(&session, &action, &self.gateway)
                .await;
                match decided {
                    Ok(Decision::Allow) => return Ok(true),
                    Err(AccessError::Unavailable) => return Err(AccessError::Unavailable),
                    Ok(Decision::Deny) | Err(_) => {}
                }
            }
            Ok(false)
        })
    }
}

/// The paired environments, as the native listener serves them.
pub(super) struct ListenerEnvironments(pub(super) Arc<PeerEnvironments>);

impl EnvironmentPeers for ListenerEnvironments {
    fn admits<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AccessError>> + Send + 'a>> {
        Box::pin(self.0.admits(proof))
    }

    fn serve(&self, stream: TlsStream) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let peers = self.0.clone();
        Box::pin(async move {
            let proof = stream.device_proof().clone();
            let closer = CloseOnDrop(stream.closer());
            let (from_environment, to_environment) = tokio::io::split(stream);
            peers
                .serve(
                    proof,
                    Box::new(from_environment),
                    Box::new(to_environment),
                    Box::new(closer),
                )
                .await;
        })
    }
}

/// Ends the TLS stream when its connection is let go: the reader is woken
/// at once, rather than waiting for the peer to close.
struct CloseOnDrop(Arc<TlsCloser>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}
