use super::{context::DOMAIN, ManualCode, PairingContext, MAX_ENROLLMENT_MESSAGE_BYTES};
use crate::application::pairing::ConfirmedClaim;
use crate::domain::pairing::{DeviceKey, InvitationId};
use opaque_ke::{
    argon2::{Algorithm, Argon2, Params, Version},
    ciphersuite::CipherSuite,
    rand::{CryptoRng, RngCore},
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, Identifiers, ServerLogin, ServerLoginParameters, ServerRegistration,
    ServerSetup,
};
use sha2::{Digest, Sha256, Sha512};
use std::fmt;
use zeroize::Zeroize;

struct Suite;
impl CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, Sha512>;
    type Ksf = Argon2<'static>;
}
fn ksf() -> Result<Argon2<'static>, PairingCryptoError> {
    let params =
        Params::new(65_536, 3, 4, Some(64)).map_err(|_| PairingCryptoError::Unavailable)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}
/// Safe protocol failures; underlying diagnostics and secret inputs are discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingCryptoError {
    /// Manual input violates the exact alphabet and normalization rule.
    InvalidCode,
    /// Public metadata or channel binding cannot form the selected transcript.
    InvalidContext,
    /// Peer message is malformed or authentication failed.
    InvalidProof,
    /// Selected cryptographic configuration is unavailable.
    Unavailable,
    /// Durable pending enrollment storage failed; KE3 must not be sent.
    PendingStorage,
}
impl fmt::Display for PairingCryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PairingCryptoError {}

