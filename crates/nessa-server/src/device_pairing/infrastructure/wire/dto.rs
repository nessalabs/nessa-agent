use super::{NativePairingStatus, NativeWireError};
use nessa_auth::domain::{
    pairing::{
        AttemptFailure, AttemptId, AttemptOutcome, ConsentIntentId, DisclosedConsent, InvitationId,
        PairingError, PublicIntent, TerminalCause,
    },
    Action, AudienceId, CredentialId, Grant, OrganizationId, Resource, ResourceId,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct WirePublic<'a> {
    invitation: [u8; InvitationId::LENGTH],
    attempt: [u8; AttemptId::LENGTH],
    consent: [u8; ConsentIntentId::LENGTH],
    generation: u64,
    expiry_ms: u64,
    class: Cow<'a, str>,
}
impl WirePublic<'_> {
    pub(super) fn from_domain(public: PublicIntent) -> WirePublic<'static> {
        WirePublic {
            invitation: *public.invitation().bytes(),
            attempt: *public.attempt().bytes(),
            consent: *public.consent().bytes(),
            generation: public.generation(),
            expiry_ms: public.expiry_ms(),
            class: Cow::Borrowed(public.class()),
        }
    }
    pub(super) fn into_domain(self) -> Result<PublicIntent, NativeWireError> {
        let public = PublicIntent::new(
            InvitationId::new(self.invitation),
            AttemptId::new(self.attempt),
            ConsentIntentId::new(self.consent),
            self.generation,
            self.expiry_ms,
        )
        .map_err(NativeWireError::Correlation)?;
        if self.class != public.class() {
            return Err(NativeWireError::Invalid);
        }
        Ok(public)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct WireConsent<'a> {
    public: WirePublic<'a>,
    audience: Cow<'a, str>,
    organization: Cow<'a, str>,
    resource: Cow<'a, str>,
    action: Cow<'a, str>,
}
impl WireConsent<'_> {
    pub(super) fn from_domain(consent: &DisclosedConsent) -> WireConsent<'_> {
        WireConsent {
            public: WirePublic::from_domain(consent.public()),
            audience: consent.audience().as_str().into(),
            organization: consent.grant().resource().organization_id().as_str().into(),
            resource: consent.grant().resource().id().as_str().into(),
            action: consent.grant().action().as_str().into(),
        }
    }
    fn into_domain(self) -> Result<Box<DisclosedConsent>, NativeWireError> {
        let public = self.public.into_domain()?;
        let audience =
            AudienceId::new(self.audience.into_owned()).map_err(|_| NativeWireError::Invalid)?;
        let resource = Resource::new(
            OrganizationId::new(self.organization.into_owned())
                .map_err(|_| NativeWireError::Invalid)?,
            ResourceId::new(self.resource.into_owned()).map_err(|_| NativeWireError::Invalid)?,
        );
        let grant = Grant::new(
            Action::new(self.action.into_owned()).map_err(|_| NativeWireError::Invalid)?,
            resource,
        );
        DisclosedConsent::new(public, audience, grant)
            .map(Box::new)
            .map_err(NativeWireError::Correlation)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum WireRequest<'a> {
    Hello {
        attempt: [u8; AttemptId::LENGTH],
    },
    Begin {
        public: WirePublic<'a>,
        request: Cow<'a, [u8]>,
    },
    Confirm {
        public: WirePublic<'a>,
        message: Cow<'a, [u8]>,
    },
    Status {
        public: WirePublic<'a>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum WireReply<'a> {
    Hello {
        public: WirePublic<'a>,
    },
    Challenge {
        public: WirePublic<'a>,
        response: Cow<'a, [u8]>,
    },
    Status {
        status: WireStatus<'a>,
    },
    Refused {},
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum WireStatus<'a> {
    Pending {
        public: WirePublic<'a>,
    },
    Unclaimed {
        public: WirePublic<'a>,
        outcome: WireUnclaimed,
        terminal: Option<WireTerminalCause>,
    },
    Claimed {
        consent: WireConsent<'a>,
    },
    Approved {
        consent: WireConsent<'a>,
    },
    Staging {
        consent: WireConsent<'a>,
    },
    Active {
        consent: WireConsent<'a>,
        credential: Cow<'a, str>,
    },
    Terminal {
        consent: WireConsent<'a>,
        cause: WireTerminalCause,
    },
}
impl WireStatus<'_> {
    pub(super) fn unclaimed(
        public: PublicIntent,
        outcome: AttemptOutcome,
        terminal: Option<TerminalCause>,
    ) -> Result<Self, NativeWireError> {
        let outcome = match outcome {
            AttemptOutcome::Failed(cause) => WireUnclaimed::Failed {
                cause: cause.into(),
            },
            AttemptOutcome::Superseded => WireUnclaimed::Superseded {},
            AttemptOutcome::Pending | AttemptOutcome::Claimed => {
                return Err(NativeWireError::Correlation(PairingError::Conflict))
            }
        };
        Ok(Self::Unclaimed {
            public: WirePublic::from_domain(public),
            outcome,
            terminal: terminal.map(Into::into),
        })
    }
    pub(super) fn into_domain(self) -> Result<NativePairingStatus, NativeWireError> {
        Ok(match self {
            Self::Pending { public } => NativePairingStatus::Pending(public.into_domain()?),
            Self::Unclaimed {
                public,
                outcome,
                terminal,
            } => NativePairingStatus::Unclaimed {
                public: public.into_domain()?,
                outcome: outcome.into(),
                terminal: terminal.map(Into::into),
            },
            Self::Claimed { consent } => NativePairingStatus::Claimed(consent.into_domain()?),
            Self::Approved { consent } => NativePairingStatus::Approved(consent.into_domain()?),
            Self::Staging { consent } => NativePairingStatus::Staging(consent.into_domain()?),
            Self::Active {
                consent,
                credential,
            } => NativePairingStatus::Active {
                consent: consent.into_domain()?,
                credential: CredentialId::new(credential.into_owned())
                    .map_err(|_| NativeWireError::Invalid)?,
            },
            Self::Terminal { consent, cause } => NativePairingStatus::Terminal {
                consent: consent.into_domain()?,
                cause: cause.into(),
            },
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum WireUnclaimed {
    Failed { cause: WireAttemptFailure },
    Superseded {},
}
impl From<WireUnclaimed> for AttemptOutcome {
    fn from(value: WireUnclaimed) -> Self {
        match value {
            WireUnclaimed::Failed { cause } => Self::Failed(cause.into()),
            WireUnclaimed::Superseded {} => Self::Superseded {},
        }
    }
}
macro_rules! mapped_enum {
    ($wire:ident, $domain:ident, {$($variant:ident),+ $(,)?}) => {
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub(super) enum $wire { $($variant),+ }
        impl From<$domain> for $wire {
            fn from(value: $domain) -> Self { match value { $($domain::$variant => Self::$variant),+ } }
        }
        impl From<$wire> for $domain {
            fn from(value: $wire) -> Self { match value { $($wire::$variant => Self::$variant),+ } }
        }
    };
}
mapped_enum!(WireTerminalCause, TerminalCause, {CredentialRevoked, Denied, Cancelled, Expired, Restarted});
mapped_enum!(WireAttemptFailure, AttemptFailure, {InvalidProof, ConnectionClosed, HandshakeDeadline, VerifierUnavailable});
