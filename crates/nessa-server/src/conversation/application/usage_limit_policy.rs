//! Whether this caller may change the gateway's usage-limit preference.
//!
//! The facts are the current principal, organization, role, whether the
//! membership is active, and each grant. Cedar decides. A member is forbidden
//! even when the grant is on the credential. The socket's grant check is a
//! separate question and is not this one.

use nessa_auth::application::ports::AccessError;

/// One grant record, as the credential stores it. Not a verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageLimitPreferenceGrant {
    /// The grant's action name.
    pub action: String,
    /// The resource the grant names.
    pub resource_id: String,
    /// The organization that owns that resource.
    pub organization_id: String,
}

/// What is true of the caller and this gateway when the preference is changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageLimitPreferenceFacts {
    /// The snapshot's principal. The service refuses a caller who is not this principal.
    pub principal_id: String,
    /// The snapshot's organization.
    pub organization_id: String,
    /// `admin` or `member`, copied from the membership. Not a permission.
    pub role: String,
    /// Whether the membership is active.
    pub active: bool,
    /// Every grant on the credential, unfiltered.
    pub grants: Vec<UsageLimitPreferenceGrant>,
    /// This gateway's resource id.
    pub gateway_id: String,
    /// The organization that owns this gateway.
    pub gateway_organization: String,
}

/// Cedar's answer, with the policies that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageLimitPreferenceDecision {
    /// The change may be written.
    Allow {
        /// Policies that permitted it.
        policies: Vec<String>,
    },
    /// The store is left unchanged.
    Deny {
        /// Policies that forbade it, when a forbid matched.
        policies: Vec<String>,
    },
}

/// The preference change asks this before it writes. Missing policy data is
/// [`AccessError::Unavailable`], which the caller treats as a refusal to write.
pub trait UsageLimitPreferencePolicyEvaluator: Send + Sync {
    /// Decide `facts`. An error is not an allow.
    fn evaluate(
        &self,
        facts: &UsageLimitPreferenceFacts,
    ) -> Result<UsageLimitPreferenceDecision, AccessError>;
}
