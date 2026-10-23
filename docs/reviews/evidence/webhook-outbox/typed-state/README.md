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

## First fresh review and correlation correction

The first review of bb561e55 found two major persisted-evidence defects;
[full report](review-1/report.md), [independent probes](review-1/probes.txt),
[39 original tests plus three bug demonstrations](review-1/tests.txt) remain
preserved. Demonstration passes affirm defective behavior at that older source.

Correction source `26b2bd355f8d3d203b0db76b94f9f78a2da41ffe` retains a closed typed
credential-stop fact on authority-stop delivery evidence, checks cause/actor
projections against it, and correlates it with the immutable current registration
stop. It compares complete settled source context with the enclosing delivery;
only Sending permits capture to precede its observation audit. The old missing
terminal context and substituted-actor probes now have explicit refusal
regressions, including coordinated actor/context, stop time and false stop against
an active registration. Valid Sending capture/reopen and canonical failed-audit
replay remain positive controls. No current owner replaces historical actors.

The sealed corrected isolated suite passed **42 cases** in 43.61 seconds, and
required seven-package all-target Clippy passed. Ten new copied-source fail/restore mutations caught these rules;
all affected tests passed after byte-identical restoration. Final sealed-source canonical suite passed **72 webhook tests**, 0 failed, 3749
filtered, 42.66 seconds; [log](correlation-cargo.txt), [Clippy](correlation-clippy.txt),
[isolated](correlation-isolated.txt), [exact hashes](correlation-source-manifest.json).
The second fresh independent review completed at dc86bc2d with two major
findings; see the current hold below. No clean persistence review is claimed.

The corrected Nessa Docs page checkpoints exact source 26b2bd35: 122 documents,
zero catalog issues, diagram rendered and visually inspected Chromium 151 with
zero page errors. [Render](correlation-render.json), [diagram](correlation-diagram-1.png).
Earlier pre-seal invocations/logs under /tmp remain diagnostic history; final
sealed runs include the last added cause/context regression case. No source
bytes changed during sealed validation. Docs rendering does not waive browser
or feature stress gates.

## Second fresh review: structural decision required

Frozen review head `dc86bc2d2cc4ee406b6bca8e73cdc721777177e5`, source unchanged
at `26b2bd355f8d3d203b0db76b94f9f78a2da41ffe`. [Full report](review-2/report.md),
[registration probes](review-2/add-probes.py), [cursor probe](review-2/add-cursor-probe.py),
[demonstration output](review-2/probes.txt) and [combined output](review-2/all-tests.txt)
preserve the exact reproductions. **Two major findings; blocker/minor/nit empty.**
Independent copied actual source passed 42 existing cases plus three bug
demonstrations, 45 total in 44.26 seconds. Those three passing demonstrations
confirm defects; they are not fixed regression evidence.

An owed registration-stop record can substitute its original revoker, or an
issue record can substitute its target, while the unchanged owning registration
is retained. An owed cursor source-change record can claim prior position 999
while its retained reset checkpoint remains at 1. Both reopen and publish.
Record-local typing covers every variant, but owning-state correlation currently
covers delivery evidence only. No POST authorization bypass was demonstrated.

The [bounded review process](../../../../../CODING_STANDARDS.md#the-review-loop)
requires revisiting the design with its owner after consecutive findings in the
same component. This second fresh round follows the parent-adopted redesign,
which itself followed the earlier bounded handoff. No further patch or extra
review cycle is started. The parent owns the structural decision; runtime
composition remains unchanged as explicitly required until persistence is clean.
This is a review hold, distinct from the denied foundation CI retry.

Suggested next design: make contextual validation a total responsibility across
all closed audit variants at restoration/publication admission. Define which
owning facts must survive each owed transition and how retained evidence can
legitimately precede a later state; do not force historical retirement to join
deleted registrations or reconstruct historical actors. Specify registration
Issue/Stop target and original-stop correlation, Cursor reset checkpoint
correlation, and existing Delivery history correlation together before changing
code. Turn these demonstrations into refusal regressions and retain positive
historical-retirement/Sending-capture controls. This proposal is not an approved
implementation or a reset of review bounds.

Source checks remain 72 canonical/42 isolated, required Clippy/architecture/fmt;
they do not resolve the findings. Full source/sender/registration composition,
capability/revoke E2E and canonical browser/stress/chaos gates remain outstanding.
Main d029d1cf includes merged inbound #1088 and #1095; runtime worktree stays at
bb561e55 with no edits. No shared auth/grant changes were made in this round.
