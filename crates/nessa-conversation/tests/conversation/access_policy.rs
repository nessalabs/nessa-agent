//! Conversation access as Cedar decides it: the checked-in bundle
//! validates, every rule allows what it names and nothing past it, a
//! refusal says which rule made it, anything the adapter cannot decide is
//! refused, and each rule answers as its truth table below says, across
//! every combination of who asks, whose conversation it is, whether it was
//! deleted, the role held and what a request carries. The tables were
//! written from the Rust the rules replaced (ADR 803 step 3) and pin that
//! behaviour now that the Rust is gone.
use super::*;
use crate::application::{AccessAction, OwnerAccess, PairedAccess, PairedParty, PairedRefusal};
use crate::domain::{
    Conversation, ConversationDeletion, ConversationRefusal, Setting, SettingLevel,
};
use nessa_auth::domain::{
    Action, Grant, MembershipRole, OrganizationId, PrincipalId, Resource, ResourceId,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn paired_list_catalogue_admission_matches_its_device_and_scope() {
    let policy = policy();
    let org = organization("org");
    let owner = principal("owner");
    let resource = AccessResource::Catalogue {
        organization: &org,
        owner: &owner,
        app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
    };
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
                assert_eq!(
                    allowed(&paired(&policy, AccessAction::List, resource, &facts)),
                    (org, binding_owner) == ("org", "owner") && signed_in_as == binding_owner
                );
            }
        }
    }
}

#[test]
fn paired_list_candidates_reuse_decisions_only_for_matching_facts() {
    struct Counted {
        policy: CedarConversationAccessPolicy,
        calls: AtomicUsize,
    }
    impl ConversationAccessPolicyEvaluator for Counted {
        fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.policy.evaluate(ask)
        }
    }
    let policy = Counted {
        policy: policy(),
        calls: AtomicUsize::new(0),
    };
    let (org, owner) = (organization("org"), principal("owner"));
    let access = PairedAccess::new(
        &policy,
        PairedParty {
            organization: &org,
            signed_in_as: &owner,
            binding_owner: &owner,
            receiver: "phone",
        },
    );
    let live = conversation("org", "owner", false);
    let foreign = conversation("other", "owner", false);
    let another_owner = conversation("org", "other", false);
    let deleted = conversation("org", "owner", true);
    let read = Some(Capabilities::read());
    assert_eq!(
        access
            .allows_each(
                AccessAction::Read,
                [
                    (&live, read),
                    (&live, read),
                    (&foreign, read),
                    (&live, None),
                    (&deleted, read),
                    (&another_owner, read),
                    (&live, Some(Capabilities::comment())),
                ]
            )
            .unwrap(),
        [true, true, false, false, false, false, true]
    );
    assert_eq!(policy.calls.load(Ordering::Relaxed), 6);
}

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
    held: Option<Capabilities>,
    request: RequestShape<'a>,
}

