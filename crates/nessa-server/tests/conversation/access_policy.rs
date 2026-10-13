//! Conversation access as Cedar decides it: the checked-in bundle
//! validates, every rule allows what it names and nothing past it, a
//! refusal says which rule made it, anything the adapter cannot decide is
//! refused, and each rule answers exactly as the Rust it replaces did,
//! across every combination of who asks, whose conversation it is, whether
//! it was deleted, the role held and what a request carries.
use super::*;
use crate::conversation::application::{AccessAction, OwnerAccess};
use crate::conversation::domain::{Conversation, ConversationDeletion, ConversationRefusal};
use nessa_auth::domain::{Action, Grant, OrganizationId, PrincipalId, Resource, ResourceId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};

fn policy() -> CedarConversationAccessPolicy {
    CedarConversationAccessPolicy::new().expect("the checked-in access policy validates")
}

fn organization(name: &str) -> OrganizationId {
    OrganizationId::new(name).unwrap()
}

fn principal(name: &str) -> PrincipalId {
    PrincipalId::new(name).unwrap()
}

/// `owner`'s conversation in `org`, deleted or not.
fn conversation(org: &str, owner: &str, deleted: bool) -> Conversation {
    let live = Conversation::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        organization(org),
        principal(owner),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    if deleted {
        live.deleted(
            ConversationDeletion::new(
                organization(org),
                principal(owner),
                "panel".into(),
                "delete".into(),
                2,
            )
            .unwrap(),
        )
        .unwrap()
    } else {
        live
    }
}

fn allowed_by(decision: &AccessDecision) -> Option<&[String]> {
    match decision {
        AccessDecision::Allow { policies } => Some(policies),
        AccessDecision::Deny(_) => None,
    }
}

fn allowed(decision: &AccessDecision) -> bool {
    allowed_by(decision).is_some()
}

/// A person's `action` on `conversation`.
fn person(
    policy: &CedarConversationAccessPolicy,
    action: AccessAction,
    conversation: &Conversation,
    org: &str,
    who: &str,
) -> AccessDecision {
    let (org, who) = (organization(org), principal(who));
    policy.evaluate(&AccessAsk::person(action, conversation, &org, &who))
}

/// A paired reader's question, with every fact a test may vary.
struct Paired<'a> {
    org: &'a str,
    signed_in_as: &'a str,
    binding_owner: &'a str,
    held: Option<ShareRole>,
    request: RequestShape<'a>,
}

impl Default for Paired<'_> {
    /// A device of `owner` in `org` holding a Read grant, sending text.
    fn default() -> Self {
        Self {
            org: "org",
            signed_in_as: "owner",
            binding_owner: "owner",
            held: Some(ShareRole::Read),
            request: RequestShape::default(),
        }
    }
}

fn paired(
    policy: &CedarConversationAccessPolicy,
    action: AccessAction,
    resource: AccessResource<'_>,
    facts: &Paired<'_>,
) -> AccessDecision {
    let (org, signed_in_as, binding_owner) = (
        organization(facts.org),
        principal(facts.signed_in_as),
        principal(facts.binding_owner),
    );
    policy.evaluate(&AccessAsk {
        subject: AccessSubject::PairedReader {
            organization: &org,
            signed_in_as: &signed_in_as,
            binding_owner: &binding_owner,
            receiver: "receiver-1",
        },
        action,
        resource,
        held: facts.held,
        request: facts.request,
        target: None,
    })
}

fn on(conversation: &Conversation) -> AccessResource<'_> {
    AccessResource::of(conversation)
}

fn gateway() -> Resource {
    Resource::new(organization("org"), ResourceId::new("gateway").unwrap())
}

fn grant(action: &str, resource: Resource) -> Grant {
    Grant::new(Action::new(action).unwrap(), resource)
}

/// The owner's share of `conversation` with a target bound in `target_org`
/// to `target_owner`, holding `grants`.
fn share(
    policy: &CedarConversationAccessPolicy,
    conversation: &Conversation,
    who: &str,
    target_org: &str,
    target_owner: &str,
    grants: &[Grant],
) -> AccessDecision {
    let (org, who) = (organization("org"), principal(who));
    let (target_org, target_owner, gateway) =
        (organization(target_org), principal(target_owner), gateway());
    policy.evaluate(&AccessAsk {
        target: Some(ShareTarget {
            organization: &target_org,
            binding_owner: &target_owner,
            grants,
            gateway: &gateway,
        }),
        ..AccessAsk::person(AccessAction::Share, conversation, &org, &who)
    })
}

