//! The caller kinds a settings change audits, read from the local credential store.
/// The local credential store knows each principal's kind.
impl crate::agents::application::CallerKindSource
    for nessa_auth::adapters::local::LocalCredentialStore
{
    fn caller_kind(&self, principal_id: &str) -> &'static str {
        match self.principal_kind(principal_id) {
            Ok(Some(nessa_auth::domain::PrincipalKind::Human)) => "person",
            Ok(Some(nessa_auth::domain::PrincipalKind::Agent)) => "agent",
            Ok(Some(nessa_auth::domain::PrincipalKind::Integration)) => "integration",
            Ok(Some(nessa_auth::domain::PrincipalKind::Gateway)) => "gateway",
            _ => "unknown",
        }
    }
}
