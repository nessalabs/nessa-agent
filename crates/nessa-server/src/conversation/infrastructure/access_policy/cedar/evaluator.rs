//! The conversation access bundle, loaded and validated once, and each
//! question decided by it.
use crate::conversation::application::{
    AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    ConversationAccessPolicyEvaluator, RequestShape, ShareTarget,
};
use crate::conversation::domain::Capabilities;
use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid, PolicyId,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use nessa_auth::domain::MembershipRole;
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

const SCHEMA: &str = include_str!("schema.json");
/// The bundle, one file per kind of party: owners, sharing, paired readers.
const POLICIES: [&str; 3] = [
    include_str!("ownership.cedar"),
    include_str!("shares.cedar"),
    include_str!("readers.cedar"),
];

/// The checked-in conversation access policy, strictly validated.
pub struct CedarConversationAccessPolicy {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

/// A step of building a request or its entities, or of evaluating it, that
/// failed, and how: the access is denied, and this logged.
struct Unavailable(String);

impl Unavailable {
    /// `step` failed with `error`.
    fn at(step: &str, error: impl std::fmt::Display) -> Self {
        Self(format!("{step}: {error}"))
    }
}

impl CedarConversationAccessPolicy {
    /// Immutable inputs to the composed receiver read-policy fingerprint.
    /// The receiver profile owner hashes these exact enforcing sources; no
    /// caller copies this bundle or derives authorization from the digest.
    pub fn profile_sources() -> [&'static str; 4] {
        [SCHEMA, POLICIES[0], POLICIES[1], POLICIES[2]]
    }

    /// Load and strictly validate the checked-in bundle.
    ///
    /// # Errors
    /// The bundle does not parse, does not validate without warnings, or
    /// holds a policy without an `@id` or two under the same one; the
    /// message says which.
    pub fn new() -> Result<Self, String> {
        Self::from_sources(SCHEMA, &POLICIES.join("\n"))
    }

    fn from_sources(schema: &str, policies: &str) -> Result<Self, String> {
        let schema = Schema::from_json_str(schema).map_err(|error| {
            format!("the conversation access policy schema does not parse: {error}")
        })?;
        let parsed = PolicySet::from_str(policies)
            .map_err(|error| format!("the conversation access policies do not parse: {error}"))?;
        // Each policy is re-identified by its `@id`, so a decision's reasons
        // are the names the bundle gives and a repeated name fails `add`.
        let mut policies = PolicySet::new();
        for policy in parsed.policies() {
            let Some(id) = policy.annotation("id").filter(|id| !id.is_empty()) else {
                return Err(format!(
                    "a conversation access policy has no @id: {}",
                    policy.id()
                ));
            };
            policies
                .add(policy.new_id(PolicyId::new(id)))
                .map_err(|error| {
                    format!("the conversation access policy @id {id:?} is not unique: {error}")
                })?;
        }
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            let found: Vec<String> = validation
                .validation_errors()
                .map(ToString::to_string)
                .chain(validation.validation_warnings().map(ToString::to_string))
                .collect();
            return Err(format!(
                "the conversation access policies do not validate: {}",
                found.join("; ")
            ));
        }
        Ok(Self {
            schema,
            policies,
            authorizer: Authorizer::new(),
        })
    }

    fn decide(&self, ask: &AccessAsk<'_>) -> Result<AccessDecision, Unavailable> {
        let principal = subject(&ask.subject)?;
        let resource = resource(&ask.resource)?;
        let action = uid("NessaConversation::Action", ask.action.name())?;
        let context = Context::from_pairs(context(ask)?)
            .map_err(|error| Unavailable::at("the context", error))?;
        let request = Request::new(
            principal.uid(),
            action,
            resource.uid(),
            context,
            Some(&self.schema),
        )
        .map_err(|error| Unavailable::at("the request", error))?;
        let entities = Entities::from_entities([principal, resource], Some(&self.schema))
            .map_err(|error| Unavailable::at("the entities", error))?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if let Some(error) = response.diagnostics().errors().next() {
            return Err(Unavailable::at("evaluation", error));
        }
        let policies: Vec<String> = response
            .diagnostics()
            .reason()
            .filter_map(|id| self.policies.annotation(id, "id"))
            .map(str::to_owned)
            .collect();
        Ok(match response.decision() {
            Decision::Allow => AccessDecision::Allow { policies },
            // Cedar denies by a forbid it names, or by no permit at all.
            Decision::Deny if policies.is_empty() => {
                AccessDecision::Deny(AccessDenial::NotPermitted)
            }
            Decision::Deny => AccessDecision::Deny(AccessDenial::Forbidden { policies }),
        })
    }
}