#[test]
fn the_checked_in_bundle_validates_and_names_every_action() {
    let policy = policy();
    // Every action the port names is one the schema declares: a request for
    // it is decided, never refused as unavailable.
    let live = conversation("org", "owner", false);
    let (org, owner) = (organization("org"), principal("owner"));
    for action in AccessAction::ALL {
        let resource = match action {
            AccessAction::List | AccessAction::CatalogueWatch => AccessResource::Catalogue {
                organization: &org,
                owner: &owner,
            },
            _ => on(&live),
        };
        let subject = match action {
            AccessAction::NoteDeliver => AccessSubject::GrantHolder {
                receiver: "receiver-1",
            },
            AccessAction::CatalogueEntry
            | AccessAction::Comment
            | AccessAction::Drive
            | AccessAction::Write
            | AccessAction::CatalogueWatch => AccessSubject::PairedReader {
                organization: &org,
                signed_in_as: &owner,
                binding_owner: &owner,
                receiver: "receiver-1",
            },
            _ => AccessSubject::Person {
                organization: &org,
                principal: &owner,
            },
        };
        let decision = policy.evaluate(&AccessAsk {
            subject,
            action,
            resource,
            held: None,
            request: RequestShape::default(),
            target: None,
        });
        assert_ne!(
            decision,
            AccessDecision::Deny(AccessDenial::Unavailable),
            "{}",
            action.name()
        );
    }
}

// One allow and one deny for each rule, by its @id.

#[test]
fn owner_locate_tells_the_owner_about_a_deleted_conversation_and_nobody_else() {
    let policy = policy();
    let deleted = conversation("org", "owner", true);
    assert_eq!(
        allowed_by(&person(
            &policy,
            AccessAction::Locate,
            &deleted,
            "org",
            "owner"
        )),
        Some(&["owner.locate".to_owned()][..])
    );
    assert_eq!(
        person(&policy, AccessAction::Locate, &deleted, "org", "other"),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
}

#[test]
fn owner_act_admits_the_owner_until_the_conversation_is_deleted() {
    let policy = policy();
    assert_eq!(
        allowed_by(&person(
            &policy,
            AccessAction::Act,
            &conversation("org", "owner", false),
            "org",
            "owner"
        )),
        Some(&["owner.act".to_owned()][..])
    );
    assert!(!allowed(&person(
        &policy,
        AccessAction::Act,
        &conversation("org", "owner", true),
        "org",
        "owner"
    )));
}

#[test]
fn owner_read_and_artifact_read_admit_the_owner_until_deleted() {
    let policy = policy();
    for (action, id) in [
        (AccessAction::Read, "owner.read"),
        (AccessAction::ArtifactRead, "owner.artifact_read"),
    ] {
        assert_eq!(
            allowed_by(&person(
                &policy,
                action,
                &conversation("org", "owner", false),
                "org",
                "owner"
            )),
            Some(&[id.to_owned()][..])
        );
        assert!(!allowed(&person(
            &policy,
            action,
            &conversation("org", "owner", true),
            "org",
            "owner"
        )));
        assert!(!allowed(&person(
            &policy,
            action,
            &conversation("org", "owner", false),
            "elsewhere",
            "owner"
        )));
    }
}

#[test]
fn owner_list_is_the_owners_own_catalogue_and_never_a_paired_readers() {
    let policy = policy();
    let (org, owner, other) = (organization("org"), principal("owner"), principal("other"));
    let catalogue = AccessResource::Catalogue {
        organization: &org,
        owner: &owner,
    };
    let list = |principal: &PrincipalId| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::Person {
                organization: &org,
                principal,
            },
            action: AccessAction::List,
            resource: catalogue,
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    assert_eq!(
        allowed_by(&list(&owner)),
        Some(&["owner.list".to_owned()][..])
    );
    assert!(!allowed(&list(&other)));
    assert!(!allowed(&paired(
        &policy,
        AccessAction::List,
        catalogue,
        &Paired::default()
    )));
}

#[test]
fn owner_unshare_and_shares_need_ownership_alone() {
    let policy = policy();
    for (action, id) in [
        (AccessAction::Unshare, "owner.unshare"),
        (AccessAction::Shares, "owner.shares"),
    ] {
        assert_eq!(
            allowed_by(&person(
                &policy,
                action,
                &conversation("org", "owner", true),
                "org",
                "owner"
            )),
            Some(&[id.to_owned()][..])
        );
        assert!(!allowed(&person(
            &policy,
            action,
            &conversation("org", "owner", false),
            "org",
            "other"
        )));
        assert!(!allowed(&paired(
            &policy,
            action,
            on(&conversation("org", "owner", false)),
            &Paired::default()
        )));
    }
}

#[test]
fn owner_share_needs_a_reader_paired_by_the_owner() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    let reads = [grant("conversation.read", gateway())];
    assert_eq!(
        allowed_by(&share(&policy, &live, "owner", "org", "owner", &reads)),
        Some(&["owner.share".to_owned()][..])
    );
    assert!(!allowed(&share(
        &policy,
        &live,
        "owner",
        "org",
        "owner",
        &[grant("environment.serve", gateway())]
    )));
    // No target at all is never a share.
    assert!(!allowed(&person(
        &policy,
        AccessAction::Share,
        &live,
        "org",
        "owner"
    )));
}

