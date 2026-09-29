# Streaming message commit cadence

Issue [#294](https://github.com/nessalabs/nessa-agent/issues/294) owns the
initial cadence. `SessionManager::Evidence` remains the only owner of observed,
committed, and pending message facts. Its existing `save_observed` is the only
writer. A timer is a wake-up, not a second record queue or a transcript source.

A pending generation begins with the first unsaved `ExecutionUpdate::Message`.
The deadline is that observation's monotonic time plus 100 ms and never moves
while the generation grows. A generation saves sooner at 16 KiB of message
payload or 64 message observations. A consequential event saves immediately.
The existing 8 MiB/1,024-message safety limits and frame bounds still apply.
On acknowledged save, the pending generation clears with its facts. On definite
or uncertain failure, both remain available for the existing typed storage
failure path and exact retry.

The application owns a monotonic timer port. SDK infrastructure adapts Tokio's
clock; tests inject a manual implementation. The active Agent invocation
supervisor waits for the next deadline alongside provider events and settlement.
A wake reacquires the evidence lock and checks the generation and deadline before
calling the same save owner. The manager never reads a clock or sleeps through a
concrete runtime directly.

| Ordering | Required result | Enforcer |
| --- | --- | --- |
| One small message, then silence | Save begins at the fixed 100 ms deadline without another provider event. | Supervisor timer branch and manager generation check. |
| Continuous tiny messages | The first deadline stays fixed; size/count may save sooner. | Pending generation state in `Evidence`. |
| Consequential event before deadline | Its save includes pending messages; the old timer wake changes nothing. | Evidence lock and generation comparison. |
| Timer and threshold become ready together | One generation receives one in-flight append, with no empty save. | `save_observed` under the evidence lock. |
| Append is slow | One save remains in flight; bounded provider ingress and cancellation work retain capacity. | Supervisor polling and existing output limits. |
| Another save holds evidence while the provider settles or Stop arrives | The supervisor still polls settlement and Stop; it waits for evidence as a select branch. | Nonblocking deadline inspection and supervised lock reacquisition. |
| Append definitely fails | Exact facts and generation remain pending; normal dispatch is fenced by typed storage supervision. | `save_observed` acknowledgement boundary and Agent failure path. |
| Append may have committed but acknowledgement is lost | Retry presents the same record bytes and identity; storage reconciles it once. | Semantic record writer and pending generation retention. |
| Stop during stalled append | The write keeps lease ownership; stop cause and provider cleanup are independently bounded. | Session lifecycle and storage lease supervision. |
| Shutdown/crash at a save boundary | A write may be source-visible before its acknowledgement; the manager advances its committed snapshot only after acknowledgement or exact reconciliation. Replay exposes the physical commit once and never executes tools. | Semantic record writer reconciliation, `committed` update after save, and storage replay. |

The panel's provisional live output can still appear before storage
acknowledgement. This slice does not replace gateway views or claim remote
subsecond delivery; those are separate issues.

## Local measurement

Run `cargo test -p nessa-sdk --all-features measure_growing_history_message_commit_latency -- --ignored --nocapture`
to repeat the real SQLite measurement. It appends 64 one-message generations to
one growing history, starts each observation timer before admission, waits the
actual 100 ms deadline, and measures through confirmed `save_changes` and the
manager snapshot. The 2026-09-29 local run measured p50 105.03 ms, p95 106.28 ms,
and p99 106.52 ms observation to commit. It completed 9.53 cadence writes per
second including the policy wait, and 419.47 writes per second when summing
only the actual SQLite save durations. The test is a low-load local sample, not
a remote delivery measurement; link and phone lag require #260/#261/#262.
