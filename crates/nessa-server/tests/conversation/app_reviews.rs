//! A conversation's apps ("A review" and "A conversation's apps",
//! `docs/design/mcp-app-calls.md`): a review ends exactly once — allowed or
//! denied by the person, expired, or withdrawn by its request going, its
//! mount's release, or its conversation's end, by whoever did it — and
//! nothing is admitted, opens or is issued for a mount released or an
//! opening ended.
use super::*;
use crate::conversation::application::mcp_apps::{
    ContextDrop, DroppedContexts, McpAppAsk, McpAppAuditPhase, McpAppAuditRecord, McpAppInitiator,
    McpAppRef, McpAppWithdrawal,
};
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::OrganizationId;
use nessa_auth::domain::PrincipalId;
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{AppModelContext, McpAppSource},
    tools::{McpTool, ToolCallId},
};
use std::sync::{Arc, Mutex};
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

/// Each drop a conversation's apps reported, as they reported it.
#[derive(Default)]
struct Reported(Mutex<Vec<McpAppAuditRecord>>);
impl DroppedContexts for Reported {
    fn context_dropped(&self, record: McpAppAuditRecord) {
        self.0.lock().unwrap().push(record);
    }
}
impl Reported {
    /// The drops reported since last asked: each by the call of the update
    /// that held what it dropped, why, and by whom.
    fn take(&self) -> Vec<(String, ContextDrop, McpAppInitiator)> {
        std::mem::take(&mut *self.0.lock().unwrap())
            .into_iter()
            .map(|record| match record.phase {
                McpAppAuditPhase::ContextDropped { cause } => {
                    (record.call_id, cause, record.initiator)
                }
                other => panic!("reported as a drop: {other:?}"),
            })
            .collect()
    }
}

/// A conversation's apps, no opening begun, and what they report dropped.
fn unopened() -> (Arc<AppReviews>, Arc<Reported>) {
    let reported = Arc::new(Reported::default());
    (Arc::new(AppReviews::new(reported.clone())), reported)
}

/// A conversation's apps, in its first opening, and what they report
/// dropped.
fn reporting() -> (Arc<AppReviews>, Arc<Reported>) {
    let (reviews, reported) = unopened();
    assert_eq!(reviews.begin(), EPOCH);
    assert!(reported.take().is_empty());
    (reviews, reported)
}

/// A conversation's apps, in its first opening.
fn reviews() -> Arc<AppReviews> {
    reporting().0
}