#[test]
fn grant_read_reads_its_grantors_live_conversation_with_any_role() {
    let policy = policy();
    assert_eq!(
        allowed_by(&paired(
            &policy,
            AccessAction::Read,
            on(&conversation("org", "owner", false)),
            &Paired::default()
        )),
        Some(&["grant.read".to_owned()][..])
    );
    assert!(!allowed(&paired(
        &policy,
        AccessAction::Read,
        on(&conversation("org", "owner", false)),
        &Paired {
            held: None,
            ..Paired::default()
        }
    )));
}

#[test]
fn grant_catalogue_entry_shows_a_granted_tombstone() {
    let policy = policy();
    assert_eq!(
        allowed_by(&paired(
            &policy,
            AccessAction::CatalogueEntry,
            on(&conversation("org", "owner", true)),
            &Paired::default()
        )),
        Some(&["grant.catalogue_entry".to_owned()][..])
    );
    assert!(!allowed(&paired(
        &policy,
        AccessAction::CatalogueEntry,
        on(&conversation("org", "other", false)),
        &Paired::default()
    )));
}

#[test]
fn grant_comment_and_drive_need_their_role() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    for (action, role, id) in [
        (AccessAction::Comment, ShareRole::Comment, "grant.comment"),
        (AccessAction::Drive, ShareRole::Drive, "grant.drive"),
    ] {
        assert_eq!(
            allowed_by(&paired(
                &policy,
                action,
                on(&live),
                &Paired {
                    held: Some(role),
                    ..Paired::default()
                }
            )),
            Some(&[id.to_owned()][..])
        );
        assert_eq!(
            paired(&policy, action, on(&live), &Paired::default()),
            AccessDecision::Deny(AccessDenial::NotPermitted)
        );
    }
}

#[test]
fn paired_peer_gateway_never_comments_or_drives() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    for action in [AccessAction::Comment, AccessAction::Drive] {
        let device = Paired {
            held: Some(ShareRole::Drive),
            ..Paired::default()
        };
        assert!(allowed(&paired(&policy, action, on(&live), &device)));
        assert_eq!(
            paired(
                &policy,
                action,
                on(&live),
                &Paired {
                    signed_in_as: "gateway:peer",
                    ..device
                }
            ),
            AccessDecision::Deny(AccessDenial::Forbidden {
                policies: vec!["paired.peer_gateway".into()]
            })
        );
    }
    // A peer still reads what it was granted.
    assert!(allowed(&paired(
        &policy,
        AccessAction::Read,
        on(&live),
        &Paired {
            signed_in_as: "gateway:peer",
            ..Paired::default()
        }
    )));
}

#[test]
fn drive_text_only_refuses_what_a_driver_could_name_unshared() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    let driver = Paired {
        held: Some(ShareRole::Drive),
        ..Paired::default()
    };
    assert!(allowed(&paired(
        &policy,
        AccessAction::Drive,
        on(&live),
        &driver
    )));
    assert_eq!(
        paired(
            &policy,
            AccessAction::Drive,
            on(&live),
            &Paired {
                request: RequestShape {
                    files: true,
                    ..RequestShape::default()
                },
                ..driver
            }
        ),
        AccessDecision::Deny(AccessDenial::Forbidden {
            policies: vec!["drive.text_only".into()]
        })
    );
}

