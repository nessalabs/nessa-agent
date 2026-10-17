//! Weak owner reads preserve typed faults without retaining decisions.
use super::*;

#[test]
fn permission_authority_reports_poison_and_drop_without_retaining_owner() {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let authority = PermissionAuthority::new(
        ExecutionSessionId::new("session").unwrap(),
        ExecutionId::new("execution").unwrap(),
        &pending,
        &Arc::new(Mutex::new(HashMap::new())),
    );
    let id = PermissionId::new("permission").unwrap();
    let _ = std::panic::catch_unwind(|| {
        let _guard = pending.lock().unwrap();
        panic!("poison pending owner");
    });
    assert_eq!(
        authority.pending(&id),
        Err(PermissionAuthorityError::Poisoned)
    );
    drop(pending);
    assert_eq!(
        authority.pending(&id),
        Err(PermissionAuthorityError::Unavailable)
    );
}

fn ask() -> AgentQuestion {
    use crate::domain::agent_execution::questions::{AnswerOption, AnswerShape, Question};
    AgentQuestion::new(
        "m",
        vec![Question::new(
            "k",
            "p",
            None,
            AnswerShape::One,
            vec![AnswerOption::new("v", "l", None).unwrap()],
            None,
            false,
        )
        .unwrap()],
    )
    .unwrap()
}

fn review(id: &str, content: Option<ReviewContent>) -> PermissionRequest {
    use super::super::{
        PermissionDecision, PermissionEffect, PermissionOfferPolicy, PermissionOption,
        PermissionOptionId, PermissionScope,
    };
    let request = PermissionRequest::new(
        PermissionId::new(id).unwrap(),
        ExecutionId::new("execution").unwrap(),
        ToolCallId::new("tool").unwrap(),
        PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow once",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap(),
    );
    match content {
        Some(content) => request.with_content(content),
        None => request,
    }
}

#[test]
fn reviews_list_every_pending_review_with_content_oldest_first() {
    let first = ReviewContent::new(
        Some("Write file".into()),
        "write".into(),
        "{\"path\":\"a\"}".into(),
        None,
    );
    let second = ReviewContent::new(None, "read".into(), "{}".into(), None);
    assert!(first.opened() < second.opened());
    assert_ne!(first.key(), second.key());
    assert_eq!(first.key().len(), 32);
    assert_eq!(first.title(), Some("Write file"));
    assert_eq!(first.tool_name(), "write");
    assert_eq!(first.arguments_json(), "{\"path\":\"a\"}");
    assert_eq!(first.deadline(), None);

    let mut answered = review("answered", Some(first.clone()));
    answered
        .cancel(super::super::PermissionCancellationReason::session_closed())
        .unwrap();
    let pending = Arc::new(Mutex::new(HashMap::from([
        (
            PermissionId::new("second").unwrap(),
            review("second", Some(second.clone())),
        ),
        (
            PermissionId::new("first").unwrap(),
            review("first", Some(first.clone())),
        ),
        (PermissionId::new("unseen").unwrap(), review("unseen", None)),
        (PermissionId::new("answered").unwrap(), answered),
    ])));
    let questions = Arc::new(Mutex::new(HashMap::new()));
    let authority = PermissionAuthority::new(
        ExecutionSessionId::new("session").unwrap(),
        ExecutionId::new("execution").unwrap(),
        &pending,
        &questions,
    );
    let reviews = authority.reviews().unwrap();
    let ids: Vec<&str> = reviews.iter().map(|review| review.id.as_str()).collect();
    assert_eq!(ids, ["first", "second"]);
    assert_eq!(*reviews[0].content, first);
    assert_eq!(reviews[0].execution_id.as_str(), "execution");
    assert_eq!(reviews[0].tool_id.as_str(), "tool");

    let _ = std::panic::catch_unwind(|| {
        let _guard = pending.lock().unwrap();
        panic!("poison pending owner");
    });
    assert_eq!(authority.reviews(), Err(PermissionAuthorityError::Poisoned));
    drop(pending);
    assert_eq!(
        authority.reviews(),
        Err(PermissionAuthorityError::Unavailable)
    );
}

#[test]
fn questions_are_listed_while_open_and_never_after_their_owner_drops() {
    let open: OpenQuestions = Arc::new(Mutex::new(HashMap::new()));
    let authority = PermissionAuthority::new(
        ExecutionSessionId::new("session").unwrap(),
        ExecutionId::new("execution").unwrap(),
        &Arc::new(Mutex::new(HashMap::new())),
        &open,
    );
    let listing = QuestionListing::new(&open);
    let execution = ExecutionId::new("execution").unwrap();
    let first = PendingQuestion::new(execution.clone(), QuestionId::new("first").unwrap(), ask());
    let second = PendingQuestion::new(execution, QuestionId::new("second").unwrap(), ask());
    assert!(first.opened() < second.opened());
    assert_ne!(first.key(), second.key());
    listing.list(second.clone());
    listing.list(first.clone());
    assert_eq!(
        authority.questions().unwrap(),
        [first.clone(), second.clone()]
    );
    assert_eq!(listing.key_of(&first.id), Some(first.key()));
    listing.unlist(&first.id);
    assert_eq!(listing.key_of(&first.id), None);
    assert_eq!(
        authority.questions().unwrap(),
        std::slice::from_ref(&second)
    );

    let _ = std::panic::catch_unwind(|| {
        let _guard = open.lock().unwrap();
        panic!("poison open questions");
    });
    assert_eq!(
        authority.questions(),
        Err(PermissionAuthorityError::Poisoned)
    );
    assert_eq!(listing.key_of(&second.id), None);
    listing.list(first.clone());
    listing.unlist(&second.id);
    drop(open);
    assert_eq!(
        authority.questions(),
        Err(PermissionAuthorityError::Unavailable)
    );
    assert_eq!(listing.key_of(&second.id), None);
    listing.list(first.clone());
    listing.unlist(&first.id);
}
