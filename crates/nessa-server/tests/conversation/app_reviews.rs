//! An app's review ends exactly once ("Approval of a destructive call"
//! table, #348): allowed or denied by the person, expired, or withdrawn —
//! by its request going, by its mount being released, or by its
//! conversation ending — and an answer to an ended one is stale.
use super::*;
use crate::conversation::application::mcp_apps::{McpAppRef, McpAppWithdrawal};
use nessa_auth::domain::PrincipalId;
use std::sync::Arc;
use std::time::Duration;

fn app(instance: &str) -> McpAppRef {
    McpAppRef {
        execution_id: "e1".into(),
        tool_id: "t1".into(),
        instance_id: instance.into(),
    }
}

fn person() -> ReviewAnswerer {
    ReviewAnswerer {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        request_id: "answer-1".into(),
    }
}

fn reviews() -> Arc<AppReviews> {
    Arc::new(AppReviews::default())
}

#[tokio::test]
async fn a_review_is_shown_with_its_app_origin_and_ends_as_answered() {
    let reviews = reviews();
    let waiting = reviews
        .open(
            new_review_id(),
            &app("i1"),
            "charts",
            "delete_rows",
            "{\"id\":1}",
        )
        .unwrap();
    let shown = reviews.reviews();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].execution_id, "e1");
    assert_eq!(shown[0].tool_id, "t1");
    assert_eq!(shown[0].permission_id, waiting.permission_id);
    assert_eq!(shown[0].arguments_json, "{\"id\":1}");
    assert_eq!(
        shown[0].origin,
        ConversationPermissionOrigin::App {
            server: "charts".into(),
            tool: "delete_rows".into()
        }
    );
    let options: Vec<_> = shown[0]
        .options
        .iter()
        .map(|option| (option.id.as_str(), option.effect))
        .collect();
    assert_eq!(
        options,
        [
            (ALLOW, ConversationPermissionOptionEffect::Allow),
            (DENY, ConversationPermissionOptionEffect::Deny)
        ]
    );
    let id = waiting.permission_id.clone();
    assert_eq!(
        reviews.answer("e1", &id, ALLOW, person()),
        ReviewAnswer::Ended
    );
    assert_eq!(
        waiting.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Allowed(person())
    );
    assert!(reviews.reviews().is_empty());
}

#[tokio::test]
async fn a_denial_and_a_cancellation_both_deny() {
    let reviews = reviews();
    let denied = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let cancelled = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let (first, second) = (
        denied.permission_id.clone(),
        cancelled.permission_id.clone(),
    );
    assert_eq!(
        reviews.answer("e1", &first, DENY, person()),
        ReviewAnswer::Ended
    );
    assert_eq!(reviews.cancel("e1", &second, person()), ReviewAnswer::Ended);
    assert_eq!(
        denied.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Denied(person())
    );
    assert_eq!(
        cancelled.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Denied(person())
    );
}

#[tokio::test]
async fn an_answer_to_an_ended_review_is_stale_and_has_no_second_effect() {
    let reviews = reviews();
    let waiting = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let id = waiting.permission_id.clone();
    assert_eq!(
        reviews.answer("e1", &id, ALLOW, person()),
        ReviewAnswer::Ended
    );
    // Again, the other way, or for another execution: stale, nothing done.
    assert_eq!(
        reviews.answer("e1", &id, DENY, person()),
        ReviewAnswer::Stale
    );
    assert_eq!(
        waiting.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Allowed(person())
    );
    let other = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let other_id = other.permission_id.clone();
    assert_eq!(
        reviews.answer("e2", &other_id, ALLOW, person()),
        ReviewAnswer::Stale
    );
    assert_eq!(
        reviews.answer("e1", &other_id, "maybe", person()),
        ReviewAnswer::Stale
    );
    // Still open after those.
    assert_eq!(reviews.reviews().len(), 1);
    // An agent's review is not an app's.
    assert_eq!(
        reviews.answer("e1", "1", ALLOW, person()),
        ReviewAnswer::NotAnAppReview
    );
    drop(other);
}

#[tokio::test(start_paused = true)]
async fn a_review_nobody_answers_expires_at_its_deadline() {
    let reviews = reviews();
    let waiting = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let id = waiting.permission_id.clone();
    assert_eq!(waiting.ended(APP_REVIEW_DEADLINE).await, ReviewEnd::Expired);
    assert!(reviews.reviews().is_empty());
    // Answered after it expired: stale.
    assert_eq!(
        reviews.answer("e1", &id, ALLOW, person()),
        ReviewAnswer::Stale
    );
}

