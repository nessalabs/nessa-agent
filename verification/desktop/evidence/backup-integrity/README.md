# Backup integrity verification (#901)

The source was frozen on base `839b1d50352c833c888254130456ac1355cccfa6`
with the 17 changed source/test/documentation files in
[source-manifest.json](source-manifest.json). Their hashes were checked after
verification and before commit. The final rerun used `5bfa447d` plus the
blocking-pool correction in `log.rs` and its design row, as those hashes record.
This directory contains synthetic evidence only;
raw temporary tokens, ACP/MCP session logs and screenshots are not copied here.

## Scripted gateway check

`pnpm test:e2e:scripted -- --channel bundled` passed in dev mode on Linux,
with the scripted agent signed out: **37/37 assertions**, zero failures.
Chromium and WebKit both passed (columns layout): MCP apps 16/16, gateway
window 13/13, permission/failure/cancel scenarios 8/8. The full verdict and two
known-harmless aborted HTTP response lines are in [pr-summary.md](pr-summary.md).

The run used Rust 1.94.0 and workspace-local browser binaries. WebKit libraries
were extracted into the workspace instead of installed system-wide. Host
`ldconfig` validation was bypassed after both engines' actual launch/render
probes passed; the complete browser assertions then passed too.

## Rust and architecture checks

- Rust 1.94.0: `cargo test -p nessa-server --lib backup::`: **94 passed**,
  including the restored reconciliation lease after its revert probe.
- Same frozen implementation: library binary `composition::`: **332 passed,
  2 ignored**; `conversation::`: **1049 passed, 4 ignored** (including the
  slow app-review case). These suites were rerun after the blocking-pool fix.
- Current CI stable Rust 1.99.1: `cargo clippy -p nessa-local-storage -p
  nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p
  nessa-device-client -p nessa-sdk --all-targets -- -D warnings`: **passed**.
- `cargo +stable fmt --all -- --check`: **passed**.
- `node scripts/check-architecture.mjs`: **passed**.

An initial Rust 1.94.0 Clippy run failed on `nonminimal_bool` in the unchanged
`relay/application.rs:327`. No relay code was changed. The required package
selection passed with CI's current stable toolchain. The pre-existing
lease-protocol long constant-evaluation diagnostic appeared during builds.

## Regressions distinguish the fixes

| Relevant original hunk / probe | Observed failing assertion before fix |
| --- | --- |
| Offline rotation startup capture absent | Inventory had 0 credential fences instead of 2; damaged-source startup returned success instead of an authentication error (two tests). |
| First-use outbox/publication locks absent | Lock file missing, or a file/symlink parent accepted (all six dedicated lock tests failed). |
| Share capture conditional on `receipt.applied` | Equivalent no-op share performed 1 capture total instead of 2. |
| Separate audit/inventory publication locks | Decision write completed while the inventory rollback copy was live. |
| Blocking reconciliation wait with one pool worker | Holder snapshot work could not progress while a later capture occupied the worker waiting for its lease. The regression failed after its 2-second progress deadline. |
| Reconciliation acquisition removed, with edit verified | Newer no-op capture returned while the older revoke read was still pending. |

The fixes were restored. The 94-test backup suite passed again. The concurrency
coverage also checks separate file adapters, dropped reconciliation waiters,
shared publisher cleanup with injected directory-sync failure, and current
credential/receiver correlation. Existing aggregate outbox budget, failed-source,
Cedar decision, pairing-cancel and deletion tests remain in the passing suites.

Full restore/quarantine and #854 oversized-session fetch/import are not verified
or implemented by this change; see the export-cut design's recovery acceptance.


## Independent review disposition

The first independent review of `5bfa447d` found one major liveness defect:
blocking lock waiters could occupy the pool needed by the holder's metadata
snapshot. Publication remained blocked. Acquisition now opens the file in short
blocking work and uses `try_lock` with an asynchronous contention wait. The real
file-adapter regression `waiting_captures_leave_a_blocking_worker_for_the_holder`
failed before the correction and passed after it; cancellation and separate-opener
checks still pass. A fresh second review is tracked on the pull request.
