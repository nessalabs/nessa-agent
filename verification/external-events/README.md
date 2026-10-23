# External event primitive verification (#1088)

The reviewed source is `0c320bd82744247a771773562510d0e5c0b8b479`, based on
`0b8019cf45f99b285f10c41199875d474492c6d1`. The primitive is an explicitly composed,
unmounted HTTP router that dispatches one authorized existing conversation turn.
It does not create a general workflow engine or a public listener.

## Executed checks

Linux x86_64, Rust stable 1.99.1, Node 24.19.0. Cargo builds used two jobs.
[Exact check summaries](checks.json) and the [feature test output](feature-tests.txt)
retain the source checkpoint and outcomes.

- All 16 `external_events::` unit/loopback E2E tests passed. Real HTTP, Cedar,
  SQLite request receipts and the SDK runner are used; only credentials, clock
  and provider are local fixtures. Cancellation tests additionally drop actual
  Axum service futures deterministically before and after command handoff; they
  do not claim to simulate TCP resets. All eight command slots remain occupied
  after post-handoff caller loss until the gated storage calls settle.
- `cargo test -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-device-client --no-fail-fast`
  ran all selected targets. The server library had **3,753 passed, one failed,
  ten ignored**. Its integration targets and the other package targets passed.
  Ignored fixture helpers retain their existing annotations. The sole failure
  is the unchanged cleanup test described below.
- `cargo clippy -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-device-client -p nessa-sdk --all-targets -- -D warnings`
  passed. Rust emits the existing long-running constant-evaluation diagnostic
  for the allowed lease fingerprint calculation; the command still exits zero.
- The affected package formatter and architecture check passed.
- Nessa Docs catalog validation passed: 120 documents, zero issues. Installed
  Chromium 151.0.7922.173 rendered both charts and opened the signature-node
  drill-down without page errors. The checkpoint screenshot is regenerated
  after writing the source revision. Browser download from the Playwright CDN
  returned `403 Domain forbidden`; the installed browser was used instead.

The existing desktop stress `stop`, `fail` and `restart` scenarios own the shared
conversation runner. No UI or conversational command is changed here; this
unmounted HTTP boundary has its separate isolated suite. Desktop UI verification,
live provider registration, real external tools and cross-platform execution were
not run for this backend-only slice.

## Baseline cleanup blocker

`agents::infrastructure::codex::tests::status_cleanup_removes_descendants_after_root_exit_and_after_timeout`
fails with `CleanupUnconfirmed` at `tests/agents/codex.rs:95`. It fails in the
changed checkout, in isolation, and when **only that exact test** is run on the
unmodified recorded base in an isolated worktree with a separate build directory:

```sh
cargo test --offline --manifest-path /workspace/nessa-agent-base-1088/Cargo.toml \
  -p nessa-server --lib \
  agents::infrastructure::codex::tests::status_cleanup_removes_descendants_after_root_exit_and_after_timeout \
  -- --exact --nocapture
```

The base worktree was clean. Its test had zero passed, one failed, and 3,747
filtered out. The unchanged checkout's isolated reproduction had the same cause.
Read-only `/proc` sampling during the base test observed the fresh fixture's
`sleep` as a zombie owned by PID 1, in process group 71624. The group remained
represented when the cleanup owner returned uncertainty. No tracer, reaper,
assertion relaxation or production cleanup change was introduced. This is evidence
of the base/environment blocker, not a claim that the primitive fixes cleanup.
See [base failure](baseline-cleanup.txt), [changed failure](changed-cleanup.txt),
and [ownership observations](baseline-cleanup-ownership.json).

The isolated base build cache was removed after preserving these results to free
workspace disk. The clean detached source worktree remains available.

## Mutation probes

[HTTP results](http-mutation-probes.json) record a passing baseline and twelve
individually broken rule variants that compile but fail the boundary tests.
Those variants cover HMAC verification, binding scope, timestamp freshness and
canonical spelling, repeated headers, strict envelope shape, body bytes, body
read deadline, admission capacity, caller-loss ownership, content type and
decode-allocation bounds.
The source SHA-256 is recorded in each result.

```sh
python3 verification/external-events/probes/run.py
```

This tool copies the actual HTTP adapter to a temporary crate, changes one rule
at a time and resets the copy afterward. Application ports are deterministic
substitutes in that harness; this is **HTTP adapter evidence**, not an alternative
claim to have exercised real authorization or execution. The real ingress suite
above owns that integration evidence. Cargo dependencies must be cached because
the probe command uses `--offline`. The shared source tree is not mutated.

[Domain results](domain-mutation-probes.json) and [key results](key-mutation-probes.json)
record five additional constructor-bound probes against the actual source.
Compact `Box<str>` and `Box<[u8]>` storage remove the supplied spare capacity;
regression tests also accept a one-megabyte-capacity input and preserve its small
value through the compact type. A separate real application authorization probe
is recorded in [authority-mutation-probe.json](authority-mutation-probe.json).

## Independent review and publication

Round 1 independently reran the original 11-test binary and found retained spare
capacity, new-domain layout, public API documentation and documentation precision
issues. The implementation fixes each: compact types and regression tests;
`domain/value_objects/`; scoped missing-docs enforcement and API docs; explicit
principal-binding, clock-skew and response-versus-receipt wording.

Round 2 found that a borrowed HTTP slot was released while detached command work
continued. The design ordering table was revisited with the owner. The corrected
per-admission task owns the slot until its command result; three new regressions
cover cancellation before/after handoff and task failure. The caller-loss mutation
probe independently fails when ownership moves back into the response future.
Round 3 found an incomplete test observation barrier: a dispatch receipt was
followed by a provider count before every queued turn was observed complete.
The test now retains each replay receipt and waits for that execution's authoritative
completed status. This separates command settlement from execution observation.
The fresh follow-up review and its exact scope are recorded in [review.md](review.md).

GitHub's read of issue #1088 returned `Forbidden`; `gh auth status` reports an
invalid supplied token. No denied GraphQL issue operation or alternative issue
access path was attempted. Draft publication status is recorded in the final
handoff; parent owns any exact-head merge after hosted gates qualify the blocker.
