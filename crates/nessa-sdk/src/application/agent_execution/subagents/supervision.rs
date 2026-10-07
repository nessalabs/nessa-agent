//! Contains injected construction, poll, installation and destruction independently.
//! Ready output enters its existing authoritative owner before future destruction.
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    task::Poll,
};

pub(super) struct Observed<T> {
    pub(super) output: Option<T>,
    pub(super) faulted: bool,
}

pub(super) fn synchronous<T>(operation: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(value) => Some(value),
        Err(payload) => {
            std::mem::forget(payload);
            None
        }
    }
}

pub(super) async fn effect<F: Future, T>(
    make: impl FnOnce() -> F,
    install: impl FnOnce(F::Output) -> T,
) -> Observed<T> {
    let Some(future) = synchronous(make) else {
        return Observed {
            output: None,
            faulted: true,
        };
    };
    let mut future = Box::pin(future);
    let ready = poll_fn(
        |context| match synchronous(|| future.as_mut().poll(context)) {
            Some(poll) => poll.map(Some),
            None => Poll::Ready(None),
        },
    )
    .await;
    let output = ready.and_then(|output| synchronous(|| install(output)));
    let dropped = synchronous(|| drop(future)).is_some();
    Observed {
        faulted: output.is_none() || !dropped,
        output,
    }
}
