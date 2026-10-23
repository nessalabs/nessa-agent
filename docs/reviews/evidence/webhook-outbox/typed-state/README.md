# Parent-authorized typed persistence redesign

Exact source: `1c3085add14833a862681f6697f1e4ea31f9ba86`, based on prior blocked
handoff `bb51b40d2df849546918cd7006d55810e32c2ee6`. The parent explicitly adopted
the focused typed persisted-state redesign and directed work to continue on
2026-10-23. [Prior open findings and exact reproductions](../blocked-review.md)
remain intact; they are historical evidence, not erased or labeled green.

One closed audit representation now owns every producer, write validation,
restore and publication. Historical delivery records include complete before
and after snapshots, checked through the existing domain transition owner.
Retained chains preserve target/checkpoint and source context and finish at
the current delivery. Historical retirement actors do not depend on a current
registration. One complete cursor decoder serves restore and live reads,
including duplicate checkpoint acknowledgement with only terminal history.
No new authorization model, migration or compatibility format was introduced.

## Checks

Linux cloud workspace, Rust 1.99.1, synthetic private fixtures only.
Required seven-package all-target Clippy `-D warnings` passed; architecture
108 tests and checker passed; formatting and diff whitespace passed.
Canonical `cargo test -p nessa-server --lib webhooks --no-fail-fast`: **69 passed**,
0 failed, 3749 filtered, 45.57 seconds. Independent copied-source run: **39 outbox
tests passed**, 44.41 seconds. [Canonical log](cargo-webhooks.txt),
[isolated log](isolated-outbox.txt). Fresh independent review is pending; this
evidence does not claim either complete #896 or clean review.

[29 deliberate copied-source mutations](mutations.json) each caused the intended
regression failure; the source was restored byte-identically and all affected
tests passed afterwards. The harness uses the canonical dependency fingerprints
and a Target DTO substitute, so it is storage evidence, not Cedar integration.
No canonical source or shared build artifacts were mutated by these probes.

The new tests convert the prior two defect reproductions into Invalid refusals
before publication. They cover malformed fields and unknown fields, impossible
and uncorrelated history, cursor text/real/negative/reset variants, valid maximum
integer position/time, and a lost retirement publication acknowledgement with
restart after the registration has already been deleted. Original revoker and
exact audit bytes survive recovery. Test-only constraint bypass is used solely
to construct corrupt private fixtures; production constraints remain enabled.

Earlier diagnostic runs are preserved under /tmp: the first Clippy found unused
code, a large enum and needless borrows; these were corrected. The first cursor
regression classified SQLite's earlier quick_check refusal as Unavailable;
known corruption/version errors now become Invalid while I/O failures remain
Unavailable. Earlier 31- and 37-case isolated passes are not final-tree proof.
Captured text logs normalize trailing blank lines only.

Nessa Docs validation: 122 documents, zero catalog issues. The updated chart
rendered in system Chromium 151 and was visually inspected for readable labels
and no overlap; zero page errors. [Render](render.json), [diagram](diagram-1.png).
This is docs presentation evidence, not canonical gateway browser acceptance.

## Remaining acceptance

Registration/configuration, credential stop/first-expiry coordination, authorized
source/replay, supervised sender and live settlement, principal share target,
owner reset admission, capability/revoke TLS recipient E2E and feature stress
remain unimplemented. Browser/stress gates remain applicable and unpassed.
Foundation source remains frozen at 6c38915d; parent reports the specific retry
of job 120322813252 was denied again at 14:09 UTC after fresh exact-head checks.
No retry, lookup, dispatch, alternate CLI or indirect trigger is attempted here.
Parent owns that CI action and final guarded merges. The redesign is independent
substantive implementation, not a way to retrigger foundation CI.
