//! Pure native enrollment JSON codec. It performs no physical framing or IO.
//! Domain constructors own identities, read class and consent correlation.
//! Syntax and envelope bounds are exercised through the public codec in
//! `tests/device_pairing/wire.rs`; the codec establishes no key proof or current authority.
#![deny(missing_docs)]
mod dto;
use crate::device_pairing::application::DevicePairingStatus;
use dto::{WireConsent, WirePublic, WireReply, WireRequest, WireStatus};
use nessa_auth::domain::{
    pairing::{
        AttemptId, AttemptOutcome, DisclosedConsent, PairingError, PairingPhase, PairingRecord,
        PublicIntent, TerminalCause,
    },
    CredentialId,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    borrow::Cow,
    error::Error,
    fmt::{Display, Formatter, Result as FmtResult},
    io::{Error as IoError, ErrorKind, Result as IoResult, Write},
};

/// Maximum bytes in a complete native enrollment JSON envelope, excluding framing.
/// This native representation policy is distinct from Auth's raw crypto-message
/// bound. JSON numeric arrays and metadata count toward this envelope.
/// `envelope_limits_apply_before_decode_and_during_encode` checks its boundary.
pub const MAX_ENROLLMENT_ENVELOPE_BYTES: usize = 4096;

/// Expected native representation refusals; physical IO is a separate owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeWireError {
    /// The complete encoded envelope exceeds the native representation bound.
    TooLarge,
    /// Unknown, malformed or contradictory current representation.
    Invalid,
    /// Public operation or protected consent contradicts canonical evidence.
    Correlation(PairingError),
}
impl Display for NativeWireError {
    fn fmt(&self, output: &mut Formatter<'_>) -> FmtResult {
        write!(output, "{self:?}")
    }
}
impl Error for NativeWireError {}
/// Decoded requests remain untrusted input until the runtime admission owner acts.
#[derive(Debug)]
pub enum NativePairingRequest {
    /// Public slot lookup for a new random attempt.
    Hello(AttemptId),
    /// Untrusted KE1 representation; later admission precedes physical server login.
    Begin {
        /// Exact opaque operation correlation.
        public: PublicIntent,
        /// Raw untrusted KE1 bytes; cryptographic validation belongs to Auth.
        request: Vec<u8>,
    },
    /// Untrusted KE3 for the later original-channel/retained-handshake consumer.
    Confirm {
        /// Exact opaque operation correlation.
        public: PublicIntent,
        /// Raw untrusted KE3 bytes for the retained original handshake.
        message: Vec<u8>,
    },
    /// Exact earlier attempt; fresh actual TLS possession is required by the owner.
    Status(PublicIntent),
}
/// Client-side decoded enrollment state; no variant grants product access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativePairingStatus {
    /// The original attempt may still claim; no terminal cause is invented.
    Pending(PublicIntent),
    /// Original failed/superseded attempt without private selectors.
    Unclaimed {
        /// Exact opaque operation correlation.
        public: PublicIntent,
        /// Original failed or superseded attempt outcome.
        outcome: AttemptOutcome,
        /// Original invitation terminal cause, if known.
        terminal: Option<TerminalCause>,
    },
    /// Claim before explicit owner consent.
    Claimed(Box<DisclosedConsent>),
    /// Explicit owner consent; staging/publication may still be pending.
    Approved(Box<DisclosedConsent>),
    /// Durable stage precedes physical receiver and registry publication.
    Staging(Box<DisclosedConsent>),
    /// Historical enrollment completion; each product request asks current authority.
    Active {
        /// Scope from the canonical confirmed enrollment projection.
        consent: Box<DisclosedConsent>,
        /// Historical issued credential identifier, not a bearer secret.
        credential: CredentialId,
    },
    /// Original claimed enrollment ended; it is not product authorization.
    Terminal {
        /// Scope from the canonical confirmed enrollment projection.
        consent: Box<DisclosedConsent>,
        /// First cause retained by the canonical invitation.
        cause: TerminalCause,
    },
}
impl NativePairingStatus {
    /// Exact operation correlation, including preclaim status.
    pub fn public(&self) -> PublicIntent {
        match self {
            Self::Pending(public) | Self::Unclaimed { public, .. } => *public,
            Self::Claimed(consent)
            | Self::Approved(consent)
            | Self::Staging(consent)
            | Self::Active { consent, .. }
            | Self::Terminal { consent, .. } => consent.public(),
        }
    }
    /// Disclosed scope when present. Its authenticity requires the later channel owner.
    pub fn consent(&self) -> Option<&DisclosedConsent> {
        match self {
            Self::Pending(_) | Self::Unclaimed { .. } => None,
            Self::Claimed(consent)
            | Self::Approved(consent)
            | Self::Staging(consent)
            | Self::Active { consent, .. }
            | Self::Terminal { consent, .. } => Some(consent),
        }
    }
    /// Ask the projection owner before replacing received same-operation evidence.
    pub fn correlate(
        &self,
        pending: PublicIntent,
        received: Option<&DisclosedConsent>,
    ) -> Result<(), NativeWireError> {
        match self.consent() {
            Some(consent) => consent
                .correlate(pending, received)
                .map_err(NativeWireError::Correlation),
            None if self.public() == pending && received.is_none() => Ok(()),
            None => Err(NativeWireError::Correlation(PairingError::Conflict)),
        }
    }
}
/// Decoded peer reply. Authentication comes from the complete native channel/PAKE.
#[derive(Debug)]
pub enum NativePairingReply {
    /// Public slot metadata contains no private owner or grant selectors.
    Hello(PublicIntent),
    /// Selected-library KE2 for the exact admitted operation.
    Challenge {
        /// Exact opaque operation correlation.
        public: PublicIntent,
        /// Raw KE2 bytes; decode does not establish validity.
        response: Vec<u8>,
    },
    /// Decoded enrollment status; authenticity remains with its channel consumer.
    Status(NativePairingStatus),
    /// Redacted refusal; it does not authorize a pin/cache change.
    Refused,
}
/// Refuse oversized complete envelopes before decoding their syntax and domain values.
/// Decoded requests remain untrusted; no key proof or admission is established.
pub fn decode_request(bytes: &[u8]) -> Result<NativePairingRequest, NativeWireError> {
    let request: WireRequest = decode(bytes)?;
    match request {
        WireRequest::Hello { attempt } => Ok(NativePairingRequest::Hello(AttemptId::new(attempt))),
        WireRequest::Begin { public, request } => Ok(NativePairingRequest::Begin {
            public: public.into_domain()?,
            request: request.into_owned(),
        }),
        WireRequest::Confirm { public, message } => Ok(NativePairingRequest::Confirm {
            public: public.into_domain()?,
            message: message.into_owned(),
        }),
        WireRequest::Status { public } => Ok(NativePairingRequest::Status(public.into_domain()?)),
    }
}
/// Encode a client request without retaining an unbounded serde output buffer.
pub fn encode_request(request: &NativePairingRequest) -> Result<Vec<u8>, NativeWireError> {
    let request = match request {
        NativePairingRequest::Hello(attempt) => WireRequest::Hello {
            attempt: *attempt.bytes(),
        },
        NativePairingRequest::Begin { public, request } => WireRequest::Begin {
            public: WirePublic::from_domain(*public),
            request: Cow::Borrowed(request),
        },
        NativePairingRequest::Confirm { public, message } => WireRequest::Confirm {
            public: WirePublic::from_domain(*public),
            message: Cow::Borrowed(message),
        },
        NativePairingRequest::Status(public) => WireRequest::Status {
            public: WirePublic::from_domain(*public),
        },
    };
    encode(&request)
}
/// Encode public metadata. The later query owner decides whether its slot is Available.
pub fn encode_hello(public: PublicIntent) -> Result<Vec<u8>, NativeWireError> {
    encode(&WireReply::Hello {
        public: WirePublic::from_domain(public),
    })
}
/// Encode raw KE2 bytes. Their validity belongs to the later retained-handshake owner.
pub fn encode_challenge(public: PublicIntent, response: &[u8]) -> Result<Vec<u8>, NativeWireError> {
    encode(&WireReply::Challenge {
        public: WirePublic::from_domain(public),
        response: Cow::Borrowed(response),
    })
}
/// Project trusted application status, checking claimed record correlation before encoding.
/// This function does not itself verify key possession or current authority.
pub fn encode_status(
    public: PublicIntent,
    status: &DevicePairingStatus,
) -> Result<Vec<u8>, NativeWireError> {
    let value = match status {
        DevicePairingStatus::Pending => WireStatus::Pending {
            public: WirePublic::from_domain(public),
        },
        DevicePairingStatus::Unclaimed { outcome, terminal } => {
            WireStatus::unclaimed(public, *outcome, *terminal)?
        }
        DevicePairingStatus::Claimed(record) => {
            let consent = DisclosedConsent::from_intent(public, record.intent())
                .map_err(NativeWireError::Correlation)?;
            return encode_claimed(public, record, &consent);
        }
    };
    encode(&WireReply::Status { status: value })
}
/// Redacted refusal contains no diagnostic, private selector or password-dependent detail.
pub fn encode_refused() -> Result<Vec<u8>, NativeWireError> {
    encode(&WireReply::Refused {})
}
/// Decode a complete bounded peer reply through the domain projection/identity owners.
/// Successful decode alone does not authenticate the peer or authorize replacing prior scope.
pub fn decode_reply(bytes: &[u8]) -> Result<NativePairingReply, NativeWireError> {
    let reply: WireReply = decode(bytes)?;
    match reply {
        WireReply::Hello { public } => Ok(NativePairingReply::Hello(public.into_domain()?)),
        WireReply::Challenge { public, response } => Ok(NativePairingReply::Challenge {
            public: public.into_domain()?,
            response: response.into_owned(),
        }),
        WireReply::Status { status } => Ok(NativePairingReply::Status(status.into_domain()?)),
        WireReply::Refused {} => Ok(NativePairingReply::Refused),
    }
}
fn encode_claimed(
    public: PublicIntent,
    record: &PairingRecord,
    consent: &DisclosedConsent,
) -> Result<Vec<u8>, NativeWireError> {
    if PublicIntent::from_record(record, public.attempt()).map_err(NativeWireError::Correlation)?
        != public
        || record
            .claim_binding()
            .is_none_or(|(attempt, _)| attempt != public.attempt())
    {
        return Err(NativeWireError::Correlation(PairingError::Conflict));
    }
    let consent = WireConsent::from_domain(consent);
    let status = match record.phase() {
        PairingPhase::Available => return Err(NativeWireError::Invalid),
        PairingPhase::Claimed => WireStatus::Claimed { consent },
        PairingPhase::Approved => WireStatus::Approved { consent },
        PairingPhase::Staging => WireStatus::Staging { consent },
        PairingPhase::Active => WireStatus::Active {
            consent,
            credential: record
                .credential()
                .ok_or(NativeWireError::Invalid)?
                .as_str()
                .into(),
        },
        PairingPhase::Terminal => WireStatus::Terminal {
            consent,
            cause: record.terminal().ok_or(NativeWireError::Invalid)?.0.into(),
        },
    };
    encode(&WireReply::Status { status })
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, NativeWireError> {
    if bytes.len() > MAX_ENROLLMENT_ENVELOPE_BYTES {
        return Err(NativeWireError::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| NativeWireError::Invalid)
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, NativeWireError> {
    let mut writer = BoundedFrame {
        bytes: Vec::with_capacity(MAX_ENROLLMENT_ENVELOPE_BYTES),
    };
    serde_json::to_writer(&mut writer, value).map_err(|error| {
        if error.io_error_kind() == Some(ErrorKind::WriteZero) {
            NativeWireError::TooLarge
        } else {
            NativeWireError::Invalid
        }
    })?;
    Ok(writer.bytes)
}
struct BoundedFrame {
    bytes: Vec<u8>,
}
impl Write for BoundedFrame {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        if bytes.len() > MAX_ENROLLMENT_ENVELOPE_BYTES - self.bytes.len() {
            return Err(IoError::from(ErrorKind::WriteZero));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/device_pairing/wire.rs"]
mod tests;
