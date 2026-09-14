use super::support::*;
use nessa_sdk::domain::agent_execution::sessions::{ExecutionSession, ExecutionSessionId};

#[test]
fn custom_cancellation_reasons_validate_byte_bounds_and_preserve_code_and_explanation() {
    let custom = CustomPermissionCancellationReason::new(
        "guard.cost_limit",
        "Estimated cost exceeds the configured limit.",
    )
    .unwrap();
    assert_eq!(custom.code(), "guard.cost_limit");
    assert_eq!(
        custom.explanation(),
        "Estimated cost exceeds the configured limit."
    );
    for (code, explanation, field) in [
        ("", "why", "cancellation reason code"),
        (" \n", "why", "cancellation reason code"),
        ("guard", "", "cancellation reason explanation"),
        ("guard", " \t", "cancellation reason explanation"),
    ] {
        assert_eq!(
            CustomPermissionCancellationReason::new(code, explanation),
            Err(ExecutionError::EmptyValue(field))
        );
    }
    let code = "é".repeat(CustomPermissionCancellationReason::MAX_CODE_BYTES / 2);
    let explanation = "é".repeat(CustomPermissionCancellationReason::MAX_EXPLANATION_BYTES / 2);
    let boundary = CustomPermissionCancellationReason::new(&code, &explanation).unwrap();
    assert_eq!(boundary.code(), code);
    assert_eq!(boundary.explanation(), explanation);
    assert_eq!(
        CustomPermissionCancellationReason::new(format!("{code}x"), &explanation),
        Err(ExecutionError::ValueTooLong {
            field: "cancellation reason code",
            max_bytes: CustomPermissionCancellationReason::MAX_CODE_BYTES
        })
    );
    assert_eq!(
        CustomPermissionCancellationReason::new(&code, format!("{explanation}x")),
        Err(ExecutionError::ValueTooLong {
            field: "cancellation reason explanation",
            max_bytes: CustomPermissionCancellationReason::MAX_EXPLANATION_BYTES
        })
    );
}

#[test]
fn permission_configuration_preserves_exact_choices_and_filters_disallowed_kinds() {
    assert!(PermissionOptionId::new(" ").is_err());
    let id = PermissionOptionId::new("option").unwrap();
    assert_eq!(id.as_str(), "option");
    assert!(PermissionOption::new(
        id,
        " ",
        PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request())
    )
    .is_err());
    assert_eq!(
        PermissionOfferPolicy::new(vec![]),
        Err(ExecutionError::NoPermissionOptions)
    );
    assert_eq!(
        PermissionOfferPolicy::new(vec![
            PermissionDecision::new(
                PermissionEffect::Allow,
                PermissionScope::request()
            );
            2
        ]),
        Err(ExecutionError::DuplicatePermissionDecision)
    );
    let config = PermissionOfferPolicy::new(vec![
        PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        PermissionDecision::new(
            PermissionEffect::Allow,
            PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
        ),
        PermissionDecision::new(
            PermissionEffect::Deny,
            PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
        ),
    ])
    .unwrap();
    let options = vec![
        choice(
            "once",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        ),
        choice(
            "file",
            PermissionDecision::new(
                PermissionEffect::Allow,
                PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
            ),
        ),
        choice(
            "directory",
            PermissionDecision::new(
                PermissionEffect::Allow,
                PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
            ),
        ),
        choice(
            "deny",
            PermissionDecision::new(
                PermissionEffect::Deny,
                PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
            ),
        ),
    ];
    let all = PermissionOptions::new(options.clone(), &config).unwrap();
    assert_eq!(all.choices(), options);
    assert_eq!(
        all.find(&PermissionOptionId::new("directory").unwrap())
            .unwrap()
            .label(),
        "Choice directory"
    );
    let filtered =
        PermissionOptions::new(options.clone(), &PermissionOfferPolicy::once_only()).unwrap();
    assert_eq!(filtered.choices(), &options[..1]);
    assert!(filtered
        .find(&PermissionOptionId::new("file").unwrap())
        .is_none());
    assert_eq!(
        PermissionOptions::new(vec![], &config),
        Err(ExecutionError::NoPermissionOptions)
    );
    assert_eq!(
        PermissionOptions::new(
            vec![choice(
                "always",
                PermissionDecision::new(
                    PermissionEffect::Allow,
                    PermissionScope::application(PermissionApplicationId::new("app").unwrap())
                )
            )],
            &PermissionOfferPolicy::once_only()
        ),
        Err(ExecutionError::NoPermissionOptions)
    );
    assert_eq!(
        PermissionOptions::new(
            vec![
                choice(
                    "duplicate",
                    PermissionDecision::new(
                        PermissionEffect::Allow,
                        PermissionScope::application(PermissionApplicationId::new("app").unwrap())
                    )
                ),
                choice(
                    "duplicate",
                    PermissionDecision::new(
                        PermissionEffect::Deny,
                        PermissionScope::application(PermissionApplicationId::new("app").unwrap())
                    )
                )
            ],
            &PermissionOfferPolicy::once_only()
        ),
        Err(ExecutionError::DuplicatePermissionOption)
    );
    for id in ["file", "directory", "deny"] {
        let permission = PermissionRequest::new(
            PermissionId::new("request").unwrap(),
            ExecutionId::new("execution").unwrap(),
            ToolCallId::new("tool").unwrap(),
            all.clone(),
        );
        let execution = permission.execution_id().clone();
        let permission_id = permission.id().clone();
        let mut session = ExecutionSession::new(ExecutionSessionId::new("options").unwrap());
        session.begin_execution(execution.clone()).unwrap();
        session.observe_tool(&execution, update("tool")).unwrap();
        session.request_permission(permission).unwrap();
        let option_id = PermissionOptionId::new(id).unwrap();
        let expected = all.find(&option_id).unwrap().decision().clone();
        let permission = session
            .answer_permission(&execution, &permission_id, &option_id)
            .unwrap();
        assert_eq!(
            permission.state(),
            PermissionStateView::Answered {
                option_id: &option_id,
                decision: &expected
            }
        );
    }
}

