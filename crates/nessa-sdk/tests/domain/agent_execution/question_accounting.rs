//! Open accounting is derived from legal asks, not independent scalar claims.
use super::*;
use crate::domain::agent_execution::questions::{
    AnswerOption, AnswerShape, Question, MAX_TEXT_BYTES,
};

fn minimal_ask() -> AgentQuestion {
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

/// Fill legal option labels to reach a carrying cost, using the owner to
/// measure the baseline and the effect of extending one label.
fn ask_at_cost(target: usize) -> AgentQuestion {
    let build = |lengths: &[usize]| {
        AgentQuestion::new(
            "m",
            vec![Question::new(
                "k",
                "p",
                None,
                AnswerShape::One,
                lengths
                    .iter()
                    .enumerate()
                    .map(|(i, length)| {
                        AnswerOption::new(format!("v{i}"), "l".repeat(*length), None).unwrap()
                    })
                    .collect(),
                None,
                false,
            )
            .unwrap()],
        )
        .unwrap()
    };
    let mut lengths = vec![1; 20];
    let baseline = build(&lengths).carrying_cost();
    lengths[0] += 1;
    let step = build(&lengths).carrying_cost() - baseline;
    lengths[0] -= 1;
    assert_eq!((target - baseline) % step, 0);
    let mut remaining = (target - baseline) / step;
    for length in &mut lengths {
        let extra = remaining.min(MAX_TEXT_BYTES - 1);
        *length += extra;
        remaining -= extra;
    }
    assert_eq!(remaining, 0);
    let ask = build(&lengths);
    assert_eq!(ask.carrying_cost(), target);
    ask
}

#[test]
fn empty_and_minimal_open_collections_keep_their_actual_totals() {
    let ask = minimal_ask();
    let empty = OpenQuestionAccounting::new([]).unwrap();
    assert_eq!(empty.count(), 0);
    assert_eq!(empty.cost(), 0);
    assert_eq!(empty.reason_for(&ask), None);
    let one = OpenQuestionAccounting::new([&ask]).unwrap();
    assert_eq!(one.count(), 1);
    assert_eq!(one.cost(), ask.carrying_cost());
    assert_eq!(one.reason_for(&ask), None);
    assert_eq!(one.clone(), one);
    let full = vec![ask.clone(); MAX_OPEN_QUESTIONS];
    let accounting = OpenQuestionAccounting::new(&full).unwrap();
    assert_eq!(accounting.count(), MAX_OPEN_QUESTIONS);
    assert_eq!(accounting.cost(), MAX_OPEN_QUESTIONS * ask.carrying_cost());
    assert_eq!(
        accounting.reason_for(&ask),
        Some(QuestionRefusalReason::TooManyOpen)
    );
    // Both would exceed their ceilings; count is the reported reason.
    assert_eq!(
        accounting.reason_for(&ask_at_cost(MAX_OPEN_ASK_COST)),
        Some(QuestionRefusalReason::TooManyOpen)
    );
}

#[test]
fn accounting_refuses_actual_collections_that_exceed_either_admitted_limit() {
    let ask = minimal_ask();
    let too_many = vec![ask.clone(); MAX_OPEN_QUESTIONS + 1];
    assert_eq!(
        OpenQuestionAccounting::new(&too_many),
        Err(ExecutionError::InvalidQuestionRefusal)
    );
    let too_costly = ask_at_cost(MAX_OPEN_ASK_COST + 2);
    assert_eq!(
        OpenQuestionAccounting::new([&too_costly]),
        Err(ExecutionError::InvalidQuestionRefusal)
    );
    // It is the collection's cost, not only the size of an individual ask.
    let full = ask_at_cost(MAX_OPEN_ASK_COST);
    assert_eq!(
        OpenQuestionAccounting::new([&full, &ask]),
        Err(ExecutionError::InvalidQuestionRefusal)
    );
}

#[test]
fn exact_cost_boundaries_admit_the_total_and_refuse_only_the_next_excess() {
    let candidate = minimal_ask();
    let full = ask_at_cost(MAX_OPEN_ASK_COST);
    let accounting = OpenQuestionAccounting::new([&full]).unwrap();
    assert_eq!(accounting.cost(), MAX_OPEN_ASK_COST);
    assert_eq!(
        accounting.reason_for(&candidate),
        Some(QuestionRefusalReason::TooLarge)
    );
    let just_fits = ask_at_cost(MAX_OPEN_ASK_COST - candidate.carrying_cost());
    assert_eq!(
        OpenQuestionAccounting::new([&just_fits])
            .unwrap()
            .reason_for(&candidate),
        None
    );
    let exceeds = ask_at_cost(MAX_OPEN_ASK_COST - candidate.carrying_cost() + 2);
    assert_eq!(
        OpenQuestionAccounting::new([&exceeds])
            .unwrap()
            .reason_for(&candidate),
        Some(QuestionRefusalReason::TooLarge)
    );
}
