# Boat alpha verification

The screenshots show the in-memory browser surface, including its honest
unavailable gateway state in Settings. Actual account behavior is tested through
the isolated real gateway; no token, login file or real user folder is committed.

`boat-provider-e2e.json` records the final source build's actual provider checks,
binary/worker provenance and confirmed absence of all four disposable VMs. The
separate provider-correlation observations confirm `NESSA_THREAD_ID` in real VMs.
The approved local-read fixture exposes one generated log on the Mac; cloud
writer access is confined to its own empty disposable VM. No real folders mount.

```text
coordinator dispatches real child on another VM
missing VM fails without replacement or Local fallback
PASS
conversation-stress: 12/12 held
receipt publication panic: expected test failure
```

A terminal read after external deletion is not a cleanup oracle. The missing-VM
check starts a fresh gateway after confirmed provider absence and requires the
original saved assignment to fail with `environment_unavailable`, without any
replacement or Local fallback. Surviving live connections may instead retain
unconfirmed cleanup; the runner never resets that uncertainty.

Final affected suites: 17 workspace tests, 6 coordinator tests, 1 real durable
audit test; all pass. The 8 recovery audit/cleanup combinations, caller loss,
port panics and exact serialized-byte boundary have regression coverage. Revert
probes remove the protective rules one at a time and fail, then the source is
restored and affected suites pass. Raised channel stress holds 12/12; signed-out
scripted gateway checks hold in Chromium and WebKit. Frontend 5,325 and SDK library
1,537 tests pass. Type checking, lint, production build, architecture, generated
protocol checks, docs, format and Clippy pass.

Broad checks have limits: baseline tooltip/focus/share fixtures are tracked in
#1162, and the macOS temporary-path alias test in #1165. A later full server run
had 3,884 passes and one unrelated process-status timing assertion; its isolated
retry passes. An earlier selected-package run passes including 3,883 server
library tests. These results do not claim the broad desktop run is green.

Independent local review ran three rounds. Receipt byte bounds, coordinator
retirement failure ordering, target-bearing recovery evidence, publication-port
panic ownership and Unix-only imports were corrected and rechecked. The closing
review found no remaining issue in its source scope; no Windows build was run
locally. CodeRabbit skipping a review is not an approval. Review dispositions
and the source checkpoint are on the pull request; unchanged atlas pages retain
their existing checkpoints.