/// Volatile one-use password file and server setup. Never serialize this owner.
/// The upstream owned setup and registration erase their secrets when dropped.
pub struct ServerInvitation {
    setup: ServerSetup<Suite>,
    registration: ServerRegistration<Suite>,
    invitation: InvitationId,
    gateway: [u8; 44],
}
impl ServerInvitation {
    /// Execute both registration roles locally using the fixed KSF and identities.
    /// Composition must reserve its sole physical KSF worker until this call returns.
    pub fn register(
        rng: &mut (impl RngCore + CryptoRng),
        code: &ManualCode,
        invitation: InvitationId,
        gateway: [u8; 44],
    ) -> Result<Self, PairingCryptoError> {
        let setup = ServerSetup::<Suite>::new(rng);
        let client = ClientRegistration::<Suite>::start(rng, code.expose_bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        let server = ServerRegistration::<Suite>::start(&setup, client.message, invitation.bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        let mut client_identity = DOMAIN.to_vec();
        client_identity.extend_from_slice(invitation.bytes());
        let mut server_identity = DOMAIN.to_vec();
        server_identity.extend_from_slice(&gateway);
        let ksf = ksf()?;
        let mut result = client
            .state
            .finish(
                rng,
                code.expose_bytes(),
                server.message,
                ClientRegistrationFinishParameters {
                    identifiers: Identifiers {
                        client: Some(&client_identity),
                        server: Some(&server_identity),
                    },
                    ksf: Some(&ksf),
                },
            )
            .map_err(|_| PairingCryptoError::Unavailable)?;
        result.export_key.zeroize();
        let registration = ServerRegistration::finish(result.message);
        Ok(Self {
            setup,
            registration,
            invitation,
            gateway,
        })
    }
    /// Start the server role for one previously reserved attempt on this exact channel.
    pub fn start(
        &self,
        rng: &mut (impl RngCore + CryptoRng),
        request: &[u8],
        context: PairingContext,
    ) -> Result<(ServerAttempt, Vec<u8>), PairingCryptoError> {
        if self.invitation != context.public.invitation() || self.gateway != context.gateway {
            return Err(PairingCryptoError::InvalidContext);
        }
        let (request, input) = decode_request(request)?;
        let parameters = ServerLoginParameters {
            context: Some(&context.bytes),
            identifiers: identities(&context),
        };
        let result = ServerLogin::<Suite>::start(
            rng,
            &self.setup,
            Some(self.registration.clone()),
            request,
            self.invitation.bytes(),
            parameters,
        )
        .map_err(|_| PairingCryptoError::InvalidProof)?;
        let response = result.message.serialize().to_vec();
        Ok((
            ServerAttempt {
                state: result.state,
                input,
                context,
            },
            response,
        ))
    }
}
/// Decode bounded KE1 and fingerprint its canonical library representation before
/// charging/dispatching ServerLogin. This creates no proof or expensive KSF job.
pub fn credential_request_fingerprint(bytes: &[u8]) -> Result<[u8; 32], PairingCryptoError> {
    decode_request(bytes).map(|(_, input)| input)
}
fn decode_request(
    bytes: &[u8],
) -> Result<(CredentialRequest<Suite>, [u8; 32]), PairingCryptoError> {
    if bytes.len() > MAX_ENROLLMENT_MESSAGE_BYTES {
        return Err(PairingCryptoError::InvalidProof);
    }
    let request = CredentialRequest::<Suite>::deserialize(bytes)
        .map_err(|_| PairingCryptoError::InvalidProof)?;
    let fingerprint = Sha256::digest(request.serialize()).into();
    Ok((request, fingerprint))
}

fn identities(context: &PairingContext) -> Identifiers<'_> {
    Identifiers {
        client: Some(&context.client_identity),
        server: Some(&context.server_identity),
    }
}
/// Volatile server login state for an admitted physical handshake.
pub struct ServerAttempt {
    state: ServerLogin<Suite>,
    context: PairingContext,
    input: [u8; 32],
}
/// Authenticated result of the selected PAKE server role, never a caller proof flag.
pub struct ConfirmedAttempt {
    context: PairingContext,
    proof: ConfirmedClaim,
}
impl ConfirmedAttempt {
    /// Borrow unforgeable confirmation evidence for the authoritative claim commit.
    pub fn proof(&self) -> &ConfirmedClaim {
        &self.proof
    }
    /// Exact authenticated public attempt correlation.
    pub fn context(&self) -> &PairingContext {
        &self.context
    }
    /// Permanent device key whose proof and transcript came from the native TLS owner.
    pub fn device(&self) -> DeviceKey {
        let mut key = [0; 32];
        key.copy_from_slice(&self.context.device[12..]);
        DeviceKey::new(key)
    }
}
impl ServerAttempt {
    /// Borrow the original channel-derived context before consuming KE3.
    /// Completion must compare this to its current transport's canonical context.
    pub fn context(&self) -> &PairingContext {
        &self.context
    }
    /// Validate KE3 using OPAQUE's native mutual confirmation; discard session keys.
    /// Commit the claim before emitting a protected claim receipt or erasing invitation setup.
    pub fn finish(self, message: &[u8]) -> Result<ConfirmedAttempt, PairingCryptoError> {
        if message.len() > MAX_ENROLLMENT_MESSAGE_BYTES {
            return Err(PairingCryptoError::InvalidProof);
        }
        let message = CredentialFinalization::<Suite>::deserialize(message)
            .map_err(|_| PairingCryptoError::InvalidProof)?;
        let mut result = self
            .state
            .finish(
                message,
                ServerLoginParameters {
                    context: Some(&self.context.bytes),
                    identifiers: identities(&self.context),
                },
            )
            .map_err(|_| PairingCryptoError::InvalidProof)?;
        result.session_key.zeroize();
        let public = self.context.public;
        let mut key = [0; 32];
        key.copy_from_slice(&self.context.device[12..]);
        let proof = ConfirmedClaim::from_pake(
            public.invitation(),
            public.attempt(),
            public.consent(),
            public.generation(),
            public.expiry_ms(),
            DeviceKey::new(key),
            self.input,
        );
        Ok(ConfirmedAttempt {
            context: self.context,
            proof,
        })
    }
}
/// Volatile client state; no credential, pin, or claim is established by KE1.
pub struct ClientAttempt {
    state: ClientLogin<Suite>,
}
impl ClientAttempt {
    /// Start the fixed PAKE with the owned normalized code.
    pub fn start(
        rng: &mut (impl RngCore + CryptoRng),
        code: &ManualCode,
    ) -> Result<(Self, Vec<u8>), PairingCryptoError> {
        let result = ClientLogin::<Suite>::start(rng, code.expose_bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        Ok((
            Self {
                state: result.state,
            },
            result.message.serialize().to_vec(),
        ))
    }
    /// Verify KE2, durably save the key/pin/exact pending context, then release KE3.
    /// `save_pending` must atomically commit and sync the client's existing private key,
    /// authenticated gateway pin, and attempt metadata. Its error sends no KE3.
    /// Successful KE2 authenticates the gateway; it does not establish a server claim.
    pub fn finish(
        self,
        rng: &mut (impl RngCore + CryptoRng),
        code: &ManualCode,
        response: &[u8],
        context: &PairingContext,
        save_pending: impl FnOnce(&PairingContext) -> Result<(), PairingCryptoError>,
    ) -> Result<Vec<u8>, PairingCryptoError> {
        if response.len() > MAX_ENROLLMENT_MESSAGE_BYTES {
            return Err(PairingCryptoError::InvalidProof);
        }
        let response = CredentialResponse::<Suite>::deserialize(response)
            .map_err(|_| PairingCryptoError::InvalidProof)?;
        let ksf = ksf()?;
        let mut result = self
            .state
            .finish(
                rng,
                code.expose_bytes(),
                response,
                ClientLoginFinishParameters {
                    context: Some(&context.bytes),
                    identifiers: identities(context),
                    ksf: Some(&ksf),
                },
            )
            .map_err(|_| PairingCryptoError::InvalidProof)?;
        result.session_key.zeroize();
        result.export_key.zeroize();
        save_pending(context).map_err(|_| PairingCryptoError::PendingStorage)?;
        Ok(result.message.serialize().to_vec())
    }
}
