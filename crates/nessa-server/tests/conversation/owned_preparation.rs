//! Absence consumes the exact opening owner's completed acquisition fact.
use super::*;
use crate::conversation::application::service::OpeningResources;

#[tokio::test]
async fn an_unacquired_preparation_requires_a_completed_opening_without_holds() {
    let cause = LifetimeCause::HostClose;
    let actor = Initiator::Host(HostActor::new("person", "panel", "close").unwrap());
    let opening = Arc::new(OpeningResources::default());
    let witness = UnacquiredPreparation {
        opening: opening.clone(),
        deadline: Duration::from_secs(10),
    };
    let pending = witness.close(&cause, &actor).await;
    assert_eq!(pending.physical, PhysicalFact::Failed);
    assert_eq!(pending.evidence, EvidenceFact::Pending);
    opening.complete(false, Duration::from_secs(10)).await;
    let absent = witness.close(&cause, &actor).await;
    assert_eq!(absent.physical, PhysicalFact::Released);
    assert_eq!(absent.evidence, EvidenceFact::Acknowledged);
    let held = Arc::new(OpeningResources::default());
    held.complete(true, Duration::from_secs(10)).await;
    let uncertain = UnacquiredPreparation {
        opening: held,
        deadline: Duration::from_secs(10),
    }
    .close(&cause, &actor)
    .await;
    assert_eq!(uncertain.physical, PhysicalFact::Failed);
    assert_eq!(uncertain.evidence, EvidenceFact::Pending);
}
