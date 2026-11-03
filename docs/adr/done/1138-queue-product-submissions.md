# 1138. A second product submission waits its turn

## Purpose

The phone sends from several tasks at once. The product worker used to refuse
the second send while the first was still being handed to the native session,
and the person saw a Nessa connection alert. This decision makes that hand-off
wait in order. The reply waits stay side by side, as they already were.

- **Date:** 2026-11-02
- **Status:** accepted

## Context

`NativeProductWorker` is hand-written Swift in
`crates/nessa-device-client/swift/`. UniFFI generates the session types it
calls. `scripts/build-native-device.sh` copies this Swift into the package it
builds. nessa-ios vendors that copy as `Vendor/NessaNative`. The behavior lives
in the hand-written file. Editing a vendored copy would be overwritten on the
next build.

`request` used to admit one submission. A second call while that admission was
held failed at once with `NativeWorkerError.busy` and sent nothing. The native
hand-off (`session.request`) is the short part. The ticket wait, which is the
reply, already runs on a separate concurrent queue so the next hand-off can
start. The busy refusal treated the whole call as the thing that must be
unique.

nessa-ios#10 retries a busy refusal after 2 ms in
`NativeGatewayProductExchange`. That retry races the hand-off. A queue in that
adapter cannot see the hand-off end, because `request` returns only once the
reply arrives. The adapter would hold its slot for the whole round trip and
undo the parallel reply waits.

Nessa is alpha. There is no compatibility shim for callers that caught `busy`
from `request`.

## Decision

The submit lane has one hand-off at a time. Further submissions wait in
arrival order and start when the hand-off in front of them returns, whether
that return is a ticket or an error. The lane releases before the ticket wait,
so replies still wait side by side.

A waiter that is cancelled leaves the queue at once and fails with
`CancellationError`. The hand-off in front, and the waiters behind, carry on.
A hand-off whose body has started is not pulled back: the native call settles,
and its reply is delivered, which is the rule the worker already had.

Shutdown fails every waiter still in the queue with `NativeWorkerError.closed`
before the drain waits. A hand-off that has already started joins the drain.
The worker calls `requestClose` before `close`, as it does today. A later
submission fails with `closed` and does not run.

`NativeWorkerError.busy` stays. The session worker is try-once on purpose:
`verification/native-device/check-worker.swift` fails if a second session call
is queued. Product Ready and product events are still one waiter each and
refuse a second the same way. No caller of those paths asked to wait. Nothing
on the submit lane throws `busy`.

## States

One lane. `handingOff` means a body has been started and has not returned.
The queue is FIFO. `closed` is set by shutdown and never clears.

| Configuration | `handingOff` | Queue | What a new submission does |
| --- | --- | --- | --- |
| Idle | no | empty | Starts the hand-off. |
| Handing off | yes | empty | Waits at the tail. This is the queued configuration. |
| Queued | yes | one or more, arrival order | Waits at the tail. |
| Closed | no, once the current body has returned | empty; waiters already failed | Fails with `NativeWorkerError.closed`. The body does not run. |

| Event | Idle | Handing off or queued | Closed |
| --- | --- | --- | --- |
| Submission arrives | Start it. Handing off. | Append. Queued. | `closed`. Body does not run. |
| Body returns, queue empty | — | Idle. | Stay closed. |
| Body returns, queue not empty | — | Start the head. The rest stay in order. | Fail anything still queued. Stay closed. |
| Cancel a queued waiter | — | Remove that waiter. `CancellationError`. The rest keep their order. Do not wait for the current body. | — |
| Cancel the running hand-off before its body starts | — | `CancellationError`. Start the next waiter, or go idle. | — |
| Cancel the running hand-off after its body starts | — | The body finishes with its own result. The next waiter starts when it returns. | The body finishes with its own result. A cancel after shutdown does not replace it with `closed`, and the reply wait still runs. |
| Shutdown | Closed. | Every queued waiter gets `closed` now. The running body finishes and does not start another. | Closed. |