#[test]
fn permission_configuration_does_not_confuse_sessions_or_application_boundaries() {
    let app = PermissionApplicationId::new("app-a").unwrap();
    let session = PermissionSessionId::new("session-a").unwrap();
    assert_eq!(app.as_str(), "app-a");
    assert_eq!(session.as_str(), "session-a");
    assert!(PermissionApplicationId::new(" ").is_err());
    assert!(PermissionSessionId::new("").is_err());
    let allowed = PermissionDecision::new(
        PermissionEffect::Allow,
        PermissionScope::session(app.clone(), session.clone()),
    );
    let config = PermissionOfferPolicy::new(vec![allowed.clone()]).unwrap();
    let choices = vec![
        choice("correct", allowed.clone()),
        choice(
            "other-session",
            PermissionDecision::new(
                PermissionEffect::Allow,
                PermissionScope::session(
                    app.clone(),
                    PermissionSessionId::new("session-b").unwrap(),
                ),
            ),
        ),
        choice(
            "other-app",
            PermissionDecision::new(
                PermissionEffect::Allow,
                PermissionScope::session(
                    PermissionApplicationId::new("app-b").unwrap(),
                    session.clone(),
                ),
            ),
        ),
        choice(
            "whole-app",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::application(app)),
        ),
    ];
    let offered = PermissionOptions::new(choices, &config).unwrap();
    assert_eq!(offered.choices(), &[choice("correct", allowed)]);
}

#[test]
fn request_only_policy_exposes_its_ordered_decisions() {
    assert_eq!(
        PermissionOfferPolicy::once_only().decisions(),
        &[
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        ]
    );
}

