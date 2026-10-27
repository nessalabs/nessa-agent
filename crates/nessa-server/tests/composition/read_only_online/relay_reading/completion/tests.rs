//! Deterministic controls of the fixture driver, never the product's clock.
use super::*;
use std::cell::Cell;

fn report(complete: bool) -> ReadReport {
    ReadReport {
        moved: true,
        complete,
        conversations: 2,
    }
}

#[test]
fn partial_reads_continue_until_the_final_complete_report() {
    let calls = Cell::new(0);
    let result = finish_with(
        |budget| {
            assert_eq!(budget, PER_READ);
            calls.set(calls.get() + 1);
            Ok(report(calls.get() == 3))
        },
        || Duration::ZERO,
    );
    assert_eq!(result, Ok(report(true)));
    assert_eq!(calls.get(), 3);
}

#[test]
fn stalled_continuation_fails_at_the_pass_limit() {
    let calls = Cell::new(0);
    let result = finish_with(
        |_| {
            calls.set(calls.get() + 1);
            Ok(report(false))
        },
        || Duration::ZERO,
    );
    assert_eq!(calls.get(), PASSES);
    assert!(matches!(
        result,
        Err(Failure::Incomplete {
            passes: PASSES,
            last: Some(ReadReport {
                complete: false,
                ..
            }),
            ..
        })
    ));
}

#[test]
fn the_total_deadline_is_not_reset_by_partial_progress() {
    let now = Cell::new(Duration::ZERO);
    let calls = Cell::new(0);
    let result = finish_with(
        |_| {
            calls.set(calls.get() + 1);
            now.set(TOTAL);
            Ok(report(false))
        },
        || now.get(),
    );
    assert_eq!(calls.get(), 1);
    assert!(matches!(result, Err(Failure::Incomplete { passes: 1, .. })));
}

#[test]
fn a_complete_report_arriving_after_the_total_deadline_fails() {
    let now = Cell::new(Duration::ZERO);
    let result = finish_with(
        |_| {
            now.set(TOTAL + Duration::from_millis(1));
            Ok(report(true))
        },
        || now.get(),
    );
    assert!(matches!(
        result,
        Err(Failure::Incomplete {
            last: Some(ReadReport { complete: true, .. }),
            ..
        })
    ));
}

#[test]
fn the_last_call_receives_only_the_remaining_fixture_budget() {
    let now = Cell::new(TOTAL - Duration::from_millis(200));
    assert_eq!(
        finish_with(
            |budget| {
                assert_eq!(budget, Duration::from_millis(200));
                now.set(now.get() + Duration::from_millis(100));
                Ok(report(true))
            },
            || now.get()
        ),
        Ok(report(true))
    );
}

#[test]
fn typed_read_failures_end_continuation_without_retry() {
    for failure in [
        ReadFailure::Refused,
        ReadFailure::Unreachable,
        ReadFailure::Protocol,
        ReadFailure::Cache,
    ] {
        let calls = Cell::new(0);
        assert_eq!(
            finish_with(
                |_| {
                    calls.set(calls.get() + 1);
                    Err(failure)
                },
                || Duration::ZERO
            ),
            Err(Failure::Read(failure))
        );
        assert_eq!(calls.get(), 1);
    }
}