#[test]
fn grant_catalogue_watch_is_a_devices_and_never_a_peers() {
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let catalogue = AccessResource::Catalogue {
        organization: &org,
        owner: &owner,
    };
    assert_eq!(
        allowed_by(&paired(
            &policy,
            AccessAction::CatalogueWatch,
            catalogue,
            &Paired::default()
        )),
        Some(&["grant.catalogue_watch".to_owned()][..])
    );
    assert!(!allowed(&paired(
        &policy,
        AccessAction::CatalogueWatch,
        catalogue,
        &Paired {
            signed_in_as: "gateway:peer",
            ..Paired::default()
        }
    )));
}

#[test]
fn grant_note_deliver_needs_comment_or_drive_on_a_live_conversation() {
    let policy = policy();
    let deliver = |held: Option<ShareRole>, deleted: bool| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::GrantHolder {
                receiver: "receiver-1",
            },
            action: AccessAction::NoteDeliver,
            resource: on(&conversation("org", "owner", deleted)),
            held,
            request: RequestShape::default(),
            target: None,
        })
    };
    assert_eq!(
        allowed_by(&deliver(Some(ShareRole::Comment), false)),
        Some(&["grant.note_deliver".to_owned()][..])
    );
    assert!(!allowed(&deliver(Some(ShareRole::Read), false)));
    assert!(!allowed(&deliver(Some(ShareRole::Drive), true)));
    assert!(!allowed(&deliver(None, false)));
}