impl ConversationAccessPolicyEvaluator for CedarConversationAccessPolicy {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        self.decide(ask).unwrap_or_else(|Unavailable(error)| {
            tracing::error!(
                %error,
                action = ask.action.name(),
                "the conversation access policy could not decide; access is refused"
            );
            AccessDecision::Deny(AccessDenial::Unavailable)
        })
    }
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, Unavailable> {
    let entity_type = EntityTypeName::from_str(entity_type)
        .map_err(|error| Unavailable::at("an entity type", error))?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

fn string(value: &str) -> RestrictedExpression {
    RestrictedExpression::new_string(value.to_owned())
}

fn entity(
    entity_type: &str,
    id: &str,
    attributes: impl IntoIterator<Item = (&'static str, RestrictedExpression)>,
) -> Result<Entity, Unavailable> {
    Entity::new(
        uid(entity_type, id)?,
        attributes
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<HashMap<_, _>>(),
        HashSet::new(),
    )
    .map_err(|error| Unavailable::at(entity_type, error))
}

/// Who asks, as the schema's person, review recipient, reader or grant holder.
fn subject(subject: &AccessSubject<'_>) -> Result<Entity, Unavailable> {
    match *subject {
        AccessSubject::AgentTurn {
            organization,
            principal,
            owner_initiated,
        } => entity(
            "NessaConversation::AgentTurn",
            principal.as_str(),
            [
                ("organization", string(organization.as_str())),
                ("id", string(principal.as_str())),
                (
                    "ownerInitiated",
                    RestrictedExpression::new_bool(owner_initiated),
                ),
            ],
        ),
        AccessSubject::Person {
            organization,
            principal,
        } => entity(
            "NessaConversation::Person",
            principal.as_str(),
            [
                ("organization", string(organization.as_str())),
                ("id", string(principal.as_str())),
            ],
        ),
        AccessSubject::ReviewRecipient {
            organization,
            principal,
            membership,
        } => entity(
            "NessaConversation::ReviewRecipient",
            principal.as_str(),
            [
                ("organization", string(organization.as_str())),
                ("id", string(principal.as_str())),
                (
                    "membership",
                    string(match membership {
                        MembershipRole::Admin => "admin",
                        MembershipRole::Member => "member",
                    }),
                ),
            ],
        ),
        AccessSubject::PairedReader {
            organization,
            signed_in_as,
            binding_owner,
            receiver,
        } => entity(
            "NessaConversation::Reader",
            receiver,
            [
                ("organization", string(organization.as_str())),
                ("signedInAs", string(signed_in_as.as_str())),
                ("bindingOwner", string(binding_owner.as_str())),
                ("receiver", string(receiver)),
            ],
        ),
        AccessSubject::GrantHolder { receiver } => entity(
            "NessaConversation::GrantHolder",
            receiver,
            [("receiver", string(receiver))],
        ),
    }
}

/// What is asked about, as the schema's `Conversation`, `Catalogue` or `Workspace`.
fn resource(resource: &AccessResource<'_>) -> Result<Entity, Unavailable> {
    match *resource {
        AccessResource::Conversation {
            organization,
            owner,
            deleted,
        } => entity(
            "NessaConversation::Conversation",
            "conversation",
            [
                ("organization", string(organization.as_str())),
                ("owner", string(owner.as_str())),
                ("deleted", RestrictedExpression::new_bool(deleted)),
            ],
        ),
        AccessResource::Catalogue {
            organization,
            owner,
        } => entity(
            "NessaConversation::Catalogue",
            "catalogue",
            [
                ("organization", string(organization.as_str())),
                ("owner", string(owner.as_str())),
            ],
        ),
        AccessResource::Workspace {
            organization,
            owner,
        } => entity(
            "NessaConversation::Workspace",
            "workspace",
            [
                ("organization", string(organization.as_str())),
                ("owner", string(owner.as_str())),
            ],
        ),
    }
}

/// The capability set as the policy reads it.
fn capability_set(capabilities: Capabilities) -> Result<RestrictedExpression, Unavailable> {
    let names = capabilities.names().map(string).collect::<Vec<_>>();
    Ok(RestrictedExpression::new_set(names))
}

fn record(
    fields: impl IntoIterator<Item = (&'static str, RestrictedExpression)>,
) -> Result<RestrictedExpression, Unavailable> {
    RestrictedExpression::new_record(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value)),
    )
    .map_err(|error| Unavailable::at("a context record", error))
}

fn request_shape(request: &RequestShape<'_>) -> Result<RestrictedExpression, Unavailable> {
    let mut fields = vec![
        (
            "attachments",
            RestrictedExpression::new_bool(request.attachments),
        ),
        ("files", RestrictedExpression::new_bool(request.files)),
    ];
    if let Some(operation) = request.receipt_operation {
        fields.push(("receiptOperation", string(operation)));
    }
    record(fields)
}

fn share_target(target: &ShareTarget<'_>) -> Result<RestrictedExpression, Unavailable> {
    let grants = target
        .grants
        .iter()
        .map(|grant| {
            record([
                ("action", string(grant.action().as_str())),
                (
                    "organization",
                    string(grant.resource().organization_id().as_str()),
                ),
                ("resource", string(grant.resource().id().as_str())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let gateway = record([
        (
            "organization",
            string(target.gateway.organization_id().as_str()),
        ),
        ("resource", string(target.gateway.id().as_str())),
    ])?;
    record([
        ("organization", string(target.organization.as_str())),
        ("bindingOwner", string(target.binding_owner.as_str())),
        ("grants", RestrictedExpression::new_set(grants)),
        ("gateway", gateway),
    ])
}

/// The schema's `Ask` context: the capabilities held, the request's shape,
/// and a share's target, each only as present.
fn context(ask: &AccessAsk<'_>) -> Result<Vec<(String, RestrictedExpression)>, Unavailable> {
    let mut pairs = vec![("request".to_owned(), request_shape(&ask.request)?)];
    if let Some(capabilities) = ask.held {
        pairs.push(("capabilities".to_owned(), capability_set(capabilities)?));
    }
    if let Some(target) = &ask.target {
        pairs.push(("target".to_owned(), share_target(target)?));
    }
    Ok(pairs)
}

#[cfg(test)]
#[path = "../../../../../tests/conversation/access_policy.rs"]
mod tests;
