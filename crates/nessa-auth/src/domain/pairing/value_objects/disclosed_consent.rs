//! Protected transient scope projection; it is neither proof nor grant authority.
use super::{ConsentIntent, PairingError, PublicIntent};
use crate::domain::{AudienceId, Grant};

/// Protected canonical read scope, omitting private owner and membership selectors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisclosedConsent {
    public: PublicIntent,
    audience: AudienceId,
    grant: Grant,
}
impl DisclosedConsent {
    /// Decode a bounded domain grant through the canonical fixed read-class owner.
    /// Key/channel authentication and canonical-intent correlation belong to consumers.
    pub fn new(
        public: PublicIntent,
        audience: AudienceId,
        grant: Grant,
    ) -> Result<Self, PairingError> {
        if grant != ConsentIntent::read_grant(grant.resource().clone())? {
            return Err(PairingError::Invalid);
        }
        Ok(Self {
            public,
            audience,
            grant,
        })
    }
    /// Derive one projection from canonical full consent and exact public correlation.
    pub fn from_intent(public: PublicIntent, intent: &ConsentIntent) -> Result<Self, PairingError> {
        let value = Self::new(public, intent.audience().clone(), intent.grant().clone())?;
        value.verify_intent(intent)?;
        Ok(value)
    }
    /// Compare all disclosed facts to the gateway's canonical retained intent.
    /// The native output owner asks this before encoding a protected receipt.
    pub fn verify_intent(&self, intent: &ConsentIntent) -> Result<(), PairingError> {
        if self.public.consent() != intent.id()
            || self.public.generation() != intent.generation()
            || self.public.class() != intent.class()
            || self.audience != *intent.audience()
            || self.grant != *intent.grant()
        {
            return Err(PairingError::Conflict);
        }
        Ok(())
    }
    /// Check exact durable pending correlation and any already received projection.
    /// With no prior disclosure, private selectors rely on the authenticated gateway;
    /// `disclosure_retains_received_scope_on_retry` exercises that knowledge boundary.
    pub fn correlate(
        &self,
        pending: PublicIntent,
        received: Option<&Self>,
    ) -> Result<(), PairingError> {
        if self.public != pending || received.is_some_and(|prior| prior != self) {
            return Err(PairingError::Conflict);
        }
        Ok(())
    }
    /// Exact invitation, attempt and opaque consent correlation.
    pub fn public(&self) -> PublicIntent {
        self.public
    }
    /// Canonical gateway audience disclosed after mutual confirmation.
    pub fn audience(&self) -> &AudienceId {
        &self.audience
    }
    /// Canonical fixed read grant, not locally issued authority.
    pub fn grant(&self) -> &Grant {
        &self.grant
    }
}
