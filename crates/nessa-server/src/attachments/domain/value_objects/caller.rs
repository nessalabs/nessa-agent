use crate::attachments::domain::AttachmentError;
use nessa_auth::domain::PrincipalId;

/// The verified caller behind one action: who, from which authenticated
/// surface, under which action identifier. Built only from session identity
/// the gateway verified, never from request metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    principal_id: PrincipalId,
    surface_id: Box<str>,
    action_id: Box<str>,
}
impl Caller {
    const MAX_BYTES: usize = 256;

    pub fn new(
        principal_id: PrincipalId,
        surface_id: &str,
        action_id: &str,
    ) -> Result<Self, AttachmentError> {
        for value in [surface_id, action_id] {
            if value.trim().is_empty()
                || value.len() > Self::MAX_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(AttachmentError::Caller);
            }
        }
        Ok(Self {
            principal_id,
            surface_id: surface_id.into(),
            action_id: action_id.into(),
        })
    }
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }
    /// The stable identifier of the logical action, used to correlate evidence.
    pub fn action_id(&self) -> &str {
        &self.action_id
    }
}