#[tokio::test(start_paused = true)]
async fn an_answer_at_the_deadline_is_what_the_review_ended_with() {
    // The deadline fires, but the answer ended the review first: the answer
    // is not lost to the expiry.
    let reviews = reviews();
    let waiting = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let id = waiting.permission_id.clone();
    let deadline = Duration::from_secs(1);
    let answering = {
        let reviews = reviews.clone();
        async move {
            tokio::time::sleep(deadline).await;
            reviews.answer("e1", &id, ALLOW, person())
        }
    };
    let (end, answered) = tokio::join!(waiting.ended(deadline), answering);
    match answered {
        ReviewAnswer::Ended => assert_eq!(end, ReviewEnd::Allowed(person())),
        ReviewAnswer::Stale => assert_eq!(end, ReviewEnd::Expired),
        ReviewAnswer::NotAnAppReview => panic!("an app review"),
    }
}

#[tokio::test]
async fn releasing_a_mount_withdraws_only_its_own_reviews() {
    let reviews = reviews();
    let mine = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let other_mount = reviews
        .open(new_review_id(), &app("i2"), "charts", "delete_rows", "{}")
        .unwrap();
    reviews.withdraw_app(&app("i1"), McpAppWithdrawal::AppTornDown);
    assert_eq!(
        mine.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Withdrawn(McpAppWithdrawal::AppTornDown)
    );
    let left = reviews.reviews();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].permission_id, other_mount.permission_id);
    // Releasing it again is nothing.
    reviews.withdraw_app(&app("i1"), McpAppWithdrawal::AppTornDown);
    assert_eq!(reviews.reviews().len(), 1);
    drop(other_mount);
}

#[tokio::test]
async fn a_conversation_ending_withdraws_every_review() {
    let reviews = reviews();
    let first = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let second = reviews
        .open(new_review_id(), &app("i2"), "files", "erase", "{}")
        .unwrap();
    reviews.end();
    for waiting in [first, second] {
        assert_eq!(
            waiting.ended(APP_REVIEW_DEADLINE).await,
            ReviewEnd::Withdrawn(McpAppWithdrawal::ConversationEnded)
        );
    }
    assert!(reviews.reviews().is_empty());
}

#[tokio::test]
async fn a_wait_dropped_before_its_review_ended_withdraws_it() {
    let reviews = reviews();
    let waiting = reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .unwrap();
    let id = waiting.permission_id.clone();
    // The app's request went, mid-wait.
    let wait = tokio::spawn(waiting.ended(APP_REVIEW_DEADLINE));
    tokio::task::yield_now().await;
    wait.abort();
    let _ = wait.await;
    assert!(reviews.reviews().is_empty());
    assert_eq!(
        reviews.answer("e1", &id, ALLOW, person()),
        ReviewAnswer::Stale
    );
}

#[tokio::test]
async fn once_the_conversation_ended_no_review_opens() {
    let reviews = reviews();
    reviews.end();
    assert_eq!(
        reviews
            .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
            .err(),
        Some(ReviewRefusal::Ended)
    );
    assert!(reviews.reviews().is_empty());
}

#[tokio::test]
async fn past_the_cap_no_review_opens_until_one_ends() {
    let reviews = reviews();
    let mut open: Vec<_> = (0..MAX_OPEN_APP_REVIEWS)
        .map(|_| {
            reviews
                .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
                .unwrap()
        })
        .collect();
    assert_eq!(
        reviews
            .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
            .err(),
        Some(ReviewRefusal::Full)
    );
    drop(open.pop());
    assert!(reviews
        .open(new_review_id(), &app("i1"), "charts", "delete_rows", "{}")
        .is_ok());
}

#[test]
fn an_agent_review_named_like_an_app_s_but_not_one_is_the_agent_s() {
    let reviews = AppReviews::default();
    // Only an app-<uuid> it could have issued is its own.
    assert_eq!(
        reviews.answer("e1", "app-1", ALLOW, person()),
        ReviewAnswer::NotAnAppReview
    );
    assert_eq!(
        reviews.answer(
            "e1",
            "app-6f1d6c0e-8f8c-4a52-9b8e-1f6c3d2a4b5c",
            ALLOW,
            person()
        ),
        ReviewAnswer::Stale
    );
}
