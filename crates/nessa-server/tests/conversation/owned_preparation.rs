//! Absence consumes the exact opening owner's completed acquisition fact.
use super::*;
use crate::conversation::application::service::{OpeningFailure, Slot};

#[tokio::test]
async fn an_unacquired_preparation_requires_a_completed_opening_without_holds() {
    let cause = LifetimeCause::HostClose;
    let actor = Initiator::Host(HostActor::new("person", "panel", "close").unwrap());
    let opening = Arc::new(Slot::new(0));
    let witness = UnacquiredPreparation {
        opening: opening.clone(),
    };
    let pending = witness.close(&cause, &actor).await;
    assert_eq!(pending.physical, PhysicalFact::Failed);
    assert_eq!(pending.evidence, EvidenceFact::Pending);
    opening
        .value
        .set(Err(OpeningFailure {
            cause: ConversationError::Unavailable,
            holds: false,
        }))
        .unwrap_or_else(|_| panic!("vacant opening"));
    let absent = witness.close(&cause, &actor).await;
    assert_eq!(absent.physical, PhysicalFact::Released);
    assert_eq!(absent.evidence, EvidenceFact::Acknowledged);
    let held = Arc::new(Slot::new(0));
    held.value
        .set(Err(OpeningFailure {
            cause: ConversationError::Unavailable,
            holds: true,
        }))
        .unwrap_or_else(|_| panic!("vacant opening"));
    let uncertain = UnacquiredPreparation { opening: held }
        .close(&cause, &actor)
        .await;
    assert_eq!(uncertain.physical, PhysicalFact::Failed);
    assert_eq!(uncertain.evidence, EvidenceFact::Pending);
}