fn open(reviews: &Arc<AppReviews>, instance: &str) -> Result<Waiting, ReviewRefusal> {
    reviews.open(
        EPOCH,
        new_review_id(),
        ReviewAsk::RunTool,
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
            ReviewAsk::RunTool,
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
        .open(
            EPOCH,
            new_review_id(),
            ReviewAsk::RunTool,
            &app("i2"),
            "files",
            "erase",
            "{}",
        )
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
    assert!(!fits(
        "app-x",
        ReviewAsk::RunTool,
        &app("i1"),
        "charts",
        "delete_rows",
        &large
    ));
    assert_eq!(
        reviews
            .open(
                EPOCH,
                new_review_id(),
                ReviewAsk::RunTool,
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
            ReviewAsk::RunTool,
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
                ReviewAsk::RunTool,
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
            ReviewAsk::RunTool,
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
    let (reviews, _) = unopened();
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
    reviews.delete(&releaser(), || released += 1);
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
    let (ended, _) = unopened();
    let epoch = ended.begin();
    ended.end(epoch, &McpAppInitiator::System, || {});
    let mut again = 0;
    ended.delete(&releaser(), || again += 1);
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

// --- An app in its conversation (#390): its messages' reviews, and the
// contexts it holds ("An app in its conversation: the gateway") -----------

/// A context the update `update` gave, saying `text`.
fn context(update: &str, text: &str) -> AppModelContext {
    AppModelContext::new(
        McpAppSource::new(
            ExecutionId::new("e1").unwrap(),
            ToolCallId::new("t1").unwrap(),
            McpTool::new("charts", "show").unwrap(),
        )
        .unwrap(),
        update,
        Some(text.into()),
        None,
    )
    .unwrap()
    .unwrap()
}

fn held(reviews: &AppReviews) -> Vec<String> {
    reviews
        .held()
        .iter()
        .map(|context| context.text().unwrap().to_owned())
        .collect()
}

/// The record of `mount`'s update whose call is `call`: what a drop of its
/// context is recorded against.
fn update(mount: &str, call: &str) -> McpAppAuditRecord {
    McpAppAuditRecord {
        conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        organization_id: OrganizationId::new("org").unwrap(),
        call_id: call.into(),
        request_id: format!("request-{call}"),
        app: app(mount),
        ask: McpAppAsk::UpdateModelContext {
            server: "charts".into(),
        },
        initiator: McpAppInitiator::System,
        phase: McpAppAuditPhase::ContextHeld { bytes: 1 },
    }
}

/// The calls of the updates `taken` names.
fn calls(taken: &[McpAppAuditRecord]) -> Vec<String> {
    taken.iter().map(|update| update.call_id.clone()).collect()
}

/// Each of `calls`' updates dropped for `cause` by `by`, in that order.
fn each(
    calls: &[&str],
    cause: ContextDrop,
    by: &McpAppInitiator,
) -> Vec<(String, ContextDrop, McpAppInitiator)> {
    calls
        .iter()
        .map(|call| ((*call).to_owned(), cause, by.clone()))
        .collect()
}

/// `mount`'s update in `epoch`, its call the context's own update: held as
/// the service holds one, once on record.
fn hold(
    reviews: &AppReviews,
    epoch: u64,
    mount: &str,
    context: Option<AppModelContext>,
) -> Result<(), NotHeld> {
    let call = context.as_ref().map_or_else(
        || "clear".to_owned(),
        |context| context.update_id().to_owned(),
    );
    reviews.hold(epoch, &app(mount), context, &update(mount, &call))
}

/// `mount`'s update in the first opening, as the service gives one: its
/// room asked, then held — `None` clears.
fn give(reviews: &AppReviews, mount: &str, context: Option<AppModelContext>) {
    reviews.room(EPOCH, &app(mount), context.is_some()).unwrap();
    hold(reviews, EPOCH, mount, context).unwrap();
}

#[test]
fn c5_c7_each_update_replaces_its_mounts_context_and_a_clear_drops_it() {
    let reviews = reviews();
    give(&reviews, "i1", Some(context("u1", "old")));
    give(&reviews, "i1", Some(context("u2", "new")));
    assert_eq!(held(&reviews), ["new"]);
    give(&reviews, "i2", Some(context("u3", "other")));
    // Held in the order given: a replacement is given last.
    give(&reviews, "i1", Some(context("u4", "newest")));
    assert_eq!(held(&reviews), ["other", "newest"]);
    give(&reviews, "i1", None);
    assert_eq!(held(&reviews), ["other"]);
}

#[test]
fn c6_a_fifth_mount_finds_no_room_and_a_mount_holds_a_place_only_while_it_holds_a_context() {
    let reviews = reviews();
    for n in 0..MAX_HELD_CONTEXTS {
        give(
            &reviews,
            &format!("i{n}"),
            Some(context(&format!("u{n}"), "x")),
        );
    }
    assert_eq!(MAX_HELD_CONTEXTS, 4);
    assert_eq!(
        reviews.room(EPOCH, &app("other"), true),
        Err(ContextRefusal::Full)
    );
    // A mount that holds one may replace it, and anyone may clear.
    assert_eq!(reviews.room(EPOCH, &app("i0"), true), Ok(()));
    assert_eq!(reviews.room(EPOCH, &app("other"), false), Ok(()));
    // Cleared, its place is free.
    give(&reviews, "i0", None);
    give(&reviews, "other", Some(context("u9", "x")));
    assert_eq!(held(&reviews).len(), MAX_HELD_CONTEXTS);
}

#[test]
fn c17_an_update_whose_mount_was_released_or_whose_opening_ended_after_its_room_is_not_held() {
    let (reviews, reported) = reporting();
    reviews.room(EPOCH, &app("i1"), true).unwrap();
    reviews.release_app(&app("i1"), &releaser(), || {});
    // Not held, and said so, for the service to record.
    assert_eq!(
        hold(&reviews, EPOCH, "i1", Some(context("u1", "late"))),
        Err(NotHeld)
    );
    assert!(held(&reviews).is_empty());
    // A clear that came too late has nothing to say.
    assert_eq!(hold(&reviews, EPOCH, "i1", None), Ok(()));
    assert_eq!(
        reviews.room(EPOCH, &app("i1"), true),
        Err(ContextRefusal::Gone(ReviewRefusal::Released))
    );
    reviews.room(EPOCH, &app("i2"), true).unwrap();
    reviews.end(EPOCH, &releaser(), || {});
    assert_eq!(
        hold(&reviews, EPOCH, "i2", Some(context("u2", "late"))),
        Err(NotHeld)
    );
    assert!(held(&reviews).is_empty());
    assert_eq!(
        reviews.room(EPOCH, &app("i2"), false),
        Err(ContextRefusal::Gone(ReviewRefusal::Ended))
    );
    // Never held, so never dropped here: its `not_held` is its own call's.
    assert!(reported.take().is_empty());
}

#[tokio::test]
async fn c8_one_context_update_of_a_conversation_runs_at_a_time_across_mounts_and_openings() {
    let reviews = reviews();
    let first = reviews.one_update().await;
    reviews.end(EPOCH, &releaser(), || {});
    let _ = reviews.begin();
    let waiting = tokio::spawn({
        let reviews = reviews.clone();
        async move { drop(reviews.one_update().await) }
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !waiting.is_finished(),
        "another update of the conversation went while one was taken"
    );
    drop(first);
    waiting.await.unwrap();
}

#[test]
fn c9_a_take_empties_the_mounts_frees_their_room_and_leaves_a_release_or_an_end_nothing() {
    let (reviews, reported) = reporting();
    for n in 0..MAX_HELD_CONTEXTS {
        give(
            &reviews,
            &format!("i{n}"),
            Some(context(&format!("u{n}"), &format!("c{n}"))),
        );
    }
    assert_eq!(
        reviews.room(EPOCH, &app("other"), true),
        Err(ContextRefusal::Full)
    );
    // Every one, in the order given, with the update that gave it.
    let taken = reviews.take_held();
    assert_eq!(
        taken
            .contexts
            .iter()
            .map(|context| context.text().unwrap())
            .collect::<Vec<_>>(),
        ["c0", "c1", "c2", "c3"]
    );
    assert_eq!(calls(&taken.updates), ["u0", "u1", "u2", "u3"]);
    // Gone from the mounts at once: their room is free, and a release or an
    // end has nothing of them to drop.
    assert!(held(&reviews).is_empty());
    assert_eq!(reviews.room(EPOCH, &app("other"), true), Ok(()));
    reviews.release_app(&app("i0"), &releaser(), || {});
    // Each taken follows its message once the agent is asked for it.
    let mut first = taken;
    first.asking();
    drop(first);
    // A mount's newer update is held anew; a second take takes only it.
    give(&reviews, "i1", Some(context("u5", "newer")));
    assert_eq!(held(&reviews), ["newer"]);
    let mut second = reviews.take_held();
    assert_eq!(calls(&second.updates), ["u5"]);
    second.asking();
    drop(second);
    let mut none = reviews.take_held();
    assert!(none.contexts.is_empty());
    none.asking();
    drop(none);
    give(&reviews, "i2", Some(context("u6", "held")));
    let mut taken = reviews.take_held();
    taken.asking();
    reviews.end(EPOCH, &releaser(), || {});
    // Nothing taken was dropped by the release or the end.
    assert!(reported.take().is_empty());
    // A message that took some and that the agent refused hands them back:
    // each dropped as not sent, by the system, against its own update.
    taken.refused();
    assert_eq!(
        reported.take(),
        each(&["u6"], ContextDrop::NotSent, &McpAppInitiator::System)
    );
}

#[test]
fn c11_what_a_message_took_is_reported_dropped_as_it_goes_unless_the_agent_was_asked() {
    let (reviews, reported) = reporting();
    give(&reviews, "i1", Some(context("u1", "one")));
    give(&reviews, "i2", Some(context("u2", "two")));
    // Gone before the agent was asked — refused, failed, its task unwound
    // or let go of: each reported dropped unsent, by the system, as it goes.
    let taken = reviews.take_held();
    assert!(reported.take().is_empty());
    drop(taken);
    assert_eq!(
        reported.take(),
        each(
            &["u1", "u2"],
            ContextDrop::NotSent,
            &McpAppInitiator::System
        )
    );
    // Once the agent is asked they follow the message: taken, or unknown,
    // it reports nothing as it goes (row C11b).
    give(&reviews, "i1", Some(context("u3", "three")));
    let mut taken = reviews.take_held();
    taken.asking();
    drop(taken);
    assert!(reported.take().is_empty());
    // Asked, then refused by the agent: handed back, reported once.
    give(&reviews, "i1", Some(context("u4", "four")));
    let mut taken = reviews.take_held();
    taken.asking();
    taken.refused();
    assert_eq!(
        reported.take(),
        each(&["u4"], ContextDrop::NotSent, &McpAppInitiator::System)
    );
    // A task that panics before it asks unwinds through what it took: still
    // reported.
    give(&reviews, "i1", Some(context("u5", "five")));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _taken = reviews.take_held();
        panic!("the submission's task fell over");
    }));
    assert!(unwound.is_err());
    assert_eq!(
        reported.take(),
        each(&["u5"], ContextDrop::NotSent, &McpAppInitiator::System)
    );
}

#[test]
fn c14_c15_contexts_are_dropped_with_their_mount_the_openings_end_a_new_opening_and_a_delete() {
    let (reviews, reported) = reporting();
    give(&reviews, "i1", Some(context("u1", "i1")));
    give(&reviews, "i2", Some(context("u2", "i2")));
    give(&reviews, "i3", Some(context("u5", "i3")));
    // Each drop is reported as it is made, against the update that held
    // what it dropped, by whoever dropped it, once.
    reviews.release_app(&app("i1"), &releaser(), || {});
    assert_eq!(
        reported.take(),
        each(&["u1"], ContextDrop::Released, &releaser())
    );
    reviews.release_app(&app("i1"), &releaser(), || {});
    assert!(reported.take().is_empty());
    assert_eq!(held(&reviews), ["i2", "i3"]);
    let closer = McpAppInitiator::Person {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "pane".into(),
        request_id: "close-1".into(),
    };
    reviews.end(EPOCH, &closer, || {});
    assert_eq!(
        reported.take(),
        each(&["u2", "u5"], ContextDrop::ConversationEnded, &closer)
    );
    assert!(held(&reviews).is_empty());
    reviews.end(EPOCH, &releaser(), || {});
    assert!(reported.take().is_empty());
    // An opening that was never ended still gives the next nothing, and
    // its contexts are dropped by the system.
    let next = reviews.begin();
    assert!(reported.take().is_empty());
    reviews.room(next, &app("i2"), true).unwrap();
    hold(&reviews, next, "i2", Some(context("u3", "again"))).unwrap();
    assert_eq!(held(&reviews), ["again"]);
    let after = reviews.begin();
    assert_eq!(
        reported.take(),
        each(
            &["u3"],
            ContextDrop::ConversationEnded,
            &McpAppInitiator::System
        )
    );
    assert!(held(&reviews).is_empty());
    reviews.room(after, &app("i2"), true).unwrap();
    hold(&reviews, after, "i2", Some(context("u4", "once more"))).unwrap();
    // A delete's, by the deleter it is given.
    let deleter = McpAppInitiator::Person {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "pane".into(),
        request_id: "delete-1".into(),
    };
    reviews.delete(&deleter, || {});
    assert_eq!(
        reported.take(),
        each(&["u4"], ContextDrop::ConversationEnded, &deleter)
    );
    assert!(held(&reviews).is_empty());
    reviews.delete(&deleter, || {});
    assert!(reported.take().is_empty());
}

#[test]
fn m6b_a_messages_turn_is_in_flight_once_until_its_call_ends() {
    let reviews = reviews();
    let first = reviews.start_message("app-turn").unwrap();
    assert!(reviews.start_message("app-turn").is_none());
    // Another turn is its own.
    let other = reviews.start_message("app-other").unwrap();
    drop(first);
    let again = reviews.start_message("app-turn").unwrap();
    // A panic while it was in flight frees it too.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _in_flight = again;
        panic!("the message's call fell over");
    }));
    assert!(panicked.is_err());
    assert!(reviews.start_message("app-turn").is_some());
    drop(other);
}