A waiter already taken off the queue for its hand-off is the running hand-off,
not a queued waiter. Shutdown and cancel both take the lane lock, so each
waiter is one or the other.

## Invariants

| Guarantee | Enforcer |
| --- | --- |
| Bodies that run do so in arrival order. | `verification/native-device/check-submit-lane.swift` admits eight waiters behind a held body and checks the execution log. |
| One body runs at a time. | The same run records depth on a concurrent queue. A serial queue cannot hide a second start. |
| A submission is not lost. Each call returns its value or throws. A body that throws still lets the next waiter start. | The order run expects each id once. The failure run throws from one body and expects the next id. |
| A submission is not run twice. A cancelled or closed waiter does not run. | Those runs record body entry. A second resume of one continuation traps the process. |
| Cancel while queued returns without waiting for the current body, and the others still run in order. | The cancel run awaits the cancelled task before releasing the held body, then checks the survivors. |
| Shutdown does not hang. Queued waiters get `NativeWorkerError.closed` while a body is still held. That body then returns its own value. A later call gets `closed` and does not run. | The shutdown run, with a five-second deadline on each wait. |
| A hand-off whose body has started keeps that result if shutdown or cancel arrives during the body. The ticket is not dropped, and the worker's drain waits for the reply. | `check-submit-lane.swift` closes, then cancels, a held body and expects its value. `check-product-request.swift` holds the native call, closes, cancels, and checks the drain is still waiting until the ticket wait leaves. |
| A capture failure leaves the drain group once. One outstanding request does not crash. With others still outstanding, the drain does not finish early. | `check-product-request.swift` throws from capture for one request and for three. |
| The submit lane does not throw `busy`. | The lane has no such path. `check-worker.swift` still requires `busy` for a second session call. |

## Alternatives considered

- **Queue in the nessa-ios adapter.** `request` returns only when the reply
  arrives, so the adapter cannot tell the end of the hand-off from the end of
  the round trip. Its queue would hold the slot for the reply and serialize
  waits the worker already runs side by side. The adapter also cannot see
  shutdown of the native child except as a late error.
- **Retry `busy`.** nessa-ios#10 waits 2 ms and tries again. The refusal is not
  a native failure; the second call never entered. Under several sends the
  retry loses the race and the phone shows the connection alert. A longer
  backoff makes the alert less common and the send slower, and it still drops
  a call when the hand-off outlasts the retries.
- **One product session per send.** The child is one native owner and it
  admits one hand-off at a time. Opening more children would invent a second
  owner for the same connection. Parallelism belongs to the ticket waits.

## Consequences

A burst of sends waits a few hand-offs instead of failing. The lane holds
those waiters in memory until they start, get cancelled, or the worker shuts
down. Reply waits are unchanged.

`request` no longer throws `busy`. Callers that treated `busy` as "try again"
are wrong; the call now waits. Ready, events, and session methods still throw
`busy`, and that is still try-once.

This repository does not contain nessa-ios `Vendor/NessaNative`. The package
build needs Apple toolchains, so this change does not regenerate that vendor
tree. nessa-ios has to copy the new Swift and delete the 2 ms retry.

## Review brief

Review the branch `cursor/queue-product-submissions-2a91` against `main`.
The tree under review is the committed head. Do not restack it during the
check. Intended behavior is the state table above.

Scope: `crates/nessa-device-client/swift/ProductSubmitLane.swift`,
`NativeWorkerError.swift`, `NativeProductWorker.request` and its close path,
`verification/native-device/check-submit-lane.swift`, and this record.
Out of scope: Ready and event admission, the session worker's try-once rule,
UniFFI session types, and the nessa-ios vendor tree.

Try to break the lane. Hold one body. Queue three. Cancel the middle one
before releasing the holder, and confirm it has already returned, its body
did not run, and the other two still run in order. Close with waiters held
behind a body and confirm they return `closed` before the body is released,
then that the body returns its own value and a new call does not run. Overlap
two bodies on a concurrent queue. A second resume, a waiter left in the queue
after cancel or close, or a `busy` error from the lane, is a miss even if a
later test is green.
