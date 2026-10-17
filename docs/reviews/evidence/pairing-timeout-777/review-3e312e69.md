# Independent exact-head review of PR #980

Reviewed base `e650b901b30b64745cd43920b5b87e82d5442981` through head
`3e312e69ff6906163fcf7b18443617b0f839fcc6` in `/workspace/nessa-777`.
The tree was clean before and after checks. No source, git state, or cached
artifacts were removed or deliberately invalidated. This is a FULL local review
of the eight changed files, under AGENTS.md and CODING_STANDARDS.md, including
the required review brief, agreement checks, lifecycle dimensions and closure.

## Findings

### Major: phase annotations exhaust the per-step display budget before the interesting phases

Location: `.github/workflows/local-auth.yml:419`.
Every phase entry and exit emits a separate notice. The existing responsiveness
probe emits more than ten lines before reaching device confirmation, listener
drain and cleanup. Parent-provided supported-annotation evidence from diagnostic
Windows job `117452691896` confirms exactly ten phase notices, ending at
`enter=device-reply`, with later phases absent even with `per_page=100`.
Thus a passing package result does not make the promised Windows phase timing
inspectable through the supported evidence route. This is consequential because
the historical timeout boundary remains unidentified and raw/denied logs must
not be used. Aggregate phase lines into a bounded escaped notice per command,
while retaining live output and the original pipeline failure status. This
finding was already known to the coordinator; this review confirms its source
mechanism, without independently fetching hosted evidence.

### Minor: initialization failure after timeout replaces the saved primary outcome

Locations: `crates/nessa-server/tests/device_pairing/infrastructure/one_thread.rs:109`
and `:210`; neighboring real constructor at `support.rs:188` (the
`restore_gateway_identity(...).await.unwrap()` sequence).
Trigger: initialization yields, the real Tokio deadline expires, retained cleanup
re-polls that original initialization, then the constructor panics. The saved
`Ok(Completion::TimedOut)` exists at line 203, but line 210 awaits cleanup outside
the catch_unwind. A late constructor panic therefore unwinds `run_owned` instead
of returning the timeout plus the separate cleanup failure.

A real constructor can take this path when its retained startup worker finishes
with an I/O/private-state/worker error after timeout: identity.rs awaits the
physical worker, maps its failure, and Fixture unwraps it. This is not a claim
that that happened in #777. Minimal deterministic regression: initialize a
gated future under an already expired deadline, poll the attempt until retained
cleanup is waiting, release the gate into `panic!("late constructor panic")`,
and assert the returned completion is TimedOut and cleanup separately contains
the panic. Current source instead unwinds the attempt. The existing constructor
panic test only panics inside the timed operation, so misses this ordering.
Preserve both facts without re-polling an unwound constructor. Record the new
ordering in the canonical table before adding its correction.

### Nit: one-thread ownership comment labels the framing module

Location: `crates/nessa-server/tests/native_enrollment.rs:6`.
The new comment describing the one-thread cancellation/cleanup owner sits above
the framing module declaration, rather than the one_thread declaration at line
17. Move it to its actual module so the integration-binary map remains accurate.

## Relationships and adversarial paths checked

The operation owns its responsiveness result; Owners owns initialization,
fixture, sender, listener handle, connection owner and device OS-thread handle.
I traced these together through initialization, creation, listener admission,
real TLS/device reply, owner status/approval, cancellation, unwind and physical
drain. Work borrows Owners, so dropping its future cannot detach these handles.
The default run uses Tokio timeout_at, preserving work-first ready/deadline
polling and cooperative semantics, and runtime still has max_blocking_threads=1.
TLS/PAKE and owner-status/approval assertions remain intact. The sample-times20
with shared WAIT30s floor is unchanged at this base; this is not a new timeout
increase or a reproduction of the old explicit5s failure.