#[test]
fn large_option_lists_check_all_identities_before_filtering_and_preserve_order() {
    let allowed = PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request());
    let denied = PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request());
    let policy = PermissionOfferPolicy::new(vec![allowed.clone()]).unwrap();
    let options = (0..20_000)
        .map(|index| {
            choice(
                &format!("option-{index}"),
                if index % 2 == 0 {
                    allowed.clone()
                } else {
                    denied.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    let filtered = PermissionOptions::new(options.clone(), &policy).unwrap();
    assert_eq!(filtered.choices().len(), 10_000);
    for (index, option) in filtered.choices().iter().enumerate() {
        assert_eq!(option.id().as_str(), format!("option-{}", index * 2));
    }
    for duplicate in [0, 1, 19_999] {
        let mut repeated = options.clone();
        repeated.push(options[duplicate].clone());
        assert_eq!(
            PermissionOptions::new(repeated, &policy),
            Err(ExecutionError::DuplicatePermissionOption)
        );
    }
}

#[test]
fn large_policies_check_exact_decisions_and_filter_without_reordering() {
    let decisions = (0..20_000)
        .map(|index| {
            PermissionDecision::new(
                PermissionEffect::Allow,
                PermissionScope::application(
                    PermissionApplicationId::new(format!("app-{index}")).unwrap(),
                ),
            )
        })
        .collect::<Vec<_>>();
    let policy = PermissionOfferPolicy::new(decisions.clone()).unwrap();
    assert_eq!(policy.decisions(), decisions.as_slice());
    for duplicate in [0, 10_000, 19_999] {
        let mut repeated = decisions.clone();
        repeated.push(decisions[duplicate].clone());
        assert_eq!(
            PermissionOfferPolicy::new(repeated),
            Err(ExecutionError::DuplicatePermissionDecision)
        );
    }
    let options = decisions
        .iter()
        .enumerate()
        .rev()
        .map(|(index, decision)| choice(&format!("option-{index}"), decision.clone()))
        .collect();
    let filtered = PermissionOptions::new(options, &policy).unwrap();
    assert_eq!(filtered.choices().len(), decisions.len());
    for (index, option) in filtered.choices().iter().enumerate() {
        assert_eq!(option.decision(), &decisions[decisions.len() - index - 1]);
    }
}

#[test]
fn policy_membership_requires_the_exact_effect_and_qualified_scope() {
    let scopes = [
        PermissionScope::request(),
        PermissionScope::application(PermissionApplicationId::new("app-a").unwrap()),
        PermissionScope::application(PermissionApplicationId::new("app-b").unwrap()),
        PermissionScope::session(
            PermissionApplicationId::new("app-a").unwrap(),
            PermissionSessionId::new("session-a").unwrap(),
        ),
        PermissionScope::session(
            PermissionApplicationId::new("app-a").unwrap(),
            PermissionSessionId::new("session-b").unwrap(),
        ),
        PermissionScope::session(
            PermissionApplicationId::new("app-b").unwrap(),
            PermissionSessionId::new("session-a").unwrap(),
        ),
    ];
    let decisions = scopes
        .into_iter()
        .flat_map(|scope| {
            [PermissionEffect::Allow, PermissionEffect::Deny]
                .map(|effect| PermissionDecision::new(effect, scope.clone()))
        })
        .collect::<Vec<_>>();
    for (allowed_index, allowed) in decisions.iter().enumerate() {
        let policy = PermissionOfferPolicy::new(vec![allowed.clone()]).unwrap();
        for (candidate_index, candidate) in decisions.iter().enumerate() {
            assert_eq!(
                policy.allows(candidate),
                allowed_index == candidate_index,
                "allowed {allowed:?}, candidate {candidate:?}"
            );
        }
        assert_eq!(policy.decisions(), std::slice::from_ref(allowed));
    }
}

#[test]
fn scopes_borrow_exact_validated_identities_and_replacements_leave_snapshots_unchanged() {
    let application = PermissionApplicationId::new(" application ").unwrap();
    let session = PermissionSessionId::new(" session ").unwrap();
    let mut scope = PermissionScope::session(application.clone(), session.clone());
    let snapshot = scope.clone();
    assert_eq!(
        scope.view(),
        PermissionScopeView::Session {
            application_id: &application,
            session_id: &session
        }
    );
    scope = PermissionScope::application(application.clone());
    assert_eq!(scope.view(), PermissionScopeView::Application(&application));
    assert_eq!(
        snapshot.view(),
        PermissionScopeView::Session {
            application_id: &application,
            session_id: &session
        }
    );
    assert_eq!(
        PermissionScope::request().view(),
        PermissionScopeView::Request
    );
}

#[test]
fn cancellation_reason_replacement_preserves_the_prior_custom_evidence() {
    let custom =
        CustomPermissionCancellationReason::new("guard.budget", " Budget reached ").unwrap();
    let mut reason = PermissionCancellationReason::custom(custom.clone());
    let recorded = reason.clone();
    assert_eq!(
        reason.view(),
        PermissionCancellationReasonView::Custom(&custom)
    );
    reason = PermissionCancellationReason::session_closed();
    assert_eq!(
        reason.view(),
        PermissionCancellationReasonView::SessionClosed
    );
    let PermissionCancellationReasonView::Custom(retained) = recorded.view() else {
        panic!("replacement cannot alter previously recorded custom evidence");
    };
    assert_eq!(retained.code(), "guard.budget");
    assert_eq!(retained.explanation(), " Budget reached ");
}
