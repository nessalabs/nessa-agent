//! Protected transient scope projection; it is neither proof nor grant authority.
use super::{ConsentIntent, PairingError, PublicIntent};
use crate::domain::{AudienceId, Grant};

/// Protected canonical scope, omitting private owner and membership selectors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisclosedConsent {
    public: PublicIntent,
    audience: AudienceId,
    grant: Grant,
    additional_grants: Vec<Grant>,
}
impl DisclosedConsent {
    /// Decode a legacy single-capability consent. Classes requiring more fail closed.
    /// Key/channel authentication and canonical-intent correlation belong to consumers.
    pub fn new(
        public: PublicIntent,
        audience: AudienceId,
        grant: Grant,
    ) -> Result<Self, PairingError> {
        Self::with_grants(public, audience, vec![grant])
    }
    /// Decode the complete ordered canonical capability set. Omitted or
    /// additional capabilities cannot produce a narrower disclosure.
    pub fn with_grants(
        public: PublicIntent,
        audience: AudienceId,
        mut grants: Vec<Grant>,
    ) -> Result<Self, PairingError> {
        let resource = grants
            .first()
            .ok_or(PairingError::Invalid)?
            .resource()
            .clone();
        if grants != public.class().grants(resource)? {
            return Err(PairingError::Invalid);
        }
        let grant = grants.remove(0);
        Ok(Self {
            public,
            audience,
            grant,
            additional_grants: grants,
        })
    }
    /// Derive one projection from canonical full consent and exact public correlation.
    pub fn from_intent(public: PublicIntent, intent: &ConsentIntent) -> Result<Self, PairingError> {
        let value = Self::with_grants(
            public,
            intent.audience().clone(),
            intent.grants().cloned().collect(),
        )?;
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
            || self.grants().ne(intent.grants())
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
    /// Canonical grant of its class, not locally issued authority.
    pub fn grant(&self) -> &Grant {
        &self.grant
    }
    /// Complete canonical consent, including every capability of an owner phone.
    pub fn grants(&self) -> impl Iterator<Item = &Grant> {
        std::iter::once(&self.grant).chain(self.additional_grants.iter())
    }
    /// The explicit additional capabilities; empty for every legacy class.
    pub fn additional_grants(&self) -> &[Grant] {
        &self.additional_grants
    }
}
