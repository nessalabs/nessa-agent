# Native device verification

`check-saved.swift` is compiled beside the generated Swift declarations and linked
with the real `nessa-device-client` dylib. It performs cache creation/query/reopen
on a dedicated dispatch thread, checks absence versus typed failure, and retains
an owned snapshot after releasing its cache handle. Its temporary directory is
private and disposable.

This first harness exercises an empty historical cache. It does not prove paired
records/checkpoints, receiver revocation, phone UI integration, artifact recovery
or device execution.

`check-paired.swift` runs on a dedicated dispatch thread against the existing
real paired-gateway fixture. It checks downloaded/applied/fact/generation values,
actual scope and catalogue, saved projection after handle close/reopen, offline
reads after gateway termination, an unchanged walk after gateway restart, and
authenticated revocation with its independent durable purge receipt. The epoch
fixture also checks Active status followed by a refused read and a separate
refused pinned recheck: confirmed partial physical records remain, incomplete
semantic units do not advance applied progress, and neither refusal claims purge.
Offline recreation preserves that progress while marking the projection stale.
Stopped
dispatch, invalid code, excessive page bounds and closed-session errors cross the
generated error ABI. Counts are actual fixture records; this does not prove the
phone repository or physical-device behavior.

The paired fixture also supplies `NESSA_NATIVE_EXPECTED_DEVICE_KEY` as hex and
`NESSA_NATIVE_EXPECTED_CREDENTIAL_ID` from its authenticated owner claim/approval,
not from the native client publication. The generated local enrollment getters
must match that key and credential/receiver, retain the saved intent and 44-byte
pinned gateway SPKI, and return no current local record after terminal removal.
Auth's known-seed ring test independently checks DeviceKey derivation. These
are locally saved correlations; they do not establish current read authority.

`check-worker.swift` compiles with the generated declarations and the production
`crates/nessa-device-client/swift/NativeSessionWorker.swift`. Held calls and real
private-state locks verify Busy before enqueue, stop before dispatch, preserving a
physical result after waiter cancellation, close before dispatch, shared close
despite a canceled waiter, scope release after drain and abandoned-worker cleanup.
Public capture overloads keep bounded passive translation in that same admission.
An actual status result is held in its capture while another call is refused,
the waiter cancels and close retains the private lock; release preserves the
captured result and permits reopen. A throwing capture releases admission.
Moving status capture after `perform` returns fails this held-capture regression.
The same held-capture case exercises the local-enrollment optional publication;
moving that capture after `perform` also fails, before private scope release.
These are dispatch/private-state experiments, not a claim that a real network socket
or physical phone was interrupted by Swift cancellation.

Compile it beside generated Swift with Swift 6 complete concurrency checking,
the generated modulemap and the same native dylib. Pass that executable as
`NESSA_NATIVE_SWIFT_CHECK` and its dylib directory as `DYLD_LIBRARY_PATH`, then run
`cargo test -p nessa-server generated_swift_runs_paired_native_lifecycle -- --ignored --nocapture`.
The explicit ignored test runs all five stages, starts disposable gateway processes and supplies public
receiver/origin and private fixture paths; no credential is printed or passed as
a command argument. It is not a substitute for running ordinary Rust tests.

Compile the worker probe by replacing `check-paired.swift` with `check-worker.swift`
and adding the production worker source. Run the resulting executable with the same
dylib directory. Strict checks use `-swift-version 6 -strict-concurrency=complete
-warnings-as-errors`. The Apple package build is `scripts/build-native-device.sh`;
that tool creates a fresh local package and does not claim simulator execution.

`check-simulator.swift` is a disposable SwiftUI app linked with the generated
declarations, worker and release iOS library. Its simulator run reports private
session creation, off-UI dispatch, typed no-credential refusal, close/reopen and
closed-worker refusal. The same source links for arm64 device, but physical-device
execution remains unverified. The product Nessa app is a separate integration gate.

The static Swift package has also linked/executed the paired fixture on macOS.
The compiler rejects an external caller supplying the worker's internal queue,
including a main-queue target. Raw generated synchronous methods still require
their documented caller dispatch discipline.

Receipts expose exact historical public intent bytes, generation, expiry and the
owner's three consent classes. These fields carry no private key, owner principal
or claimed-key proof. A changed retry attempt makes `sameIntent` false while the
enrollment's other public correlation remains equal. Saved entry keys can be
restored through the existing sync Id validator without normalizing identity or
narrowing creation revisions; invalid arguments do not pretend a cache failed.
The paired Swift checks restore a real key and continue catalogue paging through
the current issued receiver/scope owner. Phone profile/enrollment coordination and
the admitted local claimed-key correlation API remain separate required work.
