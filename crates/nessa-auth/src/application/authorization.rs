//! Current-state authorization after session authentication.
//!
//! Each call reads a coherent snapshot and checks the session deadline before
//! invoking the injected policy engine. This prevents an old login snapshot from
//! retaining roles or grants that have since changed. The hosting gateway still
//! owns resource lookup and operation admission. A successful check admits one
//! operation; later revocation does not retroactively cancel it or its response.
use super::{
    ports::{AccessError, AccessReader, Clock, Decision, PolicyEvaluator},
    session::AuthenticatedSession,
};
use crate::domain::{Action, Resource};

/// Reauthorize one action with current membership and credential restrictions.
/// Dependencies are borrowed from composition; this use case has no global state.
pub struct AuthorizeAction<'a> {
    /// Reads the current committed credential/membership revision.
    pub access: &'a dyn AccessReader,
    /// Absolute clock used for both session and credential expiry.
    pub clock: &'a dyn Clock,
    /// Embedded policy evaluator; Nessa's composition can supply Cedar.
    pub policy: &'a dyn PolicyEvaluator,
}
impl AuthorizeAction<'_> {
    /// Check `action` on the server-resolved `resource` for `session`.
    ///
    /// Returns Deny for policy/tenant rejection; invalid session metadata, expired
    /// credentials, and unavailable dependencies return typed errors. Both outcomes
    /// must prevent dispatch. Resource ownership must come from trusted application
    /// state, not from an unchecked request DTO. Reads cannot roll back an operation
    /// already admitted before revocation. The snapshot read is the ordering point
    /// relative to concurrent publication: if its checks allow, that operation may
    /// finish. A later operation must read again, never reuse this decision.
    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
        action: &Action,
        resource: &Resource,
    ) -> Result<Decision, AccessError> {
        let snapshot = super::session::ReadCurrentSession {
            access: self.access,
            clock: self.clock,
        }
        .execute(session)
        .await?;
        self.policy
            .evaluate(session.context(), action, resource, &snapshot)
    }
}
