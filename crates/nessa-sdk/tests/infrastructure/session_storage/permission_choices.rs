//! Persisted choices retain ordered identities and exact effect/scope decisions.
use super::*;
use serde_json::Value;

fn review_snapshot(name: &str, decisions: Vec<PermissionDecision>) -> SessionSnapshot {
    let mut value = snapshot(name);
    let execution = value.invocations[0].request.execution_id.clone();
    let tool = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        None,
    );
    let policy = PermissionOfferPolicy::new(decisions.clone()).unwrap();
    let mut options = decisions
        .into_iter()
        .enumerate()
        .map(|(index, decision)| {
            PermissionOption::new(
                PermissionOptionId::new(format!("option-{index}")).unwrap(),
                format!("Choice {index}"),
                decision,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    // Equal decisions under different option IDs are valid and remain distinct.
    options.push(
        PermissionOption::new(
            PermissionOptionId::new("same-decision").unwrap(),
            "Same decision, another offered choice",
            options[0].decision().clone(),
        )
        .unwrap(),
    );
    let options = PermissionOptions::new(options, &policy).unwrap();
    value.invocations[0].events = vec![
        ExecutionEvent::new(execution.clone(), ExecutionUpdate::Tool(tool.clone())),
        ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::PermissionRequested {
                id: PermissionId::new("review").unwrap(),
                tool_id: tool.id().clone(),
                observation: ToolObservation::default().with_update(tool),
                input: ToolReviewInput {
                    name: "test".into(),
                    arguments_json: "{}".into(),
                },
                options,
            },
        ),
    ];
    value
}

#[tokio::test]
async fn file_decode_preserves_many_exact_scoped_decisions_and_choice_order() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory).unwrap();
    let mut decisions = vec![PermissionDecision::new(
        PermissionEffect::Allow,
        PermissionScope::request(),
    )];
    for index in 0..5_000 {
        for effect in [PermissionEffect::Allow, PermissionEffect::Deny] {
            decisions.push(PermissionDecision::new(
                effect,
                PermissionScope::application(
                    PermissionApplicationId::new(format!("app-{index}")).unwrap(),
                ),
            ));
            decisions.push(PermissionDecision::new(
                effect,
                PermissionScope::session(
                    PermissionApplicationId::new(format!("app-{index}")).unwrap(),
                    PermissionSessionId::new("same-session-name").unwrap(),
                ),
            ));
        }
    }
    let value = review_snapshot("many-choices", decisions);
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    let loaded = lease.load().await.unwrap().unwrap();
    assert_same(&loaded, &value);
}

#[tokio::test]
async fn file_decode_rejects_duplicate_option_ids_with_equal_or_different_decisions() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let value = review_snapshot(
        "duplicate-choices",
        vec![
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        ],
    );
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    let path = journal_path(&directory, "duplicate-choices");
    let original: Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    // Index 1 differs in effect; index 2 shares the first decision exactly.
    for duplicate in [1, 2] {
        let mut invalid = original.clone();
        let choices = invalid
            .pointer_mut("/invocations/0/events/1/update/PermissionRequested/options")
            .expect("review choices")
            .as_array_mut()
            .unwrap();
        choices[duplicate]["id"] = choices[0]["id"].clone();
        let bytes = journal_bytes(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