#[test]
fn a_paired_reader_writes_nothing_else() {
    let policy = policy();
    assert_eq!(
        paired(
            &policy,
            AccessAction::Write,
            on(&conversation("org", "owner", false)),
            &Paired {
                held: Some(ShareRole::Drive),
                ..Paired::default()
            }
        ),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
}

// The bundle as built.

/// A bundle that does not parse, does not validate strictly, or holds a
/// policy without an `@id` is refused when it is built.
#[test]
fn a_malformed_bundle_fails_construction() {
    let policies = POLICIES.join("\n");
    assert!(CedarConversationAccessPolicy::from_sources("{", &policies).is_err());
    assert!(CedarConversationAccessPolicy::from_sources(SCHEMA, "permit (").is_err());
    let unknown_attribute = r#"@id("x") permit (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource) when { principal.nope };"#;
    let strict = CedarConversationAccessPolicy::from_sources(SCHEMA, unknown_attribute);
    assert!(strict.is_err_and(|error| error.contains("do not validate")));
    let unnamed = r#"permit (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource is NessaConversation::Conversation);"#;
    let missing = CedarConversationAccessPolicy::from_sources(SCHEMA, unnamed);
    assert!(missing.is_err_and(|error| error.contains("no @id")));
    let blank = r#"@id("") permit (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource is NessaConversation::Conversation);"#;
    assert!(CedarConversationAccessPolicy::from_sources(SCHEMA, blank).is_err());
}

/// Entities the schema does not accept are refused as unavailable, never
/// taken for a policy's no.
#[test]
fn an_entity_the_schema_refuses_fails_closed() {
    // A schema whose `Person.organization` is a number: every person the
    // adapter builds is refused by it, while the policy still validates.
    let mut schema: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    schema["NessaConversation"]["entityTypes"]["Person"]["shape"]["attributes"]["organization"]
        ["type"] = "Long".into();
    let schema = schema.to_string();
    let bundle = r#"@id("allow") permit (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource is NessaConversation::Conversation);"#;
    let policy = CedarConversationAccessPolicy::from_sources(&schema, bundle).expect("validates");
    assert_eq!(
        person(
            &policy,
            AccessAction::Act,
            &conversation("org", "owner", false),
            "org",
            "owner"
        ),
        AccessDecision::Deny(AccessDenial::Unavailable)
    );
}

/// A rule that errors while it is evaluated refuses as unavailable, even
/// where Cedar would skip it and allow.
#[test]
fn an_evaluation_error_fails_closed() {
    let bundle = r#"
        @id("allow")
        permit (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource is NessaConversation::Conversation);
        @id("overflows")
        forbid (principal, action, resource)
        when { 9223372036854775807 + 1 > 0 };
    "#;
    let policy = CedarConversationAccessPolicy::from_sources(SCHEMA, bundle).expect("validates");
    assert_eq!(
        person(
            &policy,
            AccessAction::Act,
            &conversation("org", "owner", false),
            "org",
            "owner"
        ),
        AccessDecision::Deny(AccessDenial::Unavailable)
    );
}

// Equivalence with the Rust each rule replaces.

const PEOPLE: [(&str, &str); 3] = [("org", "owner"), ("org", "other"), ("elsewhere", "owner")];
const ROLES: [Option<ShareRole>; 4] = [
    None,
    Some(ShareRole::Read),
    Some(ShareRole::Comment),
    Some(ShareRole::Drive),
];

/// `Conversation::check_access` and `OwnerAccess::admit_to` (act, then
/// locate on a refusal) agree for every caller and deletion: only the owner
/// learns of a deletion.
#[test]
fn owner_access_answers_as_check_access_did() {
    let policy = policy();
    let access = OwnerAccess::new(&policy);
    for deleted in [false, true] {
        let record = conversation("org", "owner", deleted);
        for (org, who) in PEOPLE {
            let (org, who) = (organization(org), principal(who));
            assert_eq!(
                access.admit_to(AccessAction::Act, &record, &org, &who),
                record.check_access(&org, &who),
                "{org:?} {who:?} deleted={deleted}"
            );
            assert_eq!(
                access.locates(&record, &org, &who),
                Ok(record.allows(&org, &who)),
            );
        }
    }
}

/// An evaluator that cannot decide makes every owner admission unavailable.
#[test]
fn owner_access_the_policy_cannot_decide_is_unavailable() {
    struct Undecided;
    impl ConversationAccessPolicyEvaluator for Undecided {
        fn evaluate(&self, _: &AccessAsk<'_>) -> AccessDecision {
            AccessDecision::Deny(AccessDenial::Unavailable)
        }
    }
    let record = conversation("org", "owner", false);
    let access = OwnerAccess::new(&Undecided);
    let (org, owner) = (organization("org"), principal("owner"));
    assert_eq!(
        access.admit_to(AccessAction::Act, &record, &org, &owner),
        Err(ConversationRefusal::Unavailable)
    );
    assert_eq!(
        access.locates(&record, &org, &owner),
        Err(ConversationRefusal::Unavailable)
    );
}

/// What a paired reader may do, as `admit_read`, `admit_role` with
/// `ShareRole::at_least`, `passive_read`'s ownership check and
/// `read_access`'s peer-gateway refusal decided it, for every binding,
/// signer, deletion and role.
#[test]
fn paired_access_answers_as_the_grant_checks_did() {
    let policy = policy();
    for deleted in [false, true] {
        let record = conversation("org", "owner", deleted);
        for (org, binding_owner) in PEOPLE {
            for signed_in_as in [binding_owner, "gateway:peer"] {
                for held in ROLES {
                    let facts = Paired {
                        org,
                        signed_in_as,
                        binding_owner,
                        held,
                        request: RequestShape::default(),
                    };
                    let (borg, bowner) = (organization(org), principal(binding_owner));
                    // `AdmitPassiveRead::execute`: the binding's owner's,
                    // not deleted, and granted with any role.
                    let owned = record.check_access(&borg, &bowner).is_ok();
                    assert_eq!(
                        allowed(&paired(&policy, AccessAction::Read, on(&record), &facts)),
                        owned && held.is_some(),
                        "read {org} {binding_owner} {signed_in_as} {held:?} deleted={deleted}"
                    );
                    // `admit_request`: a peer gateway is refused; the role
                    // is `admit_role`'s `at_least`; the service then asks
                    // the owner's ownership.
                    let device = signed_in_as == binding_owner;
                    for (action, role) in [
                        (AccessAction::Comment, ShareRole::Comment),
                        (AccessAction::Drive, ShareRole::Drive),
                    ] {
                        assert_eq!(
                            allowed(&paired(&policy, action, on(&record), &facts)),
                            device && owned && held.is_some_and(|held| held.at_least(role)),
                            "{} {org} {binding_owner} {signed_in_as} {held:?} deleted={deleted}",
                            action.name()
                        );
                    }
                    // The catalogue's grant predicate: granted, and the
                    // binding owner's rows, tombstones included.
                    assert_eq!(
                        allowed(&paired(
                            &policy,
                            AccessAction::CatalogueEntry,
                            on(&record),
                            &facts
                        )),
                        record.allows(&borg, &bowner) && held.is_some(),
                    );
                    // `admit_list` and the share commands refuse a paired
                    // device by what it is.
                    for action in [
                        AccessAction::Write,
                        AccessAction::Share,
                        AccessAction::Unshare,
                        AccessAction::Shares,
                    ] {
                        assert!(!allowed(&paired(&policy, action, on(&record), &facts)));
                    }
                    // `admit_current`: a catalogue watch is refused unless
                    // the binding's owner is who signed in.
                    let catalogue = AccessResource::Catalogue {
                        organization: &borg,
                        owner: &bowner,
                    };
                    assert_eq!(
                        allowed(&paired(
                            &policy,
                            AccessAction::CatalogueWatch,
                            catalogue,
                            &facts
                        )),
                        device
                    );
                }
            }
        }
    }
}

/// `product::read_access::drives_only_text`, as its truth table: it is
/// private to the product module, so its rule is restated here from the
/// code: a Drive command naming attachments or files, or a receipt lookup
/// for `create`, is refused, and anything else is let through.
#[test]
fn drive_text_only_answers_as_drives_only_text_did() {
    fn drives_only_text(attachments: bool, files: bool, receipt: Option<&str>) -> bool {
        !(attachments || files || receipt == Some("create"))
    }
    let policy = policy();
    let live = conversation("org", "owner", false);
    for attachments in [false, true] {
        for files in [false, true] {
            for receipt_operation in [None, Some("create"), Some("send"), Some("stop")] {
                let decision = paired(
                    &policy,
                    AccessAction::Drive,
                    on(&live),
                    &Paired {
                        held: Some(ShareRole::Drive),
                        request: RequestShape {
                            attachments,
                            files,
                            receipt_operation,
                        },
                        ..Paired::default()
                    },
                );
                assert_eq!(
                    allowed(&decision),
                    drives_only_text(attachments, files, receipt_operation),
                    "{attachments} {files} {receipt_operation:?}"
                );
                // A note carries no attachments rule: Comment ignores them.
                let comment = paired(
                    &policy,
                    AccessAction::Comment,
                    on(&live),
                    &Paired {
                        held: Some(ShareRole::Comment),
                        request: RequestShape {
                            attachments,
                            files,
                            receipt_operation,
                        },
                        ..Paired::default()
                    },
                );
                assert!(allowed(&comment));
            }
        }
    }
}

/// `ShareConversation::share`'s target check and the store's in-transaction
/// ownership check, restated as their predicate: the conversation is the
/// caller's and not deleted, the target's binding is in the caller's
/// organization and names the caller, and its credential holds
/// `conversation.read` on exactly this gateway.
#[test]
fn owner_share_answers_as_the_share_checks_did() {
    let policy = policy();
    let elsewhere = Resource::new(
        organization("elsewhere"),
        ResourceId::new("gateway").unwrap(),
    );
    let other = Resource::new(organization("org"), ResourceId::new("other").unwrap());
    let grant_sets: [Vec<Grant>; 6] = [
        vec![],
        vec![grant("conversation.read", gateway())],
        vec![grant("conversation.read", other.clone())],
        vec![grant("conversation.read", elsewhere.clone())],
        vec![grant("environment.serve", gateway())],
        vec![
            grant("environment.serve", gateway()),
            grant("conversation.read", gateway()),
        ],
    ];
    for deleted in [false, true] {
        let record = conversation("org", "owner", deleted);
        for who in ["owner", "other"] {
            for (target_org, target_owner) in PEOPLE {
                for grants in &grant_sets {
                    let rust = record
                        .check_access(&organization("org"), &principal(who))
                        .is_ok()
                        && organization(target_org) == organization("org")
                        && principal(target_owner) == principal(who)
                        && grants.iter().any(|grant| {
                            grant.action().as_str() == "conversation.read"
                                && grant.resource() == &gateway()
                        });
                    assert_eq!(
                        allowed(&share(
                            &policy,
                            &record,
                            who,
                            target_org,
                            target_owner,
                            grants
                        )),
                        rust,
                        "{who} {target_org} {target_owner} {grants:?} deleted={deleted}"
                    );
                }
            }
        }
    }
}
