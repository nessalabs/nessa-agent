//! Provider context must agree with every retained provider-side fact.

use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, ProviderContextEvidenceError,
};

const EVIDENCE_FACT_COUNT: usize = 5;

fn validate(
    context: &ProviderContext,
    evidence: [bool; EVIDENCE_FACT_COUNT],
) -> Result<(), ProviderContextEvidenceError> {
    context.validate_evidence(
        evidence[0],
        evidence[1],
        evidence[2],
        evidence[3],
        evidence[4],
    )
}

fn evidence_from_mask(mask: u8) -> [bool; EVIDENCE_FACT_COUNT] {
    [
        mask & 0b00001 != 0,
        mask & 0b00010 != 0,
        mask & 0b00100 != 0,
        mask & 0b01000 != 0,
        mask & 0b10000 != 0,
    ]
}

#[test]
fn absent_context_accepts_a_checkpoint_without_provider_evidence() {
    assert_eq!(
        validate(&ProviderContext::Absent, [false; EVIDENCE_FACT_COUNT]),
        Ok(())
    );
}

#[test]
fn absent_context_rejects_each_provider_fact_independently() {
    for (fact, evidence) in [
        ("provider observations", [true, false, false, false, false]),
        ("provider report", [false, true, false, false, false]),
        ("dispatched stage", [false, false, true, false, false]),
        ("native correlation", [false, false, false, true, false]),
        ("selected queue entry", [false, false, false, false, true]),
    ] {
        assert_eq!(
            validate(&ProviderContext::Absent, evidence),
            Err(ProviderContextEvidenceError),
            "absent context accepted {fact}"
        );
    }
}

#[test]
fn absent_context_rejects_every_combination_of_provider_facts() {
    for mask in 1_u8..(1 << EVIDENCE_FACT_COUNT) {
        if mask.count_ones() < 2 {
            continue;
        }
        assert_eq!(
            validate(&ProviderContext::Absent, evidence_from_mask(mask)),
            Err(ProviderContextEvidenceError),
            "absent context accepted evidence mask {mask:05b}"
        );
    }
}

#[test]
fn recorded_context_accepts_a_checkpoint_without_provider_evidence() {
    let context =
        ProviderContext::recorded_after(ExecutionSessionId::new("provider-session").unwrap(), 0);

    assert_eq!(validate(&context, [false; EVIDENCE_FACT_COUNT]), Ok(()));
}

#[test]
fn recorded_context_accepts_every_combination_of_provider_facts() {
    let context =
        ProviderContext::recorded_after(ExecutionSessionId::new("provider-session").unwrap(), 0);

    for mask in 1_u8..(1 << EVIDENCE_FACT_COUNT) {
        assert_eq!(
            validate(&context, evidence_from_mask(mask)),
            Ok(()),
            "recorded context rejected evidence mask {mask:05b}"
        );
    }
}

#[test]
fn admission_watermark_distinguishes_old_history_from_new_context_work() {
    let context =
        ProviderContext::recorded_after(ExecutionSessionId::new("provider-session").unwrap(), 3);
    assert_eq!(context.admission_watermark(), Some(3));
    assert_eq!(context.validate_admission(3, 3), Ok(()));
    assert!(!context.has_admitted_work(3));
    assert!(context.has_admitted_work(4));
    assert!(context.has_admitted_work(2), "invalid counts fail closed");
    assert_eq!(ProviderContext::Absent.admission_watermark(), None);
}

#[test]
fn admission_boundary_cannot_skip_pending_work_or_exceed_history() {
    use nessa_sdk::domain::agent_execution::sessions::ProviderContextAdmissionError;
    let context =
        ProviderContext::recorded_after(ExecutionSessionId::new("provider-session").unwrap(), 3);
    for (count, prefix) in [(2, 2), (4, 2), (4, 5)] {
        assert_eq!(
            context.validate_admission(count, prefix),
            Err(ProviderContextAdmissionError::InvalidWatermark),
        );
    }
    assert_eq!(context.validate_admission(4, 3), Ok(()));
}

#[test]
fn repeated_native_context_preserves_its_admission_boundary() {
    use nessa_sdk::domain::agent_execution::sessions::ProviderContextAdmissionError;
    let native = ExecutionSessionId::new("provider-session").unwrap();
    let previous = ProviderContext::recorded_after(native.clone(), 1);
    let reset = ProviderContext::recorded_after(native, 3);
    assert_eq!(previous.validate_transition(&previous, 3, 3, false), Ok(()));
    assert_eq!(
        reset.validate_transition(&previous, 3, 3, false),
        Err(ProviderContextAdmissionError::InvalidTransition),
    );
    assert_eq!(reset.validate_transition(&previous, 3, 3, true), Ok(()));
}

#[test]
fn replacement_uses_current_prefix_or_conservative_zero() {
    use nessa_sdk::domain::agent_execution::sessions::ProviderContextAdmissionError;
    let previous =
        ProviderContext::recorded_after(ExecutionSessionId::new("old-native").unwrap(), 0);
    for watermark in [0, 3] {
        let next = ProviderContext::recorded_after(
            ExecutionSessionId::new("new-native").unwrap(),
            watermark,
        );
        assert_eq!(next.validate_transition(&previous, 3, 3, false), Ok(()));
    }
    let arbitrary =
        ProviderContext::recorded_after(ExecutionSessionId::new("new-native").unwrap(), 1);
    assert_eq!(
        arbitrary.validate_transition(&previous, 3, 3, false),
        Err(ProviderContextAdmissionError::InvalidTransition),
    );
}

#[test]
fn missing_observations_do_not_authorize_clearing_an_admitted_context() {
    use nessa_sdk::domain::agent_execution::sessions::ProviderContextAdmissionError;
    let previous =
        ProviderContext::recorded_after(ExecutionSessionId::new("provider-session").unwrap(), 0);
    assert_eq!(
        ProviderContext::Absent.validate_transition(&previous, 1, 0, false),
        Err(ProviderContextAdmissionError::InvalidTransition),
    );
    assert_eq!(
        ProviderContext::Absent.validate_transition(&previous, 0, 0, false),
        Ok(()),
    );
}
