use crate::attachments::domain::AttachmentError;
use nessa_auth::domain::PrincipalId;

/// The verified caller behind one action: who, from which authenticated
/// surface, under which action identifier. Built only from session identity
/// the gateway verified, never from request metadata.
///
/// The rule is the one the conversation context already applies to a caller
/// (the SDK's `ActionContext`): not blank, at most 256 bytes, and nothing else.
/// A caller the conversation accepts must be one this context can write down,
/// or a close would let go of files in a name no record could carry. Evidence
/// is stored as JSON, which can spell any character.
///
/// The domain does not read the SDK's application layer, so it states its own
/// bound; the application layer, which sees both, refuses to compile if the two
/// ever stop agreeing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    principal_id: PrincipalId,
    surface_id: Box<str>,
    action_id: Box<str>,
}
impl Caller {
    /// Longest surface or action identifier, in bytes.
    pub(crate) const MAX_BYTES: usize = 256;

    pub fn new(
        principal_id: PrincipalId,
        surface_id: &str,
        action_id: &str,
    ) -> Result<Self, AttachmentError> {
        for value in [surface_id, action_id] {
            if value.trim().is_empty() || value.len() > Self::MAX_BYTES {
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
