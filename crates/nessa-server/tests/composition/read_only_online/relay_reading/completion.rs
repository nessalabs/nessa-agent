//! Positive relay controls resume the same retained cache, not a new cold read.
//!
//! The fixture seeds two conversations and 301 facts. Six passes allow one
//! catalogue pass, two record passes per conversation, then a settled-walk
//! confirmation. This is a finite fixture allowance, not a network guarantee.
//! Each call keeps its five-second ceiling; all calls and relay joins share a
//! thirty-second wall ceiling. No public failure is retried or turned into
//! progress. `tests` covers partial, stalled, refused and late completion.
use nessa_client_core::retained::{ReadFailure, ReadReport};
use std::time::{Duration, Instant};

const PASSES: usize = 1 + 2 * 2 + 1;
const PER_READ: Duration = Duration::from_secs(5);
const TOTAL: Duration = Duration::from_secs(PASSES as u64 * PER_READ.as_secs());

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Failure {
    Read(ReadFailure),
    Incomplete {
        passes: usize,
        elapsed: Duration,
        last: Option<ReadReport>,
    },
}

pub(super) fn finish(
    read: impl FnMut(Duration) -> Result<ReadReport, ReadFailure>,
) -> Result<ReadReport, Failure> {
    let started = Instant::now();
    finish_with(read, || started.elapsed())
}

fn finish_with(
    mut read: impl FnMut(Duration) -> Result<ReadReport, ReadFailure>,
    elapsed: impl Fn() -> Duration,
) -> Result<ReadReport, Failure> {
    let mut last = None;
    let mut passes = 0;
    while passes < PASSES {
        let before = elapsed();
        let Some(left) = TOTAL.checked_sub(before).filter(|left| !left.is_zero()) else {
            break;
        };
        let report = read(PER_READ.min(left)).map_err(Failure::Read)?;
        passes += 1;
        let after = elapsed();
        eprintln!("relay fixture pass {passes}/{PASSES} after {after:?}: {report:?}");
        let complete = report.complete;
        last = Some(report);
        if after >= TOTAL {
            break;
        }
        if complete {
            return Ok(last.expect("this pass supplied its report"));
        }
    }
    Err(Failure::Incomplete {
        passes,
        elapsed: elapsed(),
        last,
    })
}

#[path = "completion/tests.rs"]
mod tests;
