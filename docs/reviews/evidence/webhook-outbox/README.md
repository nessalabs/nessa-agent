# Outbound webhook persistence evidence

Source checked: `c608eaa2de4e24143d349f9eb60fe63b74f34193` on
`codex/896-webhook-delivery`, dependent on foundation PR #1091. This evidence
covers storage and admission foundations, not a usable composed #896 feature.
Registration authorization, credential stop/expiry coordination, source replay,
sender supervision, owner reset admission and capability/revoke E2E remain open.

**Review blocked.** The bounded final round found one major audit restore gap
and one minor cursor restore gap. No further code patch/review loop was started.
See [the owner handoff](blocked-review.md).

## Checks and environment

Linux cloud workspace; Rust 1.99.1; synthetic identities, keys and private
SQLite/filesystem fixtures only. The source manifest names exact file hashes.

- `cargo test -p nessa-server --lib webhooks --no-fail-fast`: 61 passed,
  0 failed, 3749 filtered; 37.93 seconds; [log](cargo-webhooks.txt).
- Required CI package selection, all targets, Clippy `-D warnings`: passed,
  [log](clippy.txt). Selection: nessa-local-storage, nessa-auth, nessa-server,
  nessa-protocol, nessa-client-core, nessa-device-client, nessa-sdk.
- Architecture test suite and checker: passed, [log](architecture.txt).
- Formatting and committed diff whitespace: passed.
- [Browsable state](../../../state/services/gateway/webhook-outbox.md) uses the
  exact source checkpoint. Catalog: 122 documents, zero issues. Diagram rendered
  in system Chromium 151, visually inspected: labels readable, no overlap,
  zero page errors; [render metadata](render.json), [diagram](diagram-1.png).

Captured text logs normalize trailing blank lines only.

Cargo environment was `PATH=/tmp/nessa-cargo/bin:$PATH`,
`CARGO_HOME=/tmp/nessa-cargo`, `RUSTUP_HOME=/tmp/nessa-rustup`,
`CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`. Existing lease-protocol constant evaluation
notice remains in successful Clippy output; it was not suppressed or repaired.

## Falsification and review

[19 redesign mutations](redesign-mutations.json),
[6 audit/source-context mutations](context-mutations.json) and
[9 registration restore mutations](registration-mutations.json) all caused
expected regression failures. Copies under /tmp were compiled against the
canonical dependency fingerprints; canonical source was not mutated. The
isolated wrapper substitutes only the registration Target DTO and is not
credential/Cedar admission evidence. The audit acknowledgement mutation spins
on zero affected rows; its isolated process was terminated after three seconds.
Restored affected tests passed. The canonical Cargo suite remains the execution
proof for the assembled module.

Two earlier review rounds found retention/identity/projection and cleanup gaps.
The journal was then redesigned around one commit owner and shared decoders;
round-two evidence under [round2](round2/) is historical, not final acceptance.
The first fresh review of redesign source e7ecfb1b passed 29 outbox tests and a
legal maximum-position control, but found malformed registrations bypassing
restore validation and reaching audit identity fields. Source c608eaa2 adds one
checked registration decoder for startup and live audit/retirement consumers.
Ten malformed persisted cases refuse before effects; legal revoked/expired
controls preserve maximum i64 timestamps. Nine fail/restore probes cover the
new rules. The last permitted fresh follow-up round remains blocked: saved audit fields
and cursor position bypass complete restore validation.
[Findings, exact reproduction, diagnosis and owner next step](blocked-review.md).

Earlier diagnostic failures are preserved in /tmp: an unindexed full-retention
case exceeded two minutes before indexes were added; test imports initially
failed compilation; a mutation script initially generated invalid Rust before
its replacement was corrected. Initial validation invocations missing Cargo
PATH or selecting nonexistent nessa-acp did not run their intended checks;
corrected canonical invocations are the successful logs above. None is reported
as a passing gate.

## Applicable gates still open

See the [canonical browser/stress handoff](../webhook-validation-gates/README.md).
Docs rendering in system Chromium proves diagram presentation only. The local
canonical gateway run could not start its pinned Chromium/WebKit binaries.
Foundation exact-head hosted run 39925346689 failed installing pinned browser
dependencies after 15 minutes; gateway browser/stress assertions did not run.
Chaos subsequently lacked its build. Parent confirmed all other jobs passed.
A single-job retry of 120322813252 was denied, including an evidence-backed
retry. No workflow lookup, dispatch, CLI route, no-op push or other equivalent
trigger is authorized to evade that denial. Parent owns any further approval.

This persistence slice adds no product surface, so it does not prove full
feature gate 19. Existing stress/chaos exercise shared gateway restart,
subscription, access-change and uncertain-ack paths; outbound recipient
capability/revoke steps and default/raised runs remain required with composition.
No exactly-once remote effect, live recipient or production behavior is claimed.
