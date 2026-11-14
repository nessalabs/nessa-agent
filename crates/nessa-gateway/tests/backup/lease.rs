//! The export hold is the conversation service's admission lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nessa_auth::domain::{OrganizationId, PrincipalId};

use crate::conversation::application::{ConversationCaller, ExportHoldFailure};
use crate::conversation_test_support::image_fixture;

#[tokio::test]
async fn a_mutation_during_the_hold_waits_until_release() {
    let (service, _, _, _) = image_fixture(false, None);
    let hold = service.hold_for_export().await.unwrap();
    let entered = std::sync::Arc::new(AtomicBool::new(false));
    let flag = entered.clone();
    let listing = service.clone();
    let waiting = tokio::spawn(async move {
        let _ = listing
            .list(
                ConversationCaller {
                    organization_id: OrganizationId::new("org").unwrap(),
                    principal_id: PrincipalId::new("alice").unwrap(),
                    surface_id: "desktop".into(),
                    action_id: "list".into(),
                },
                false,
            )
            .await;
        flag.store(true, Ordering::SeqCst);
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !entered.load(Ordering::SeqCst),
        "a command waits while the export holds admission"
    );
    drop(hold);
    waiting.await.unwrap();
    assert!(entered.load(Ordering::SeqCst));
}

#[tokio::test]
async fn retirement_before_the_hold_aborts() {
    let (service, _, _, _) = image_fixture(false, None);
    service
        .retire("export_test", &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert!(matches!(
        service.hold_for_export().await,
        Err(ExportHoldFailure::Retired)
    ));
}

#[tokio::test]
async fn retirement_during_the_hold_aborts_and_releases() {
    let (service, _, _, _) = image_fixture(false, None);
    let hold = service.hold_for_export().await.unwrap();
    let retiring = service.clone();
    let retirement = tokio::spawn(async move {
        retiring
            .retire("export_test", &uuid::Uuid::new_v4().to_string())
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !hold.retirement_signaled() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "retirement did not signal while the hold was kept"
        );
        tokio::task::yield_now().await;
    }
    assert!(!retirement.is_finished());
    drop(hold);
    retirement.await.unwrap().unwrap();
    assert!(matches!(
        service.hold_for_export().await,
        Err(ExportHoldFailure::Retired)
    ));
}
