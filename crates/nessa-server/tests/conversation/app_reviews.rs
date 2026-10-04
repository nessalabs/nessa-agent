//! A conversation's apps ("A review" and "A conversation's apps",
//! `docs/design/mcp-app-calls.md`): a review ends exactly once — allowed or
//! denied by the person, expired, or withdrawn by its request going, its
//! mount's release, or its conversation's end, by whoever did it — and
//! nothing is admitted, opens or is issued for a mount released or an
//! opening ended.
use super::*;
use crate::conversation::application::mcp_apps::{McpAppInitiator, McpAppRef, McpAppWithdrawal};
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

fn releaser() -> McpAppInitiator {
    McpAppInitiator::Person {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "pane".into(),
        request_id: "release-1".into(),
    }
}

/// The epoch of a conversation's first opening.
const EPOCH: u64 = 1;

/// A conversation's apps, in its first opening.
fn reviews() -> Arc<AppReviews> {
    let reviews = Arc::new(AppReviews::default());
    assert_eq!(reviews.begin(), EPOCH);
    reviews
}

fn open(reviews: &Arc<AppReviews>, instance: &str) -> Result<Waiting, ReviewRefusal> {
    reviews.open(
        EPOCH,
        new_review_id(),
        &app(instance),
        "charts",
        "delete_rows",
        "{}",
    )
}

