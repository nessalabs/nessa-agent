# One-thread pairing timeout and retained cleanup (#777)

The historical Windows failure at explicit floor5s remains unreproduced. Current
main already changed the floor to the shared socket `WAIT`=30s in #843/c611bdde.
This change retains that budget, one blocking thread, real TLS/PAKE, owner-status
and approval assertions. It does not claim that a larger budget diagnoses #777.

The scoped correction is test cleanup: the timed operation borrows the original
initialization future, fixture, stop sender, listener, connection owner and
OS-thread handle. Timeout or panic ends that borrow. The original owners then
finish initialization if necessary, stop/join the listener, drain connections
and gateway, and join the device before the runtime retires. The primary typed
outcome is preserved; ready work uses Tokio's original timeout implementation.
A device join failure cannot make otherwise successful work pass.

`one_thread.rs` contains seven regressions plus the existing responsiveness test
in `enrollment.rs`. Real completed TLS establishes socket admission. The timeout
cases use an already expired real Tokio deadline after the required boundary;
they do not wait for random sleeps to guess when a peer was admitted. The
initialization regression counts polls of one retained gated future and proves
it is not replaced. This is a test harness owner, not a new production cleanup
or authorization implementation. Abrupt process/CI termination is not graceful
cleanup; a genuinely deadlocked worker cannot be declared drained and the
existing outer CI step bound remains the final process boundary.

## Local evidence

[results.json](results.json) records source hashes and the exact package commands.
It is a precommit source checkpoint, not a claim that an uncommitted tree already
had its eventual commit id. Isolated selection passed8/8; the native-enrollment
binary under the CI seven-package selection passed115/115. The package group's
all-target Clippy, formatting, architecture and workflow checks passed. Only
that integration binary was executed locally, not all tests of all packages.

[probes.log](probes.log) keeps small diagnostic excerpts from six independent
counterfactuals. Each mutation's exact replacement and changed-source SHA256
were verified before running its named regression. Each failed with exit101.
The helper was restored byte-for-byte with a fresh mtime after each probe; the
final restored isolated and package runs passed. The stop-sender probe
reproduces the original secondary `RecvError`; it is cleanup evidence, not proof
of what caused the historical deadline to expire. The deadline-first probe is
an intentionally incorrect precheck against Tokio's work-first behavior, not a
change to Tokio or a claim that such a precheck was present in old source.

## Hosted scope and disposition

Initial diagnostic head685622b2 is on run39052123497, tested mergeaf4ac1e81dd63af01019da71ee460cc01b5fc2e6.
Its isolated Windows observation and package step passed; final job completion
and supported phase notices were still pending at this checkpoint. Final cleanup
qualification needs a new exact-head Windows run and independent review.

Use `Refs #777`, not a closing keyword. The narrow merge candidate is reliable
failure-path cleanup and diagnosis visibility. Resolving #777 additionally needs
attribution/reproduction of its original boundary or an explicit evidence-based
owner disposition; fresh green tests alone do not resolve it. PR#962's unknown
Windows failure is separately owned. No denied logs or project-board route is
retried; ProjectV2 status remains blocked by its reported access denial.