I checked cancellation before fixture completion, atomic transfer of a completed
fixture into Owners, before listener creation, after listener creation and after
physical TLS admission. Fixture.listener_serving has no await between spawning
the listener and returning its owner tuple; the caller has no await between
return and saving that tuple. Awaiting an existing listener handle by mutable
reference retains it across cancellation, and no await separates receiving its
result from taking the completed handle. Device spawn and storing its join handle
likewise have no async cancellation gap. Before-admission paths were inspected
in source; the admitted blocked-peer paths are exercised by the focused tests.

The listener's stop closes admission/wakes sockets and its join collects owned
connection tasks. The subsequent connection shutdown checks real permits and
waits for physical workers, rather than treating wake_report as drain evidence.
Gateway shutdown closes owner/registration admission and waits for original
permits, including an admitted create with a dropped waiter. The OS-thread join
is performed directly after server drain and queues no second blocking-pool job.
The focused timeout test additionally reopens the device's actual FilePairingState.
Panic/device join errors remain separate from work outcomes; successful work
cannot pass assert_success with a failed device join. Ready success plus expired
deadline remains successful work and still drains. The late initialization panic
finding above is the uncovered contradiction between those facts.

Cleanup deliberately has no inner deadline: a truly stuck worker can hold it
until the existing outer CI process bound. This avoids declaring an unconfirmed
physical drain successful or detaching a still-owned worker to make the timeout
test terminate. The evidence README expressly discloses this limit. I found no
new false claim of confirmed drain while workers remain. Abrupt job/process
termination is outside graceful cleanup. No broader production cleanup owner,
policy evaluator, persistence protocol or authorization decision is added.

Reviewed the canonical ordering table, evidence README/results/probe excerpts
and source hashes against the frozen files. All five recorded source hashes
match. The six counterfactuals are reported as implementer evidence, not tests
independently re-mutated by this review. The design/evidence correctly leave the
historical boundary unresolved and require Refs #777, not closing language.
No attribution to separately owned #962 is established.

## Independently executed checks

Environment: Linux; RUSTUP_HOME=/tmp/nessa-777-rustup;
CARGO_HOME=/tmp/nessa-777-cargo; CARGO_TARGET_DIR=/tmp/nessa-777-target;
CARGO_PROFILE_DEV_DEBUG=0; CARGO_INCREMENTAL=0; cargo from the specified home.

- `cargo test -p nessa-server --test native_enrollment one_thread -- --nocapture`:
  8 passed, 0 failed, 0.42s (seven helper regressions plus responsiveness test).
- `cargo test -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-device-client -p nessa-sdk --test native_enrollment`:
  115 passed, 0 failed, 14.47s. This checks CI dependency-feature selection,
  not all integration binaries/doctests in seven packages.
- `node --test scripts/desktop/ci-harness.test.mjs`: 18 passed, 0 failed.
- `git diff --check <base> <head>`: exit0.
- Source SHA256 comparison: all five recorded entries matched.
- Final `git status --short`: empty; final HEAD unchanged.

Cargo test emitted the existing long constant-evaluation warning in env/config.rs;
it did not cause test failures. I did not independently rerun Clippy, actionlint,
architecture suites, formatting or counterfactual mutations; the implementer's
recorded checks cover those and should be refreshed after fixes as appropriate.

## Limits and disposition

No hosted Windows execution was performed by this reviewer. The old diagnostic
Windows result supplied by parent is prior-head evidence only. Final-head
Windows tests, required checks and a new exact-head review remain necessary after
corrections. I did not fetch raw logs, use project-board access, touch credentials,
or alter protected local work. UI/browser, production domain/policy changes,
cross-process termination and unrelated #962 behavior are excluded.

Recommendation: address all three findings, then review the final combined head.
The narrow candidate remains test cleanup plus diagnostic visibility, not an
established correction of #777's historical timeout cause. Do not close #777 on
fresh green tests alone, and do not merge before parent integration-order review.