impl Default for Paired<'_> {
    /// A device of `owner` in `org` holding a Read grant, sending text.
    fn default() -> Self {
        Self {
            org: "org",
            signed_in_as: "owner",
            binding_owner: "owner",
            held: Some(Capabilities::read()),
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
            AccessAction::List
            | AccessAction::Write
            | AccessAction::ChannelCreate
            | AccessAction::ChannelList
            | AccessAction::TerminalUse => AccessResource::Catalogue {
                organization: &org,
                owner: &owner,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            AccessAction::ChannelUse | AccessAction::ChannelSettings => AccessResource::Channel {
                organization: &org,
                owner: &owner,
            },
            AccessAction::WorkspaceRead | AccessAction::WorkspaceControl => {
                AccessResource::Workspace {
                    organization: &org,
                    owner: &owner,
                }
            }
            AccessAction::CloseCreated => AccessResource::created(&live, Some("creator")),
            AccessAction::SettingChange => AccessResource::Setting {
                organization: &org,
                owner: &owner,
                setting: Setting::Access,
            },
            _ => on(&live),
        };
        let subject = match action {
            AccessAction::AgentTools
            | AccessAction::ReadTurns
            | AccessAction::AskOwner
            | AccessAction::RequestAccess
            | AccessAction::CloseCreated => AccessSubject::AgentTurn {
                organization: &org,
                principal: &owner,
                owner_initiated: true,
                conversation: "creator",
            },
            AccessAction::ReviewNotice => AccessSubject::ReviewRecipient {
                organization: &org,
                principal: &owner,
                membership: MembershipRole::Member,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            AccessAction::NoteDeliver => AccessSubject::GrantHolder {
                receiver: "receiver-1",
            },
            AccessAction::CatalogueEntry
            | AccessAction::Comment
            | AccessAction::Drive
            | AccessAction::Write => AccessSubject::PairedReader {
                organization: &org,
                signed_in_as: &owner,
                binding_owner: &owner,
                receiver: "receiver-1",
            },
            _ => AccessSubject::Person {
                organization: &org,
                principal: &owner,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
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

#[test]
fn a_setting_changes_as_far_as_its_level_lets_who_asks() {
    let policy = policy();
    let (org, elsewhere, owner, other) = (
        organization("org"),
        organization("elsewhere"),
        principal("owner"),
        principal("other"),
    );
    let person = |organization, principal| AccessSubject::Person {
        organization,
        principal,
        app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
    };
    let turn = |organization, principal, owner_initiated| AccessSubject::AgentTurn {
        organization,
        principal,
        owner_initiated,
        conversation: "thread",
    };
    let change = |subject, setting| {
        policy.evaluate(&AccessAsk {
            subject,
            action: AccessAction::SettingChange,
            resource: AccessResource::Setting {
                organization: &org,
                owner: &owner,
                setting,
            },
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    for setting in Setting::ALL {
        let name = setting.as_str();
        // The owner, on their own surface, changes every level.
        assert_eq!(
            allowed_by(&change(person(&org, &owner), setting)),
            Some(&["owner.setting_change".to_owned()][..]),
            "{name}"
        );
        // A turn the owner started changes only what an agent may.
        let agent = change(turn(&org, &owner, true), setting);
        if setting.level() == SettingLevel::AgentChanges {
            assert_eq!(
                allowed_by(&agent),
                Some(&["agent.setting_change".to_owned()][..]),
                "{name}"
            );
        } else {
            assert_eq!(
                agent,
                AccessDecision::Deny(AccessDenial::NotPermitted),
                "{name}"
            );
        }
        // Nobody else, and no turn the owner did not start.
        for refused in [
            person(&org, &other),
            person(&elsewhere, &owner),
            turn(&org, &owner, false),
            turn(&org, &other, true),
            turn(&elsewhere, &owner, true),
        ] {
            assert!(!allowed(&change(refused, setting)), "{name} {refused:?}");
        }
        // The helper callers use answers the same, rule and all.
        assert_eq!(
            crate::application::setting_change(
                &policy,
                turn(&org, &owner, true),
                &org,
                &owner,
                setting
            ),
            agent,
            "{name}"
        );
    }
    // Every level has a setting, and raising access is never an agent's.
    for level in SettingLevel::ALL {
        assert!(
            Setting::ALL.iter().any(|setting| setting.level() == level),
            "{}",
            level.as_str()
        );
    }
    assert_eq!(Setting::Access.level(), SettingLevel::RaisesAccess);
    assert_eq!(Setting::ReplyStyle.level(), SettingLevel::AgentChanges);
    assert_eq!(Setting::VoiceKey.level(), SettingLevel::OwnerTypes);
    assert_eq!(Setting::McpServers.level(), SettingLevel::AgentFills);
}

/// `ALL` lists each setting and level once, in order. The matches below
/// have no default arm, so a new variant does not compile until it is given
/// a place here, and a place past the end of `ALL` fails the length check.
#[test]
fn every_setting_and_level_is_listed_once() {
    let place = |setting: Setting| match setting {
        Setting::ReplyStyle => 0,
        Setting::Model => 1,
        Setting::UsageLimitAutoContinue => 2,
        Setting::McpServers => 3,
        Setting::ChannelCoordinator => 4,
        Setting::ChannelPlacement => 5,
        Setting::Devices => 6,
        Setting::CloudRuntime => 7,
        Setting::VoiceKey => 8,
        Setting::GitHub => 9,
        Setting::Credentials => 10,
        Setting::McpSignIn => 11,
        Setting::Access => 12,
        Setting::PermissionRules => 13,
        Setting::AppTrust => 14,
        Setting::Shares => 15,
        Setting::Linking => 16,
        Setting::AgentHooks => 17,
    };
    for (at, setting) in Setting::ALL.into_iter().enumerate() {
        assert_eq!(place(setting), at, "{}", setting.as_str());
    }
    assert_eq!(place(Setting::AgentHooks) + 1, Setting::ALL.len());
    let level_place = |level: SettingLevel| match level {
        SettingLevel::AgentChanges => 0,
        SettingLevel::AgentFills => 1,
        SettingLevel::OwnerTypes => 2,
        SettingLevel::RaisesAccess => 3,
    };
    for (at, level) in SettingLevel::ALL.into_iter().enumerate() {
        assert_eq!(level_place(level), at, "{}", level.as_str());
    }
    assert_eq!(
        level_place(SettingLevel::RaisesAccess) + 1,
        SettingLevel::ALL.len()
    );
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
fn owner_terminal_run_admits_the_owner_until_the_conversation_is_deleted() {
    let policy = policy();
    assert_eq!(
        allowed_by(&person(
            &policy,
            AccessAction::TerminalUse,
            &conversation("org", "owner", false),
            "org",
            "owner"
        )),
        Some(&["owner.terminal_run".to_owned()][..])
    );
    for (conversation, org, who) in [
        (conversation("org", "owner", true), "org", "owner"),
        (conversation("org", "owner", false), "org", "someone"),
        (conversation("org", "owner", false), "other", "owner"),
    ] {
        assert!(!allowed(&person(
            &policy,
            AccessAction::TerminalUse,
            &conversation,
            org,
            who
        )));
    }
}

#[test]
fn owner_erase_admits_only_the_owner_of_a_tombstone() {
    let policy = policy();
    let deleted = conversation("org", "owner", true);
    let live = conversation("org", "owner", false);
    assert_eq!(
        allowed_by(&person(
            &policy,
            AccessAction::Erase,
            &deleted,
            "org",
            "owner"
        )),
        Some(&["owner.erase".to_owned()][..])
    );
    assert!(!allowed(&person(
        &policy,
        AccessAction::Erase,
        &live,
        "org",
        "owner"
    )));
    assert_eq!(
        person(&policy, AccessAction::Erase, &deleted, "org", "other"),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
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
fn owner_and_device_lists_ask_their_own_catalogue_policies() {
    let policy = policy();
    let (org, owner, other) = (organization("org"), principal("owner"), principal("other"));
    let catalogue = AccessResource::Catalogue {
        organization: &org,
        owner: &owner,
        app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
    };
    let list = |principal: &PrincipalId| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::Person {
                organization: &org,
                principal,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
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
    assert_eq!(
        allowed_by(&paired(
            &policy,
            AccessAction::List,
            catalogue,
            &Paired::default()
        )),
        Some(&["grant.list".to_owned()][..])
    );
}

#[test]
fn owner_workspace_read_is_the_owners_own_windows_and_nobody_elses() {
    let policy = policy();
    let (org, elsewhere, owner, other) = (
        organization("org"),
        organization("elsewhere"),
        principal("owner"),
        principal("other"),
    );
    let read = |organization: &OrganizationId, principal: &PrincipalId| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::Person {
                organization,
                principal,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            action: AccessAction::WorkspaceRead,
            resource: AccessResource::Workspace {
                organization: &org,
                owner: &owner,
            },
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    assert_eq!(
        allowed_by(&read(&org, &owner)),
        Some(&["owner.workspace_read".to_owned()][..])
    );
    assert!(!allowed(&read(&org, &other)));
    assert!(!allowed(&read(&elsewhere, &owner)));
    // A paired device signs in as its owner and still reads no window: only
    // a person's own turn does.
    assert!(!allowed(&paired(
        &policy,
        AccessAction::WorkspaceRead,
        AccessResource::Workspace {
            organization: &org,
            owner: &owner,
        },
        &Paired::default()
    )));
}

#[test]
fn owner_workspace_control_is_the_owners_own_windows_and_nobody_elses() {
    let policy = policy();
    let (org, elsewhere, owner, other) = (
        organization("org"),
        organization("elsewhere"),
        principal("owner"),
        principal("other"),
    );
    let control = |organization: &OrganizationId, principal: &PrincipalId| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::Person {
                organization,
                principal,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            action: AccessAction::WorkspaceControl,
            resource: AccessResource::Workspace {
                organization: &org,
                owner: &owner,
            },
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    assert_eq!(
        allowed_by(&control(&org, &owner)),
        Some(&["owner.workspace_control".to_owned()][..])
    );
    assert!(!allowed(&control(&org, &other)));
    assert!(!allowed(&control(&elsewhere, &owner)));
    // A paired device signs in as its owner and still moves no window.
    assert!(!allowed(&paired(
        &policy,
        AccessAction::WorkspaceControl,
        AccessResource::Workspace {
            organization: &org,
            owner: &owner,
        },
        &Paired::default()
    )));
}

#[test]
fn owner_channels_are_the_owners_own_and_nobody_elses() {
    let policy = policy();
    let (org, elsewhere, owner, other) = (
        organization("org"),
        organization("elsewhere"),
        principal("owner"),
        principal("other"),
    );
    let cases = [
        (
            AccessAction::ChannelCreate,
            AccessResource::Catalogue {
                organization: &org,
                owner: &owner,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            "owner.channel_create",
        ),
        (
            AccessAction::ChannelList,
            AccessResource::Catalogue {
                organization: &org,
                owner: &owner,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            "owner.channel_list",
        ),
        (
            AccessAction::ChannelUse,
            AccessResource::Channel {
                organization: &org,
                owner: &owner,
            },
            "owner.channel_use",
        ),
        (
            AccessAction::ChannelSettings,
            AccessResource::Channel {
                organization: &org,
                owner: &owner,
            },
            "owner.channel_settings",
        ),
    ];
    for (action, resource, id) in cases {
        let ask = |organization: &OrganizationId, principal: &PrincipalId| {
            policy.evaluate(&AccessAsk {
                subject: AccessSubject::Person {
                    organization,
                    principal,
                    app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
                },
                action,
                resource,
                held: None,
                request: RequestShape::default(),
                target: None,
            })
        };
        assert_eq!(
            allowed_by(&ask(&org, &owner)),
            Some(&[id.to_owned()][..]),
            "{id}"
        );
        assert!(!allowed(&ask(&org, &other)), "{id}");
        assert!(!allowed(&ask(&elsewhere, &owner)), "{id}");
        // A paired device makes, lists, and uses no channel: a channel can
        // name a folder on this machine.
        assert!(
            !allowed(&paired(&policy, action, resource, &Paired::default())),
            "{id}"
        );
    }
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
fn grant_artifact_read_follows_the_read_capability() {
    let policy = policy();
    let peer = Paired {
        signed_in_as: "gateway:peer",
        ..Paired::default()
    };
    let live = conversation("org", "owner", false);
    assert_eq!(
        allowed_by(&paired(
            &policy,
            AccessAction::ArtifactRead,
            on(&live),
            &peer
        )),
        Some(&["grant.artifact_read".to_owned()][..])
    );
    assert!(!allowed(&paired(
        &policy,
        AccessAction::ArtifactRead,
        on(&live),
        &Paired {
            signed_in_as: "gateway:peer",
            held: None,
            ..Paired::default()
        }
    )));
    let deleted = conversation("org", "owner", true);
    assert!(!allowed(&paired(
        &policy,
        AccessAction::ArtifactRead,
        on(&deleted),
        &peer
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

/// A device signed in as the binding owner, holding only read, is denied
/// comment and drive. Removing `capabilities.contains` from those permits
/// would allow them: the other conditions of the permit already hold.
#[test]
fn grant_comment_and_drive_need_their_capability() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    for (action, role, id) in [
        (
            AccessAction::Comment,
            Capabilities::comment(),
            "grant.comment",
        ),
        (AccessAction::Drive, Capabilities::drive(), "grant.drive"),
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

/// A peer holding drive comments and drives. Holding only read, it does
/// neither. Removing `capabilities.contains` from the permit would let the
/// read-only peer through: the other conditions already hold.
#[test]
fn a_peer_comments_and_drives_when_the_grant_names_the_capability() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    let peer = |held| Paired {
        signed_in_as: "gateway:peer",
        held,
        ..Paired::default()
    };
    for (action, id) in [
        (AccessAction::Comment, "grant.comment"),
        (AccessAction::Drive, "grant.drive"),
    ] {
        assert_eq!(
            allowed_by(&paired(
                &policy,
                action,
                on(&live),
                &peer(Some(Capabilities::drive()))
            )),
            Some(&[id.to_owned()][..])
        );
    }
    assert_eq!(
        paired(
            &policy,
            AccessAction::Drive,
            on(&live),
            &peer(Some(Capabilities::read()))
        ),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
    assert!(allowed(&paired(
        &policy,
        AccessAction::Read,
        on(&live),
        &peer(Some(Capabilities::read()))
    )));
}

#[test]
fn drive_text_only_refuses_what_a_driver_could_name_unshared() {
    let policy = policy();
    let live = conversation("org", "owner", false);
    let driver = Paired {
        held: Some(Capabilities::drive()),
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
fn grant_note_deliver_needs_comment_or_drive_on_a_live_conversation() {
    let policy = policy();
    let deliver = |held: Option<Capabilities>, deleted: bool| {
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
        allowed_by(&deliver(Some(Capabilities::comment()), false)),
        Some(&["grant.note_deliver".to_owned()][..])
    );
    assert!(!allowed(&deliver(Some(Capabilities::read()), false)));
    assert!(!allowed(&deliver(Some(Capabilities::drive()), true)));
    assert!(!allowed(&deliver(None, false)));
}

#[test]
fn a_paired_reader_writes_nothing_else() {
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    assert_eq!(
        paired(
            &policy,
            AccessAction::Write,
            AccessResource::Catalogue {
                organization: &org,
                owner: &owner,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            &Paired {
                held: Some(Capabilities::drive()),
                ..Paired::default()
            }
        ),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
}

// The bundle as built.

/// A bundle that does not parse, does not validate strictly, holds a
/// policy without an `@id`, or repeats an `@id` is refused when it is built.
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
    let twice = r#"
        @id("owner.act") permit (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource is NessaConversation::Conversation);
        @id("owner.act") forbid (principal is NessaConversation::Person, action == NessaConversation::Action::"conversation.act", resource is NessaConversation::Conversation);
    "#;
    let duplicate = CedarConversationAccessPolicy::from_sources(SCHEMA, twice);
    assert!(duplicate.is_err_and(|error| error.contains("not unique")));
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

// Truth tables: each answer written out, as the Rust each rule replaced
// gave it.

const PEOPLE: [(&str, &str); 3] = [("org", "owner"), ("org", "other"), ("elsewhere", "owner")];
const ROLES: [Option<Capabilities>; 4] = [
    None,
    Some(Capabilities::read()),
    Some(Capabilities::comment()),
    Some(Capabilities::drive()),
];

/// `OwnerAccess::admit_to` (act, then locate on a refusal) for every caller
/// and deletion: the owner acts on a live conversation and is told a
/// deleted one was deleted; anybody else is told it does not exist.
#[test]
fn owner_access_tells_only_the_owner_of_a_deletion() {
    use ConversationRefusal::{Deleted, NotFound};
    let policy = policy();
    let access = OwnerAccess::new(&policy);
    type Row<'a> = (
        (&'a str, &'a str),
        bool,
        Result<(), ConversationRefusal>,
        bool,
    );
    let table: [Row<'_>; 6] = [
        // (caller, deleted, act, locates)
        (("org", "owner"), false, Ok(()), true),
        (("org", "owner"), true, Err(Deleted), true),
        (("org", "other"), false, Err(NotFound), false),
        (("org", "other"), true, Err(NotFound), false),
        (("elsewhere", "owner"), false, Err(NotFound), false),
        (("elsewhere", "owner"), true, Err(NotFound), false),
    ];
    for ((org, who), deleted, act, locates) in table {
        let record = conversation("org", "owner", deleted);
        let (org, who) = (organization(org), principal(who));
        assert_eq!(
            access.admit_to(AccessAction::Act, &record, &org, &who),
            act,
            "{org:?} {who:?} deleted={deleted}"
        );
        assert_eq!(access.locates(&record, &org, &who), Ok(locates));
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

/// What a paired reader may do, for every binding, signer, deletion and
/// role: it reads its grantor's live conversation with any role; comments
/// with Comment or Drive and drives with Drive, a device and a peer gateway
/// alike; sees a granted tombstone in its catalogue; and never writes,
/// shares or takes owner write commands. A device lists its grant scope.
#[test]
fn paired_access_answers_its_truth_table() {
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
                    // The binding's owner's conversation, in its
                    // organization.
                    let grantors = (org, binding_owner) == ("org", "owner");
                    let owned = grantors && !deleted;
                    assert_eq!(
                        allowed(&paired(&policy, AccessAction::Read, on(&record), &facts)),
                        owned && held.is_some(),
                        "read {org} {binding_owner} {signed_in_as} {held:?} deleted={deleted}"
                    );
                    // Comment and drive follow the capability, for a device
                    // and for a peer. Removing `capabilities.contains` from
                    // the permit would allow a read-only grant.
                    for (action, roles) in [
                        (
                            AccessAction::Comment,
                            &[Capabilities::comment(), Capabilities::drive()][..],
                        ),
                        (AccessAction::Drive, &[Capabilities::drive()][..]),
                    ] {
                        assert_eq!(
                            allowed(&paired(&policy, action, on(&record), &facts)),
                            owned && held.is_some_and(|held| roles.contains(&held)),
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
                        grantors && held.is_some(),
                    );
                    // The share commands refuse a paired device by what it is.
                    for action in [
                        AccessAction::Share,
                        AccessAction::Unshare,
                        AccessAction::Shares,
                    ] {
                        assert!(!allowed(&paired(&policy, action, on(&record), &facts)));
                    }
                    // No owner command no grant role names is ever a paired
                    // reader's.
                    let catalogue = AccessResource::Catalogue {
                        organization: &borg,
                        owner: &bowner,
                        app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
                    };
                    assert!(!allowed(&paired(
                        &policy,
                        AccessAction::Write,
                        catalogue,
                        &facts
                    )));
                }
            }
        }
    }
}

/// A driver sends text, as its truth table: a Drive command naming
/// attachments or files, or a receipt lookup for `create`, is refused, and
/// anything else is let through (row J14).
#[test]
fn drive_text_only_answers_its_truth_table() {
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
                        held: Some(Capabilities::drive()),
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
                        held: Some(Capabilities::comment()),
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

/// The share rule as its truth table: the conversation is the caller's and
/// not deleted, the target's binding is in the caller's organization and
/// names the caller, and its credential holds `conversation.read` on
/// exactly this gateway.
#[test]
fn owner_share_answers_its_truth_table() {
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
                    let expected = who == "owner"
                        && !deleted
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
                        expected,
                        "{who} {target_org} {target_owner} {grants:?} deleted={deleted}"
                    );
                }
            }
        }
    }
}

/// What a paired party is told, for every signer, role held, deletion and
/// request: allowed; a party that may read the conversation, a peer gateway
/// included, forbidden what its capability or request does not allow; a
/// granted tombstone told it was deleted when the party signed in as its
/// owner, as the socket's owner check told it; and anything else told it
/// does not exist, as an ungranted id is (rows G1, J11 to J14). A peer
/// without a grant, or with one on a deleted conversation, learns no more
/// than from a missing id.
#[test]
fn paired_access_tells_each_refusal_as_its_table_says() {
    use PairedRefusal::{Deleted, Forbidden, NotFound};
    let policy = policy();
    let (org, owner, peer) = (
        organization("org"),
        principal("owner"),
        principal("gateway:peer"),
    );
    let attachments = RequestShape {
        attachments: true,
        ..RequestShape::default()
    };
    let text = RequestShape::default();
    type Row<'a> = (
        bool,
        AccessAction,
        Option<Capabilities>,
        bool,
        RequestShape<'a>,
        Result<(), PairedRefusal>,
    );
    // (peer, action, held, deleted, request, told)
    let table: [Row<'_>; 32] = [
        (false, AccessAction::Read, None, false, text, Err(NotFound)),
        (
            false,
            AccessAction::Read,
            Some(Capabilities::read()),
            false,
            text,
            Ok(()),
        ),
        (
            false,
            AccessAction::Read,
            Some(Capabilities::drive()),
            false,
            text,
            Ok(()),
        ),
        (false, AccessAction::Read, None, true, text, Err(NotFound)),
        (
            false,
            AccessAction::Read,
            Some(Capabilities::read()),
            true,
            text,
            Err(Deleted),
        ),
        (
            false,
            AccessAction::Comment,
            None,
            false,
            text,
            Err(NotFound),
        ),
        (
            false,
            AccessAction::Comment,
            Some(Capabilities::read()),
            false,
            text,
            Err(Forbidden),
        ),
        (
            false,
            AccessAction::Comment,
            Some(Capabilities::comment()),
            false,
            text,
            Ok(()),
        ),
        (
            false,
            AccessAction::Comment,
            Some(Capabilities::drive()),
            false,
            text,
            Ok(()),
        ),
        (
            false,
            AccessAction::Comment,
            Some(Capabilities::comment()),
            false,
            attachments,
            Ok(()),
        ),
        (
            false,
            AccessAction::Comment,
            Some(Capabilities::comment()),
            true,
            text,
            Err(Deleted),
        ),
        (
            false,
            AccessAction::Comment,
            None,
            true,
            text,
            Err(NotFound),
        ),
        (false, AccessAction::Drive, None, false, text, Err(NotFound)),
        (
            false,
            AccessAction::Drive,
            Some(Capabilities::read()),
            false,
            text,
            Err(Forbidden),
        ),
        (
            false,
            AccessAction::Drive,
            Some(Capabilities::comment()),
            false,
            text,
            Err(Forbidden),
        ),
        (
            false,
            AccessAction::Drive,
            Some(Capabilities::drive()),
            false,
            text,
            Ok(()),
        ),
        (
            false,
            AccessAction::Drive,
            Some(Capabilities::drive()),
            false,
            attachments,
            Err(Forbidden),
        ),
        (
            false,
            AccessAction::Drive,
            None,
            false,
            attachments,
            Err(NotFound),
        ),
        (
            false,
            AccessAction::Drive,
            Some(Capabilities::drive()),
            true,
            text,
            Err(Deleted),
        ),
        // A role below the action's on a deleted conversation is told it
        // was deleted: the policy says nothing of a role on a tombstone.
        (
            false,
            AccessAction::Drive,
            Some(Capabilities::read()),
            true,
            text,
            Err(Deleted),
        ),
        (false, AccessAction::Drive, None, true, text, Err(NotFound)),
        (
            true,
            AccessAction::Read,
            Some(Capabilities::read()),
            false,
            text,
            Ok(()),
        ),
        (true, AccessAction::Read, None, false, text, Err(NotFound)),
        // A peer's own principal owns nothing, so it is never told of a
        // deletion.
        (
            true,
            AccessAction::Read,
            Some(Capabilities::read()),
            true,
            text,
            Err(NotFound),
        ),
        (
            true,
            AccessAction::Comment,
            Some(Capabilities::comment()),
            false,
            text,
            Ok(()),
        ),
        (
            true,
            AccessAction::Comment,
            None,
            false,
            text,
            Err(NotFound),
        ),
        (
            true,
            AccessAction::Drive,
            Some(Capabilities::drive()),
            false,
            text,
            Ok(()),
        ),
        (true, AccessAction::Drive, None, true, text, Err(NotFound)),
        (
            true,
            AccessAction::Drive,
            Some(Capabilities::drive()),
            true,
            text,
            Err(NotFound),
        ),
        (
            true,
            AccessAction::Drive,
            Some(Capabilities::drive()),
            false,
            attachments,
            Err(Forbidden),
        ),
        (true, AccessAction::Comment, None, true, text, Err(NotFound)),
        (
            true,
            AccessAction::Comment,
            Some(Capabilities::comment()),
            true,
            text,
            Err(NotFound),
        ),
    ];
    for (is_peer, action, held, deleted, request, told) in table {
        let record = conversation("org", "owner", deleted);
        let access = PairedAccess::new(
            &policy,
            PairedParty {
                organization: &org,
                signed_in_as: if is_peer { &peer } else { &owner },
                binding_owner: &owner,
                receiver: "receiver-1",
            },
        );
        assert_eq!(
            access.admit(action, &record, held, request),
            told,
            "peer={is_peer} {} {held:?} deleted={deleted} {request:?}",
            action.name()
        );
    }
}

/// A paired party lists its granted scope, sends no other owner
/// command and shares nothing, and an
/// evaluator that cannot decide makes every paired question unavailable.
#[test]
fn paired_access_lists_its_scope_but_shares_nothing_and_fails_closed() {
    struct Undecided;
    impl ConversationAccessPolicyEvaluator for Undecided {
        fn evaluate(&self, _: &AccessAsk<'_>) -> AccessDecision {
            AccessDecision::Deny(AccessDenial::Unavailable)
        }
    }
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let party = PairedParty {
        organization: &org,
        signed_in_as: &owner,
        binding_owner: &owner,
        receiver: "receiver-1",
    };
    let record = conversation("org", "owner", false);
    let access = PairedAccess::new(&policy, party);
    assert_eq!(access.admit_catalogue(AccessAction::List), Ok(()));
    assert_eq!(
        access.admit_catalogue(AccessAction::Write),
        Err(PairedRefusal::Forbidden)
    );
    for action in [
        AccessAction::Share,
        AccessAction::Unshare,
        AccessAction::Shares,
    ] {
        assert_eq!(
            access.allows(action, &record, Some(Capabilities::drive())),
            Ok(false)
        );
    }
    let undecided = PairedAccess::new(&Undecided, party);
    assert_eq!(
        undecided.admit(
            AccessAction::Read,
            &record,
            Some(Capabilities::read()),
            RequestShape::default()
        ),
        Err(PairedRefusal::Unavailable)
    );
    for action in [AccessAction::List, AccessAction::Write] {
        assert_eq!(
            undecided.admit_catalogue(action),
            Err(PairedRefusal::Unavailable)
        );
    }
}

/// Role, organization, owner and deletion remain separate policy facts.
/// Paired readers have their grant-specific read path instead of this action.
#[test]
fn review_notices_use_current_membership_and_live_organization_ownership() {
    let policy = policy();
    for membership in [MembershipRole::Admin, MembershipRole::Member] {
        for same_organization in [false, true] {
            for owns in [false, true] {
                for deleted in [false, true] {
                    let org = organization(if same_organization { "org" } else { "other" });
                    let who = principal(if owns { "owner" } else { "surface" });
                    let conversation = conversation("org", "owner", deleted);
                    let decision = policy.evaluate(&AccessAsk {
                        subject: AccessSubject::ReviewRecipient {
                            organization: &org,
                            principal: &who,
                            membership,
                            app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
                        },
                        action: AccessAction::ReviewNotice,
                        resource: on(&conversation),
                        held: None,
                        request: RequestShape::default(),
                        target: None,
                    });
                    assert_ne!(decision, AccessDecision::Deny(AccessDenial::Unavailable));
                    assert_eq!(allowed(&decision), same_organization && !deleted &&
                        (owns || membership == MembershipRole::Admin),
                        "{membership:?}, same org={same_organization}, owns={owns}, deleted={deleted}");
                }
            }
        }
    }
}

/// Agent delegation is its own Cedar question; a grant origin cannot inherit
/// the metadata owner's cross-thread authority, even on a matching account.
#[test]
fn agent_tools_require_an_owner_origin_and_matching_live_source_owner() {
    let policy = policy();
    let org = organization("org");
    let owner = principal("owner");
    for owner_initiated in [false, true] {
        for same_owner in [false, true] {
            for same_org in [false, true] {
                for deleted in [false, true] {
                    let source = conversation(
                        if same_org { "org" } else { "other" },
                        if same_owner { "owner" } else { "other" },
                        deleted,
                    );
                    let decision = policy.evaluate(&AccessAsk {
                        subject: AccessSubject::AgentTurn {
                            organization: &org,
                            principal: &owner,
                            owner_initiated,
                            conversation: "source",
                        },
                        action: AccessAction::AgentTools,
                        resource: AccessResource::of(&source),
                        held: None,
                        request: RequestShape::default(),
                        target: None,
                    });
                    assert_eq!(
                        allowed(&decision),
                        owner_initiated && same_owner && same_org && !deleted
                    );
                }
            }
        }
    }
}

/// A turn another agent started reads where the owner's turns came from, and
/// still holds none of the owner's other agent tools. Another owner's
/// conversation, and a deleted one, it cannot read.
#[test]
fn an_agent_started_turn_reads_turns_but_holds_no_other_agent_tool() {
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let ask = |action, conversation: &Conversation, owner_initiated| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::AgentTurn {
                organization: &org,
                principal: &owner,
                owner_initiated,
                conversation: "source",
            },
            action,
            resource: on(conversation),
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    let live = conversation("org", "owner", false);
    assert!(matches!(
        ask(AccessAction::ReadTurns, &live, false),
        AccessDecision::Allow { .. }
    ));
    assert!(matches!(
        ask(AccessAction::ReadTurns, &live, true),
        AccessDecision::Allow { .. }
    ));
    assert!(matches!(
        ask(AccessAction::AgentTools, &live, false),
        AccessDecision::Deny(_)
    ));
    let others = conversation("org", "someone-else", false);
    assert!(matches!(
        ask(AccessAction::ReadTurns, &others, false),
        AccessDecision::Deny(_)
    ));
    let deleted = conversation("org", "owner", true);
    assert!(matches!(
        ask(AccessAction::ReadTurns, &deleted, false),
        AccessDecision::Deny(_)
    ));
}

/// Any turn of the owner's agents asks the owner a question, one another
/// agent started included (a coordinator woken by a thread's report), and
/// only in the owner's own conversation that is not deleted.
#[test]
fn any_turn_of_the_owners_asks_the_owner() {
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let ask = |conversation: &Conversation, owner_initiated| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::AgentTurn {
                organization: &org,
                principal: &owner,
                owner_initiated,
                conversation: "source",
            },
            action: AccessAction::AskOwner,
            resource: on(conversation),
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    let live = conversation("org", "owner", false);
    for owner_initiated in [true, false] {
        assert_eq!(
            allowed_by(&ask(&live, owner_initiated)),
            Some(&["owner.ask_owner".to_owned()][..])
        );
    }
    assert!(!allowed(&ask(
        &conversation("org", "someone-else", false),
        false
    )));
    assert!(!allowed(&ask(
        &conversation("elsewhere", "owner", false),
        true
    )));
    assert!(!allowed(&ask(&conversation("org", "owner", true), true)));
}

/// Only a turn the owner started asks to raise the owner's access, and only
/// in the owner's own conversation that is not deleted: a turn another agent
/// started may ask a question, never put the red card in front of the owner.
#[test]
fn only_a_turn_the_owner_started_asks_to_raise_access() {
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let ask = |conversation: &Conversation, owner_initiated| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::AgentTurn {
                organization: &org,
                principal: &owner,
                owner_initiated,
                conversation: "source",
            },
            action: AccessAction::RequestAccess,
            resource: on(conversation),
            held: None,
            request: RequestShape::default(),
            target: None,
        })
    };
    let live = conversation("org", "owner", false);
    assert_eq!(
        allowed_by(&ask(&live, true)),
        Some(&["owner.request_access".to_owned()][..])
    );
    assert!(!allowed(&ask(&live, false)));
    assert!(!allowed(&ask(
        &conversation("org", "someone-else", false),
        true
    )));
    assert!(!allowed(&ask(
        &conversation("elsewhere", "owner", false),
        true
    )));
    assert!(!allowed(&ask(&conversation("org", "owner", true), true)));
}

/// A turn the owner started closes only a thread its own thread created, as
/// the spawn's record says: not one another thread created, not one no agent
/// created, and not from a turn another agent started.
#[test]
fn a_turn_closes_only_a_thread_its_own_thread_created() {
    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let child = conversation("org", "owner", false);
    let ask = |conversation: &Conversation, created_by, owner_initiated| {
        allowed(&policy.evaluate(&AccessAsk {
            subject: AccessSubject::AgentTurn {
                organization: &org,
                principal: &owner,
                owner_initiated,
                conversation: "parent",
            },
            action: AccessAction::CloseCreated,
            resource: AccessResource::created(conversation, created_by),
            held: None,
            request: RequestShape::default(),
            target: None,
        }))
    };
    assert!(ask(&child, Some("parent"), true));
    assert!(
        !ask(&child, Some("another"), true),
        "another thread's child"
    );
    assert!(!ask(&child, None, true), "a thread no agent created");
    assert!(
        !ask(&child, Some("parent"), false),
        "a turn an agent started"
    );
    let others = conversation("org", "someone-else", false);
    assert!(
        !ask(&others, Some("parent"), true),
        "another owner's thread"
    );
    let deleted = conversation("org", "owner", true);
    assert!(!ask(&deleted, Some("parent"), true), "a deleted thread");
}

#[test]
fn app_namespace_isolation_denies_cross_namespace_owner_actions() {
    use nessa_auth::domain::AppNamespace;

    let policy = policy();
    let (org, owner) = (organization("org"), principal("owner"));
    let default_ns = AppNamespace::DEFAULT;
    let ext_a = AppNamespace::extension("tracker-a").unwrap();
    let ext_b = AppNamespace::extension("tracker-b").unwrap();

    let live_default = conversation("org", "owner", false);
    let live_ext_a = conversation("org", "owner", false).with_app_namespace(ext_a.clone());
    let deleted_ext_a = conversation("org", "owner", true).with_app_namespace(ext_a.clone());

    for action in [
        AccessAction::Locate,
        AccessAction::Act,
        AccessAction::Read,
        AccessAction::ArtifactRead,
        AccessAction::TerminalUse,
    ] {
        // Matching namespaces are permitted (both built-in Nessa and extensions).
        assert!(
            allowed(&policy.evaluate(&AccessAsk::person_in(
                action,
                &live_default,
                &org,
                &owner,
                &default_ns,
            ))),
            "same default namespace allows {action:?}"
        );
        assert!(
            allowed(&policy.evaluate(&AccessAsk::person_in(
                action,
                &live_ext_a,
                &org,
                &owner,
                &ext_a,
            ))),
            "same extension namespace allows {action:?}"
        );
        // Cross-namespace access is denied in all directions.
        assert!(
            !allowed(&policy.evaluate(&AccessAsk::person_in(
                action,
                &live_default,
                &org,
                &owner,
                &ext_a,
            ))),
            "extension cannot access default nessa conversation for {action:?}"
        );
        assert!(
            !allowed(&policy.evaluate(&AccessAsk::person_in(
                action,
                &live_ext_a,
                &org,
                &owner,
                &default_ns,
            ))),
            "default nessa cannot access extension conversation for {action:?}"
        );
        assert!(
            !allowed(&policy.evaluate(&AccessAsk::person_in(
                action,
                &live_ext_a,
                &org,
                &owner,
                &ext_b,
            ))),
            "extension B cannot access extension A conversation for {action:?}"
        );
    }

    // Deleted conversation erase also requires matching namespace.
    assert!(allowed(&policy.evaluate(&AccessAsk::person_in(
        AccessAction::Erase,
        &deleted_ext_a,
        &org,
        &owner,
        &ext_a,
    ))));
    assert!(!allowed(&policy.evaluate(&AccessAsk::person_in(
        AccessAction::Erase,
        &deleted_ext_a,
        &org,
        &owner,
        &default_ns,
    ))));

    // Catalogue actions require matching namespace.
    for action in [
        AccessAction::List,
        AccessAction::ChannelCreate,
        AccessAction::ChannelList,
        AccessAction::TerminalUse,
    ] {
        let ask = |caller_ns: &AppNamespace, cat_ns: &AppNamespace| {
            allowed(&policy.evaluate(&AccessAsk {
                subject: AccessSubject::Person {
                    organization: &org,
                    principal: &owner,
                    app_namespace: caller_ns,
                },
                action,
                resource: AccessResource::Catalogue {
                    organization: &org,
                    owner: &owner,
                    app_namespace: cat_ns,
                },
                held: None,
                request: RequestShape::default(),
                target: None,
            }))
        };
        assert!(ask(&default_ns, &default_ns));
        assert!(ask(&ext_a, &ext_a));
        assert!(!ask(&ext_a, &default_ns));
        assert!(!ask(&default_ns, &ext_a));
        assert!(!ask(&ext_b, &ext_a));
    }

    // Owner share management (Unshare, Shares) requires matching namespace.
    for action in [AccessAction::Unshare, AccessAction::Shares] {
        assert!(allowed(&policy.evaluate(&AccessAsk::person_in(
            action,
            &live_ext_a,
            &org,
            &owner,
            &ext_a,
        ))));
        assert!(!allowed(&policy.evaluate(&AccessAsk::person_in(
            action,
            &live_ext_a,
            &org,
            &owner,
            &default_ns,
        ))));
        assert!(!allowed(&policy.evaluate(&AccessAsk::person_in(
            action,
            &live_default,
            &org,
            &owner,
            &ext_a,
        ))));
    }

    // Review notices (both owner and admin) require matching namespace.
    for membership in [MembershipRole::Member, MembershipRole::Admin] {
        let review = |recipient_ns: &AppNamespace, target: &Conversation| {
            allowed(&policy.evaluate(&AccessAsk {
                subject: AccessSubject::ReviewRecipient {
                    organization: &org,
                    principal: &owner,
                    membership,
                    app_namespace: recipient_ns,
                },
                action: AccessAction::ReviewNotice,
                resource: AccessResource::of(target),
                held: None,
                request: RequestShape::default(),
                target: None,
            }))
        };
        assert!(review(&default_ns, &live_default));
        assert!(review(&ext_a, &live_ext_a));
        assert!(!review(&default_ns, &live_ext_a));
        assert!(!review(&ext_a, &live_default));
        assert!(!review(&ext_b, &live_ext_a));
    }
}
