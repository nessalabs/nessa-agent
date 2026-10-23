# Total contextual restoration correction

Source `fb0f403648a2bb9d8dc6f43385c1e1e5371588f2`, parent-authorized structural
correction after [fresh round two](../typed-state/review-2/report.md). Published
findings and bug demonstrations remain intact; their passing demonstrations
confirm defects at the historical source, not fixes.

One restoration owner now exhaustively matches Registration, Cursor and Delivery.
It checks each against retained owning facts at startup and validates the whole
owed batch before its first immutable publication effect. Delivery histories
remain correlated through the same owner. Registration Issue matches original
target and issuance time; Stop matches its original cause/actor/time. Cursor
matches prior checkpoint and captured observed scope/reader/time.

Private retirement still deletes registration URL/key/cursors before its audit
acknowledgement. Bounded secret-free facts retain original target, issuance,
stop, retirement time and exact retirement audit identity. The deferred foreign
key retains that owed identity. Historical Issue/Stop evidence from the same
retirement correlates with those original facts, while Drained requires them
and no live registration. Acknowledged audit/fact projections trim together.
Original actors are never synthesized from current ownership. Cursor reset
observations remain checked even after their published audit is trimmed.

## Exact-source validation

Linux cloud workspace, Rust1.99.1, synthetic private fixtures only. Required
seven-package all-target Clippy `-D warnings` passed in69s. Architecture
108tests/checker, formatting and diff checks passed. Final copied-source isolated suite passed **50 tests**, zero failures,61.60s;
[log](isolated.txt). Canonical `cargo test -p nessa-server --lib webhooks --no-fail-fast` passed
**80 tests**, zero failures,3749filtered,58.29s; [log](canonical.txt).

[28 copied-source fail/restore mutations](mutations.json) each caused its intended
regression failure; each affected test passed after restoration. Whole Registration,
Cursor and Delivery context arms were individually removed as probes. Other
probes cover target, original issue/stop facts, retirement identity/time/cause,
reset checkpoint/scope/reader/time, owed-batch admission, FK and retention limits,
secret-free cleanup and whole reset decoding after audit trim. Source copies
were restored byte-identically. [Run](mutation-run.txt), [script](mutations.py),
[exact source hashes](source-manifest.json). Harness substitutes only Target DTO
and uses canonical dependency artifacts; this is not complete Cedar/sender E2E.

The first architecture invocation omitted the installed Cargo PATH and failed
one metadata test with spawnSync cargo ENOENT (107/108); the corrected environment
passed108/108 plus checker. Intermediate tests/Clippy before the last added
reset timestamp probe remain diagnostic under /tmp. The final logs linked here qualify this source. No test assertion or security gate was weakened.

## Review and remaining work

Fresh independent structural review is pending. No persistence approval, merge
qualification or full #896 completion is claimed. Composition worktree remains
unchanged until clean persistence review, as directed by the parent. All actual
registration/share/source/sender/revoke/expiry/capability E2E and feature stress/
chaos gates remain unimplemented or unpassed. Current main d029d1cf, including
merged #1088 and #1095, must be preserved before shared composition edits.
Application installation #1052 remains separate; no paired principal is invented.

Canonical Chromium/WebKit remains required; docs rendering does not waive it.
Foundation #1091 stays frozen6c38915d, denied CI retry120322813252 is not attempted
or routed around. Parent owns that action and guarded merges. No live registration,
real persistent key, public listener, production data, security/network changes,
merge or force-push occurred.


## Self-review before independent handoff

After source ef015c87 passed79 canonical/49 isolated checks, an implementer
copied-source probe showed that live publication checked individual Delivery
context but did not recheck the complete history/current end. A locally valid
refusal could replace the first queued audit and publish while the owning row
remained Queued. The [demonstration](self-review-ef015c87.txt) affirms that earlier
acceptance; [exact probe](self-review-ef015c87.py). It was not a passing fix.

The current source reuses the complete restoration history validator for only
deliveries selected by unpublished audit identities before the first publication.
Startup still validates all histories; ordinary publication does not scan every
unrelated retained history. `live_owed_delivery_history_matches_current_before_publication`
turns this into a refusal regression. Fresh source checks include28 caught/restored mutation probes and80 canonical
webhook tests. Earlier ef015c87 checks do not qualify the changed source.


The final source remained frozen throughout validation; source/manifest hashes
match. Docs catalog122documents/0issues, chart rendered in Chromium151 and
visually inspected with0pageerrors; [catalog](catalog.json), [render](render.json),
[diagram](diagram-1.png). This is docs evidence, not gateway browser acceptance.
Fresh independent review remains pending; validation alone does not close it.
