# Issue896 — bounded recipient socket-mode review

Disposition: **no findings in the exact fixture delta**. Blocker: empty. Major: empty. Minor: empty. Nit: empty.

Base: `4be5703a83f742ef4c48d6e4e2d8a589fb047d0c`.
Reviewed head: `c59640811a937821cb82d42d69402fa52bd23947`.
Author checkout: `/workspace/nessa-webhook-delivery-review`. Tracked source matches that head. Existing dirty/untracked CI evidence documentation and tooling were explicitly excluded. The entire committed diff is three added lines in `crates/nessa-server/tests/webhooks/infrastructure/product_delivery.rs`: two comment lines and `stream.set_nonblocking(false).unwrap()` immediately after successful accept, before the existing socket timeouts and synchronous Rustls stream.

This is bounded affected review of an expressly authorized recoverable CI repair, **not a fourth whole-feature round or a reset of its counter**. Previous whole-feature reports, closure and the preceding three-file CI fixture review remain unchanged. I made no author-source edit and did not duplicate the coordinator's Cargo session29048.

## Failure correlation and portability

The selected original Mac log `/tmp/nessa-896-ac6225-macos-tls-failures.txt` contains fourteen distinct unnamed recipient thread IDs panicking at old `product_delivery.rs:108:56`, the synchronous `stream.read(...).unwrap()`, each with `WouldBlock`/OS code35. The original run40249699947/job121338743669 ended4101 passed/21 failed/13 ignored. The seven private-root failures are covered by the preceding reviewed fix; this mode correction targets the additional shared recipient-read path. I did not infer that every arbitrary Mac timeout has this cause, or inspect a live failed socket's flag state.

The fixture sets its listener nonblocking so idle accept can poll the stop flag. Apple documents creation of an accepted socket with the listener's properties; the Linux man-pages maintainer explicitly documents the differing Linux/BSD flag inheritance and recommends setting required accepted-socket flags explicitly. Those primary sources support the identified portability defect and the narrow correction: [Apple accept(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/accept.2.html), [Linux man-pages accept(2), Versions](https://man7.org/linux/man-pages/man2/accept.2.html#VERSIONS).

A synchronous `rustls::StreamOwned` reader should use the fixture's existing bounded blocking socket operations, rather than inheriting an immediate-WouldBlock mode. The correction establishes that mode explicitly on the accepted connection only. It does not change production HTTPS or listener/address policy. Actual repaired Mac execution remains necessary; Linux success alone does not establish it.

## Stop, join and watchdog inspection

- The listener remains `127.0.0.1:0` and nonblocking. Empty accept still handles WouldBlock with the existing5ms park and rechecks the stop flag.
- The accepted connection still has the **same3-second read and write timeouts**, configured immediately after the new explicit mode. This review does not turn those per-operation timeouts into a claimed global request deadline.
- The synchronous TLS/parser path, real request-byte requirement, maximum body bound, exact Content-Length completion, response status sequence, response writes/flush and bounded observation channel are unchanged. A stalled or failed read still fails the fixture; it is not swallowed or converted to success.
- The optional held-response gate retains its **same3-second recv_timeout watchdog**. The real-POST/revoke/production-cleanup control still waits with the original assertions and releases its response before confirming cleanup. No POST/signature/no-send assertion or timeout was weakened.
- Drop still sets stop and **joins the actual recipient thread**. It still propagates a recipient panic unless already unwinding. The mode change does not bypass that join, detach a connection, make stop preempt an in-progress read, or invent successful cleanup evidence.

The exact scope receipt `/tmp/nessa-896-ci-recipient-mode-review-scope.json` independently verifies that deleting only those three new lines reconstructs the entire base file. All other fixture text, production, assertions, budgets, watchdogs and join code are byte-identical. `git diff --check` passed.

## Independent mode control

I created only a standalone scratch Rust probe under `/tmp`, compiled directly with rustc (no Cargo/shared artifact mutation), and bound only ephemeral loopback sockets. It explicitly forces the accepted child nonblocking to emulate the inherited BSD mode on this Linux host:

1. Old-mode control: the read returns WouldBlock before the delayed synthetic payload arrives.
2. Corrected mode: applying the exact `set_nonblocking(false)` operation before the same3-second socket timeouts allows the delayed read to succeed.
3. In both cases the client thread is joined, and a fresh idle accept still returns WouldBlock, demonstrating that changing the child did not make the listener blocking.

Both controls passed. Source: `/tmp/nessa-896-recipient-mode-probe.rs`; output: `/tmp/nessa-896-recipient-mode-probe.log`. This is an accepted-socket mode emulation, **not a Mac reproduction or TLS qualification**. Actual canonical TLS/product tests are the separate coordinator-run suite below.

## Coordinator execution and limits

The coordinator's frozen-source, process-local022 webhook selection completed **295/295 passed**,18.92seconds, session29048 terminal0. I inspected the terminal `/tmp/nessa-896-ci-portable-recipient-webhooks.log`; both runtime controls, all three retention controls and actual TLS/product/cleanup cases explicitly passed. This is observed coordinator execution, not an independently duplicated reviewer suite. The Linux result does not establish corrected Mac acceptance. The coordinator is separately running required all-target Clippy for the fixture change; I do not claim its outcome here.

The actual corrected Mac and browser/stress runs remain pending normal new-source CI. Preserve the original Mac failures and distinguish old-source results from the new publication. No Windows/macOS process, real provider/account/credential/recipient, public listener, browser/download, workflow rerun/dispatch, network/security-setting change, publication or merge was performed by this review. No additional correction is requested within this exact fixture-only delta; overall readiness remains with the coordinating owner and its remaining gates.
