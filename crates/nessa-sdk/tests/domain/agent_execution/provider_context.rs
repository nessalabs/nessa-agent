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
    let context = ProviderContext::Recorded(ExecutionSessionId::new("provider-session").unwrap());

    assert_eq!(validate(&context, [false; EVIDENCE_FACT_COUNT]), Ok(()));
}

#[test]
fn recorded_context_accepts_every_combination_of_provider_facts() {
    let context = ProviderContext::Recorded(ExecutionSessionId::new("provider-session").unwrap());

    for mask in 1_u8..(1 << EVIDENCE_FACT_COUNT) {
        assert_eq!(
            validate(&context, evidence_from_mask(mask)),
            Ok(()),
            "recorded context rejected evidence mask {mask:05b}"
        );
    }
}