#[test]
fn a_message_review_says_what_it_asks() {
    let reviews = reviews();
    let waiting = reviews
        .open(
            EPOCH,
            new_review_id(),
            ReviewAsk::SendMessage,
            &app("i1"),
            "charts",
            "show",
            r#"{"text":"hi"}"#,
        )
        .unwrap();
    let shown = reviews.reviews();
    assert_eq!(
        shown[0].title,
        "The show app on charts asks to send a message as you"
    );
    assert_eq!(shown[0].arguments_json, r#"{"text":"hi"}"#);
    assert_eq!(
        shown[0].origin,
        ConversationPermissionOrigin::App {
            server: "charts".into(),
            tool: "show".into(),
        }
    );
    drop(waiting);
}

#[tokio::test]
async fn every_message_is_its_own_review_and_allowing_one_allows_no_other() {
    let reviews = reviews();
    let ask = |text: &str| {
        reviews
            .open(
                EPOCH,
                new_review_id(),
                ReviewAsk::SendMessage,
                &app("i1"),
                "charts",
                "show",
                text,
            )
            .unwrap()
    };
    let first = ask("one");
    let second = ask("two");
    let answered = reviews.reviews()[0].clone();
    assert_eq!(
        reviews.answer(
            &answered.execution_id,
            &answered.permission_id,
            ALLOW,
            person()
        ),
        ReviewAnswer::Ended
    );
    assert_eq!(
        first.ended(Duration::from_secs(1)).await,
        ReviewEnd::Allowed(person())
    );
    // The same mount's other message still waits on its own review.
    let open = reviews.reviews();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].permission_id, second.permission_id);
    drop(second);
    assert!(reviews.reviews().is_empty());
}
