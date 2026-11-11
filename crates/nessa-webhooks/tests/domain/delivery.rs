use super::*;
use uuid::Uuid;

fn queued() -> Delivery {
    Delivery::queued(Uuid::from_u128(1), 7)
}

#[test]
fn admission_pins_epoch_and_timestamp() {
    let queued = queued();
    assert_eq!(queued.admit(8, 100), Err(DeliveryError::WrongEpoch));
    assert_eq!(queued.admit(6, 100), Err(DeliveryError::WrongEpoch));
    let sending = queued.admit(7, 100).unwrap();
    assert_eq!(sending.phase(), Phase::Sending);
    assert_eq!(sending.attempts(), 1);
    assert_eq!(sending.timestamp_ms(), Some(100));
    assert_eq!(
        queued.timestamp_ms(),
        None,
        "a transition does not mutate a stored value"
    );
    assert_eq!(sending.admit(7, 100), Err(DeliveryError::NotReady));
}

#[test]
fn retries_are_bounded_and_keep_identity() {
    let mut delivery = queued();
    let mut now = 100;
    for attempt in 1..=MAX_ATTEMPTS {
        let sending = delivery.admit(7, now).unwrap();
        assert_eq!(sending.id(), queued().id());
        assert_eq!(sending.epoch(), 7);
        assert_eq!(sending.attempts(), attempt);
        assert_eq!(sending.timestamp_ms(), Some(100));
        delivery = sending.observed(Observation::Retryable, now).unwrap();
        if attempt < MAX_ATTEMPTS {
            let due = now + (1000 << (attempt - 1));
            assert_eq!(delivery.phase(), Phase::Retry { due_at_ms: due });
            assert_eq!(delivery.admit(7, due - 1), Err(DeliveryError::NotReady));
            assert_eq!(delivery.admit(8, due), Err(DeliveryError::WrongEpoch));
            now = due;
        } else {
            assert_eq!(delivery.phase(), Phase::Terminal(Outcome::NotSent));
            assert_eq!(delivery.admit(7, u64::MAX), Err(DeliveryError::NotReady));
        }
    }
}

#[test]
fn observations_are_terminal_except_bounded_retry() {
    for (observation, outcome) in [
        (Observation::Success, Outcome::Sent),
        (Observation::Rejected, Outcome::NotSent),
        (Observation::TimedOut, Outcome::TimedOut),
        (Observation::Unknown, Outcome::Unknown),
    ] {
        assert_eq!(
            queued().observed(observation, 100),
            Err(DeliveryError::InvalidTransition)
        );
        let final_value = queued()
            .admit(7, 100)
            .unwrap()
            .observed(observation, 100)
            .unwrap();
        assert_eq!(final_value.phase(), Phase::Terminal(outcome));
        assert_eq!(final_value.admit(7, 100), Err(DeliveryError::NotReady));
        assert_eq!(
            final_value.observed(observation, 100),
            Err(DeliveryError::InvalidTransition)
        );
        assert_eq!(final_value.recovered(), final_value);
    }
    let sending = queued().admit(7, 1).unwrap();
    assert_eq!(
        sending.observed(Observation::Retryable, u64::MAX),
        Err(DeliveryError::TimeOverflow)
    );
}

#[test]
fn restart_does_not_repost_an_unknown_effect() {
    let queued = queued();
    assert_eq!(queued.recovered(), queued);
    let sending = queued.admit(7, 100).unwrap();
    let unknown = sending.recovered();
    assert_eq!(unknown.phase(), Phase::Terminal(Outcome::Unknown));
    assert_eq!(unknown.id(), sending.id());
    assert_eq!(unknown.timestamp_ms(), Some(100));
    assert_eq!(unknown.admit(7, 100), Err(DeliveryError::NotReady));
    let retry = sending.observed(Observation::Retryable, 100).unwrap();
    assert_eq!(retry.recovered(), retry);
    assert!(retry.recovered().admit(7, 1100).is_ok());
}

#[test]
fn refusal_cannot_cancel_an_inflight_observation() {
    assert_eq!(
        queued().refused().unwrap().phase(),
        Phase::Terminal(Outcome::NotSent)
    );
    let sending = queued().admit(7, 100).unwrap();
    assert_eq!(sending.refused(), Err(DeliveryError::InvalidTransition));
    let retry = sending.observed(Observation::Retryable, 100).unwrap();
    let refused = retry.refused().unwrap();
    assert_eq!(refused.id(), sending.id());
    assert_eq!(refused.phase(), Phase::Terminal(Outcome::NotSent));
    assert_eq!(refused.refused(), Err(DeliveryError::InvalidTransition));
}
