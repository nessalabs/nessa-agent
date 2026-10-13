//! First-party phone scope decisions consume current canonical facts through Cedar.
use super::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    ConversationAccessPolicyEvaluator, RequestShape,
};
use crate::conversation::domain::{Conversation, ReceiverBinding};
use nessa_auth::{
    application::{
        pairing::{OwnerPhonePurpose, OwnerPhonePurposeSource},
        ports::{AccessError, Clock},
        session::AuthenticatedSession,
    },
    domain::{AuthContext, OrganizationId, PrincipalId, Resource},
};
use nessa_protocol::conversation::read_scope::ReadRefusal;
use std::{future::Future, pin::Pin, sync::Arc};

/// One freshly admitted first-party reader. It carries trusted purpose and
/// current original holding, never a cached permission decision. Each target
/// still asks the composed policy with authoritative metadata.
#[derive(Clone)]
pub struct OwnerPhoneReader {
    context: AuthContext,
    purpose: Arc<OwnerPhonePurpose>,
    receiver: ReceiverBinding,
    gateway: Resource,
    policy: Arc<dyn OwnerPhonePolicyEvaluator>,
    access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
}
impl OwnerPhoneReader {
    pub(crate) fn new(
        context: AuthContext,
        purpose: OwnerPhonePurpose,
        receiver: ReceiverBinding,
        gateway: Resource,
        policy: Arc<dyn OwnerPhonePolicyEvaluator>,
        access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    ) -> Self {
        Self {
            context,
            purpose: Arc::new(purpose),
            receiver,
            gateway,
            policy,
            access_policy,
        }
    }

    /// Scope correlation is factual; this does not grant access to a target.
    pub fn receiver(&self) -> &ReceiverBinding {
        &self.receiver
    }

    /// The shared policy receives the actual domain target, including deletion.
    pub fn admit_conversation(
        &self,
        conversation: &Conversation,
        action: AccessAction,
    ) -> Result<(), ReadRefusal> {
        self.admit_access(action, AccessResource::of(conversation))
    }

    /// The resource names the requested catalogue, never a substituted scope.
    pub fn admit_catalogue(
        &self,
        owner: &PrincipalId,
        organization: &OrganizationId,
        action: AccessAction,
    ) -> Result<(), ReadRefusal> {
        self.admit_access(
            action,
            AccessResource::Catalogue {
                organization,
                owner,
            },
        )
    }

    /// Catalogue metadata can include an actual tombstone; Locate is shared
    /// policy's question for whether this first-party surface may learn its ID.
    pub fn admit_entry(
        &self,
        owner: &PrincipalId,
        organization: &OrganizationId,
        deleted: bool,
    ) -> Result<(), ReadRefusal> {
        self.admit_access(
            AccessAction::Locate,
            AccessResource::Conversation {
                organization,
                owner,
                deleted,
            },
        )
    }

    fn admit_access(
        &self,
        action: AccessAction,
        resource: AccessResource<'_>,
    ) -> Result<(), ReadRefusal> {
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: self.context.organization_id(),
                principal: self.context.principal_id(),
            },
            action,
            resource,
            held: None,
            request: RequestShape::default(),
            target: None,
        };
        match self.access_policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => Ok(()),
            AccessDecision::Deny(AccessDenial::Unavailable) => Err(ReadRefusal::Unverifiable),
            AccessDecision::Deny(denial) => {
                tracing::debug!(
                    action = action.name(),
                    ?denial,
                    "conversation access refused"
                );
                Err(ReadRefusal::WrongOwner)
            }
        }
    }

    pub(crate) fn admit_operation(
        &self,
        operation: OwnerPhoneOperation<'_>,
    ) -> Result<(), ReadRefusal> {
        let operation_name = operation.name();
        // Diagnostics identify the refused request without retaining its body or
        // admitting unbounded user-supplied names into the audit record.
        const MAX_METHOD_TRACE_BYTES: usize = 256;
        let original_method = match &operation {
            OwnerPhoneOperation::ProductMethod { method, .. } => Some(*method),
            _ => None,
        };
        let method = original_method.map(|value| {
            let mut end = value.len().min(MAX_METHOD_TRACE_BYTES);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            &value[..end]
        });
        let method_bytes = original_method.map(str::len);
        let method_truncated = original_method
            .zip(method)
            .is_some_and(|(original, shown)| original.len() != shown.len());
        match self.policy.evaluate(OwnerPhonePolicyFacts {
            gateway: &self.gateway,
            context: &self.context,
            purpose: &self.purpose,
            receiver: &self.receiver,
            operation,
        }) {
            Ok(OwnerPhoneDecision::Allow { .. }) => Ok(()),
            Ok(OwnerPhoneDecision::Deny { mut rules }) => {
                rules.sort();
                rules.dedup();
                let outcome = if rules.is_empty() {
                    "default_deny"
                } else {
                    "policy_deny"
                };
                tracing::debug!(outcome, operation = operation_name, ?method, ?method_bytes, method_truncated, ?rules,
                    credential = %self.context.credential_id().as_str(), receiver = %self.receiver.receiver_id,
                    owner = %self.context.principal_id().as_str(), organization = %self.context.organization_id().as_str(),
                    gateway = %self.gateway.id().as_str(), "owner phone purpose refused");
                Err(ReadRefusal::WrongOwner)
            }
            Err(error) => {
                tracing::error!(outcome = "policy_error", operation = operation_name, ?method, ?method_bytes, method_truncated, ?error,
                    credential = %self.context.credential_id().as_str(), receiver = %self.receiver.receiver_id,
                    owner = %self.context.principal_id().as_str(), organization = %self.context.organization_id().as_str(),
                    gateway = %self.gateway.id().as_str(), "owner phone purpose could not be decided");
                Err(ReadRefusal::Unverifiable)
            }
        }
    }
}
impl std::fmt::Debug for OwnerPhoneReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerPhoneReader")
            .field("context", &self.context)
            .field("receiver", &self.receiver)
            .finish_non_exhaustive()
    }
}
impl PartialEq for OwnerPhoneReader {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context
            && self.receiver == other.receiver
            && self.gateway == other.gateway
            && self.purpose.credential() == other.purpose.credential()
            && self.purpose.receiver() == other.purpose.receiver()
            && self.purpose.paired_epoch() == other.purpose.paired_epoch()
            && self.purpose.owner() == other.purpose.owner()
            && self.purpose.organization() == other.purpose.organization()
            && self.purpose.membership() == other.purpose.membership()
            && self.purpose.audience() == other.purpose.audience()
            && Arc::ptr_eq(&self.policy, &other.policy)
            && Arc::ptr_eq(&self.access_policy, &other.access_policy)
    }
}
impl Eq for OwnerPhoneReader {}