#[tokio::test]
async fn a_review_is_shown_with_its_app_origin_and_ends_as_answered() {
    let reviews = reviews();
    let waiting = reviews
        .open(
            EPOCH,
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
    // The review names the tool the app asked to call, as its origin does.
    assert_eq!(shown[0].tool_name, "delete_rows");
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
    let denied = open(&reviews, "i1").unwrap();
    let cancelled = open(&reviews, "i1").unwrap();
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
async fn an_ended_review_is_no_longer_the_apps_and_a_wrong_answer_changes_nothing() {
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
    let id = waiting.permission_id.clone();
    assert_eq!(
        reviews.answer("e1", &id, ALLOW, person()),
        ReviewAnswer::Ended
    );
    // Ended: the identity names no open app review, so it is the agent's to
    // answer — which answers it stale itself — and nothing happens here.
    assert_eq!(
        reviews.answer("e1", &id, DENY, person()),
        ReviewAnswer::NotAnAppReview
    );
    assert_eq!(
        waiting.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Allowed(person())
    );
    // Open, but answered for another execution or with an option it did not
    // offer: stale, and still open.
    let other = open(&reviews, "i1").unwrap();
    let other_id = other.permission_id.clone();
    assert_eq!(
        reviews.answer("e2", &other_id, ALLOW, person()),
        ReviewAnswer::Stale
    );
    assert_eq!(
        reviews.answer("e1", &other_id, "maybe", person()),
        ReviewAnswer::Stale
    );
    assert_eq!(reviews.reviews().len(), 1);
    drop(other);
}

#[test]
fn an_agents_review_is_the_agents_whatever_it_is_named() {
    // An agent names its own reviews: one named as an app's would be is
    // still the agent's, since no app review is open by that name.
    let reviews = reviews();
    for name in ["1", "app-1", "app-6f1d6c0e-8f8c-4a52-9b8e-1f6c3d2a4b5c"] {
        assert_eq!(
            reviews.answer("e1", name, ALLOW, person()),
            ReviewAnswer::NotAnAppReview,
            "{name}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_review_nobody_answers_expires_at_its_deadline() {
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
    assert_eq!(waiting.ended(APP_REVIEW_DEADLINE).await, ReviewEnd::Expired);
    assert!(reviews.reviews().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_answer_at_the_deadline_is_what_the_review_ended_with() {
    // The deadline fires, but the answer ended the review first: the answer
    // is not lost to the expiry.
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
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
        ReviewAnswer::NotAnAppReview => assert_eq!(end, ReviewEnd::Expired),
        ReviewAnswer::Stale => panic!("the right execution and option"),
    }
}

#[tokio::test]
async fn releasing_a_mount_withdraws_only_its_own_reviews_as_the_releasers() {
    let reviews = reviews();
    let mine = open(&reviews, "i1").unwrap();
    let other_mount = open(&reviews, "i2").unwrap();
    let mut released = 0;
    reviews.release_app(&app("i1"), &releaser(), || released += 1);
    assert_eq!(released, 1);
    assert_eq!(
        mine.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Withdrawn {
            cause: McpAppWithdrawal::AppTornDown,
            by: Some(releaser()),
        }
    );
    let left = reviews.reviews();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].permission_id, other_mount.permission_id);
    // Releasing it again withdraws nothing more.
    reviews.release_app(&app("i1"), &releaser(), || {});
    assert_eq!(reviews.reviews().len(), 1);
    drop(other_mount);
}

#[tokio::test]
async fn a_released_mount_opens_and_is_issued_nothing_again() {
    let reviews = reviews();
    reviews.release_app(&app("i1"), &releaser(), || {});
    assert_eq!(open(&reviews, "i1").err(), Some(ReviewRefusal::Released));
    assert_eq!(
        reviews.issue(EPOCH, &app("i1"), || "ticket").err(),
        Some(ReviewRefusal::Released)
    );
    // Another mount of the same tool call is its own.
    assert!(open(&reviews, "i2").is_ok());
    assert_eq!(reviews.issue(EPOCH, &app("i2"), || "ticket"), Ok("ticket"));
}

#[tokio::test]
async fn a_conversation_ending_withdraws_every_review_as_whoever_ended_it() {
    let reviews = reviews();
    let first = open(&reviews, "i1").unwrap();
    let second = reviews
        .open(EPOCH, new_review_id(), &app("i2"), "files", "erase", "{}")
        .unwrap();
    let mut released = 0;
    reviews.end(EPOCH, &releaser(), || released += 1);
    assert_eq!(released, 1);
    for waiting in [first, second] {
        assert_eq!(
            waiting.ended(APP_REVIEW_DEADLINE).await,
            ReviewEnd::Withdrawn {
                cause: McpAppWithdrawal::ConversationEnded,
                by: Some(releaser()),
            }
        );
    }
    assert!(reviews.reviews().is_empty());
    // Ended once: a second end releases nothing again.
    reviews.end(EPOCH, &McpAppInitiator::System, || released += 1);
    assert_eq!(released, 1);
}

#[tokio::test]
async fn once_the_conversation_ended_nothing_opens_or_is_issued() {
    let reviews = reviews();
    reviews.end(EPOCH, &McpAppInitiator::System, || {});
    assert_eq!(open(&reviews, "i1").err(), Some(ReviewRefusal::Ended));
    assert_eq!(
        reviews.issue(EPOCH, &app("i1"), || "ticket").err(),
        Some(ReviewRefusal::Ended)
    );
    assert!(reviews.reviews().is_empty());
}

#[tokio::test]
async fn a_wait_dropped_before_its_review_ended_withdraws_it_as_the_apps() {
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
    let id = waiting.permission_id.clone();
    // The app's request went, mid-wait.
    let wait = tokio::spawn(waiting.ended(APP_REVIEW_DEADLINE));
    tokio::task::yield_now().await;
    wait.abort();
    let _ = wait.await;
    assert!(reviews.reviews().is_empty());
    assert_eq!(
        reviews.answer("e1", &id, ALLOW, person()),
        ReviewAnswer::NotAnAppReview
    );
}

#[tokio::test]
async fn past_the_count_no_review_opens_until_one_ends() {
    let reviews = reviews();
    let mut held: Vec<_> = (0..MAX_OPEN_APP_REVIEWS)
        .map(|_| open(&reviews, "i1").unwrap())
        .collect();
    assert_eq!(open(&reviews, "i1").err(), Some(ReviewRefusal::Full));
    drop(held.pop());
    assert!(open(&reviews, "i1").is_ok());
}

#[tokio::test]
async fn the_open_reviews_take_at_most_their_share_of_the_view() {
    let reviews = reviews();
    // One review alone past the share is never opened.
    let large = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_REVIEW_BYTES));
    assert!(!fits("app-x", &app("i1"), "charts", "delete_rows", &large));
    assert_eq!(
        reviews
            .open(
                EPOCH,
                new_review_id(),
                &app("i1"),
                "charts",
                "delete_rows",
                &large
            )
            .err(),
        Some(ReviewRefusal::TooLarge)
    );
    // Reviews that fit alone are opened until together they would not.
    let half = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_REVIEW_BYTES / 2));
    let first = reviews
        .open(
            EPOCH,
            new_review_id(),
            &app("i1"),
            "charts",
            "delete_rows",
            &half,
        )
        .unwrap();
    assert_eq!(
        reviews
            .open(
                EPOCH,
                new_review_id(),
                &app("i1"),
                "charts",
                "delete_rows",
                &half
            )
            .err(),
        Some(ReviewRefusal::Full)
    );
    // An ended review gives its share back.
    drop(first);
    assert!(reviews
        .open(
            EPOCH,
            new_review_id(),
            &app("i1"),
            "charts",
            "delete_rows",
            &half
        )
        .is_ok());
}

