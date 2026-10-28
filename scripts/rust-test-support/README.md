# Isolated Rust tests

`isolated.rs` is shared, test-only source included by the SDK physical-fault
ownership tests and native-device owner-original preparation tests. Both crates
already depend on `tempfile` for tests. It is not compiled into product builds.

The parent starts exactly one libtest case and waits up to 60 seconds for its
entry marker. On observing entry, it starts a separate 30-second execution
watchdog. Result channels inside that child can wait for actual work instead of
using short filesystem/scheduler deadlines. These are hang-detection budgets,
not performance claims or product timeouts.

| Observation | Parent action |
| --- | --- |
| Child has not entered | Keep the startup deadline; do not spend execution time |
| Entry observed | Start the execution deadline once |
| Child exits | Reap it and inspect status, entry and one-test summary |
| Either deadline expires | Kill and wait; retain the expired phase and diagnostics |
| Observation unwinds | The owned child guard attempts kill and wait |

Stdout and stderr use files instead of pipes so child output cannot block on a
full pipe while the parent waits. The report reads at most the last 64 KiB of
each stream and refuses a truncated capture. Temporary files are removed when
the parent returns. The harness owns only its direct child; its callers exercise
threads and private storage, not a process tree. It must not replace a product
process supervisor.

The included control tests cover six-second startup without spending execution
time, child panic diagnostics, and kill/wait on startup and execution hangs.
They run as part of each including crate's library suite. Device tests also
exercise delayed actual private preparation for both a proof and the original
uncertain failure; the lease, close and reopening checks are retained.

Refs: #1060 and #1074. No SDK, device-session or statechart behavior changes.