/// Canonical classification and original current holding precede constructing
/// a reader. Availability of a listener/source is never classification evidence.
pub struct AdmitOwnerPhoneReader<'a> {
    pub purposes: &'a dyn OwnerPhonePurposeSource,
    pub holding: Option<&'a dyn OwnerPhoneReceiverAuthority>,
    pub clock: &'a dyn Clock,
    pub gateway: &'a Resource,
    pub policy: Arc<dyn OwnerPhonePolicyEvaluator>,
    pub access_policy: Option<Arc<dyn ConversationAccessPolicyEvaluator>>,
}
impl AdmitOwnerPhoneReader<'_> {
    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
    ) -> Result<Option<OwnerPhoneReader>, ReadRefusal> {
        let Some(purpose) = self
            .purposes
            .owner_phone_purpose(session, self.clock)
            .await
            .map_err(super::passive_read::access_refusal)?
        else {
            return Ok(None);
        };
        // Availability is not first-party classification. An ordinary session
        // above returns None without needing the phone's access policy owner.
        let access_policy = self
            .access_policy
            .as_ref()
            .ok_or(ReadRefusal::Unverifiable)?;
        let receiver = self
            .holding
            .ok_or(ReadRefusal::Unverifiable)?
            .holding(&purpose)
            .await?
            .ok_or(ReadRefusal::Unauthorized)?;
        let reader = OwnerPhoneReader::new(
            session.context().clone(),
            purpose,
            receiver,
            self.gateway.clone(),
            self.policy.clone(),
            access_policy.clone(),
        );
        // Canonical linkage Cedar must pass before this delegate may expose the
        // shared policy's first-party Person subject. Holding remains its owner.
        reader.admit_operation(OwnerPhoneOperation::Linkage)?;
        Ok(Some(reader))
    }
}

/// Current factual holding of the original Pair receipt. The receiver journal
/// owns this identity/progress relation, including intervening revocations;
/// resolving only the latest credential binding is insufficient after regrant.
pub trait OwnerPhoneReceiverAuthority: Send + Sync {
    fn holding<'a>(
        &'a self,
        purpose: &'a OwnerPhonePurpose,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>;
}

/// The operation whose policy is being asked. These variants name factual
/// application operations, not a Rust access verdict or a grant scan.
pub enum OwnerPhoneOperation<'a> {
    /// Current source/session/receiver linkage before exposing a Person subject.
    Linkage,
    /// A product request before its handler; the action is the published
    /// manifest mapping, including None for an unknown method.
    ProductMethod {
        method: &'a str,
        published_action: Option<&'a str>,
    },
    /// Parsed creation selection and the service-owned default, before effects.
    Creation {
        requested_mode: Option<&'a str>,
        default_mode: &'a str,
    },
    /// Count of canonically decoded owner-local path links before submission.
    SubmissionInput { linked_files: usize },
}

impl OwnerPhoneOperation<'_> {
    fn name(&self) -> &'static str {
        match self {
            Self::Linkage => "phone.context",
            Self::ProductMethod { .. } => "product.request",
            Self::Creation { .. } => "conversation.create",
            Self::SubmissionInput { .. } => "conversation.input",
        }
    }
}

/// Related current facts. Auth supplies the unforgeable purpose; the receiver
/// authority supplies current holding and epoch, and the target repository owns
/// metadata. Application identity checks must precede asking this policy port.
pub struct OwnerPhonePolicyFacts<'a> {
    /// Composition-resolved gateway, independent of requested target metadata.
    pub gateway: &'a Resource,
    pub context: &'a AuthContext,
    pub purpose: &'a OwnerPhonePurpose,
    pub receiver: &'a ReceiverBinding,
    pub operation: OwnerPhoneOperation<'a>,
}

/// Typed policy evidence. The adapter retains matched @id rules, or an empty
/// collection for Cedar's default denial; diagnostics never become authority.
pub enum OwnerPhoneDecision {
    Allow { rules: Vec<String> },
    Deny { rules: Vec<String> },
}

/// Cedar owns delegated purpose, method and input restrictions. This is additional to coarse Auth grant
/// admission, not a replacement for it or for a command's target admission.
/// Invalid policy data or evaluator failure cannot produce an Allow.
pub trait OwnerPhonePolicyEvaluator: Send + Sync {
    fn evaluate(&self, facts: OwnerPhonePolicyFacts<'_>)
        -> Result<OwnerPhoneDecision, AccessError>;
}