#[test]
fn a_conversation_remembers_its_last_released_mounts() {
    let reviews = reviews();
    for mount in 0..=MAX_RELEASED_MOUNTS {
        reviews.release_app(&app(&mount.to_string()), &releaser(), || {});
    }
    // The newest are remembered; the oldest, one past the bound, is not.
    assert_eq!(
        reviews
            .issue(EPOCH, &app(&MAX_RELEASED_MOUNTS.to_string()), || ())
            .err(),
        Some(ReviewRefusal::Released)
    );
    assert_eq!(
        reviews.issue(EPOCH, &app("1"), || ()).err(),
        Some(ReviewRefusal::Released)
    );
    assert_eq!(reviews.issue(EPOCH, &app("0"), || ()), Ok(()));
}

#[tokio::test]
async fn an_ended_opening_admits_nothing_and_the_next_opening_is_its_own() {
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
    reviews.end(EPOCH, &McpAppInitiator::System, || {});
    assert!(matches!(
        waiting.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Withdrawn { .. }
    ));
    assert_eq!(reviews.admit(EPOCH, &app("i1")), Err(ReviewRefusal::Ended));
    // Opened again: a call that resolved the old opening still admits,
    // opens and is issued nothing — the new one is another epoch.
    let next = reviews.begin();
    assert_ne!(next, EPOCH);
    assert_eq!(reviews.admit(EPOCH, &app("i1")), Err(ReviewRefusal::Ended));
    assert_eq!(open(&reviews, "i1").err(), Some(ReviewRefusal::Ended));
    assert_eq!(
        reviews.issue(EPOCH, &app("i1"), || ()).err(),
        Some(ReviewRefusal::Ended)
    );
    assert_eq!(reviews.admit(next, &app("i1")), Ok(()));
    // Ending the old opening again ends nothing of the new one.
    let mut released = 0;
    reviews.end(EPOCH, &McpAppInitiator::System, || released += 1);
    assert_eq!(released, 0);
    assert_eq!(reviews.admit(next, &app("i1")), Ok(()));
}

#[test]
fn a_release_outlasts_the_opening_it_came_in_and_any_before_one() {
    // Released before any opening, and kept through two.
    let reviews = AppReviews::default();
    reviews.release_app(&app("i1"), &releaser(), || {});
    let first = reviews.begin();
    assert_eq!(
        reviews.admit(first, &app("i1")),
        Err(ReviewRefusal::Released)
    );
    reviews.end(first, &McpAppInitiator::System, || {});
    let second = reviews.begin();
    assert_eq!(
        reviews.issue(second, &app("i1"), || ()).err(),
        Some(ReviewRefusal::Released)
    );
    assert_eq!(reviews.admit(second, &app("i2")), Ok(()));
}

#[tokio::test]
async fn a_deleted_conversations_apps_take_nothing_and_keep_nothing() {
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
    let mut released = 0;
    reviews.delete(|| released += 1);
    assert_eq!(released, 1);
    assert!(matches!(
        waiting.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Withdrawn {
            cause: McpAppWithdrawal::ConversationEnded,
            ..
        }
    ));
    // No opening begins again, and a release racing it keeps nothing.
    let after = reviews.begin();
    assert_eq!(reviews.admit(after, &app("i2")), Err(ReviewRefusal::Ended));
    reviews.release_app(&app("i3"), &releaser(), || {});
    assert_eq!(reviews.admit(after, &app("i3")), Err(ReviewRefusal::Ended));
    // Its opening ended already: what it held was let go then, not again.
    let ended = AppReviews::default();
    let epoch = ended.begin();
    ended.end(epoch, &McpAppInitiator::System, || {});
    let mut again = 0;
    ended.delete(|| again += 1);
    assert_eq!(again, 0);
}

#[tokio::test]
async fn an_opening_never_carries_another_s_reviews() {
    let reviews = reviews();
    let waiting = open(&reviews, "i1").unwrap();
    // Should an opening ever begin over one not ended, its reviews end.
    let next = reviews.begin();
    assert!(matches!(
        waiting.ended(APP_REVIEW_DEADLINE).await,
        ReviewEnd::Withdrawn {
            cause: McpAppWithdrawal::ConversationEnded,
            by: Some(McpAppInitiator::System),
        }
    ));
    assert!(reviews.reviews().is_empty());
    assert_eq!(reviews.admit(next, &app("i1")), Ok(()));
}
