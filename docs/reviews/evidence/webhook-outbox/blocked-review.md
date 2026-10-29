# Outbox review blocked: owner handoff

Draft [PR #1096](https://github.com/nessalabs/nessa-agent/pull/1096), branch
`codex/896-webhook-delivery`; reviewed frozen head
`2ef1264a162f7232fa8b52724200260b92513ec7`, outbox source
`c608eaa2de4e24143d349f9eb60fe63b74f34193`. Intended outbox review base
`777da4336084b69e84deefc6f52081160093df37`; stacked PR base is frozen foundation
`6c38915d5201de4cac54500ec4b5ad5e5f34229e` / PR #1091.
No subsequent production source change fixes the findings below.

## Open findings

**Major: retained audit facts bypass full restoration validation.**
`publication.rs`, SavedAudit::checked (lines 120–147 at reviewed head), checks
canonical SQL keys, recordId, registration, optional deliveryId and kind, but
not the rest of the identity, phase, cause, initiator or time fields.
The isolated probe queues one synthetic record, removes its already-published
fixture audit file, changes its saved audit to principal="", before="unknown_phase"
and atMs=-1, and marks that record owed again. Reopen succeeds; due() publishes
those exact malformed bytes as immutable private evidence before returning work.
Expected: Invalid before publication or cleanup. No authorization bypass or
live data disclosure was demonstrated. This is persisted audit integrity.

**Minor: cursor-only position is not decoded at startup.**
`stored.rs` lines 138–152 selects only registration, conversation and scope
from cursors. SQLite accepts position='invalid' despite CHECK(position>=0).
When the delivery is already terminal, open succeeds because check_current
skips terminal rows. Duplicate queue returns the old ID, but the next
contiguous queue returns Unavailable on SQL type decoding. Expected: Invalid
at restore, without acknowledging a usable malformed cursor.

[Exact two isolated probe bodies](review-probes.rs.txt) use the existing test
Fixture. [Independent output](review-probes.txt) passes all 31 existing tests
plus two **bug-demonstration assertions**: 33/33 in 30.58 seconds. The two probe
passes affirm the bad behavior; they are not passing fixed regressions. Source
was copied into /tmp/nessa-outbox-final-review and compiled against canonical
dependency fingerprints. Shared source and Cargo artifacts were untouched.

## Why work stops here

The repository's [bounded review rule](../../../../CODING_STANDARDS.md#the-review-loop)
requires: “Still open after those, the work stops: push what is done to its
branch ... and hand it off in a draft pull request and on its issue.”
Two earlier rounds found consistency omissions; the design was revisited into
one commit owner, shared delivery decoding, generated state projections and
bounded restore accounting. The first fresh round then found unchecked
registration restoration, fixed at c608eaa2. This second and last fresh round
still finds restoration omissions in that component. No further patch or review
loop was started. The parent/person owns the next design decision and usable
feature integration. This is a review hold, separate from denied CI operations.

## Diagnosis and suggested next step

The retained audit representation is an open serde_json Value assembled by
several call sites. Its validator owns only envelope correlations; the rest of
its causal contract is implicit in those writers. Shared publication therefore
does not provide one complete typed restoration owner. Before more runtime
composition, replace that open saved audit representation with the smallest
closed typed audit contract that captures the required registration, cursor and
delivery transitions. Serialize and restore through the same owner; validate
all identity/target/cause/initiator/time fields and legal before/after transitions.
Historical records must remain valid after their registration is retired;
joining them to only current registration facts is insufficient. Do not add
another permission decision or reconstruct missing historical actors. Reject
unknown fields and impossible histories instead of repairing evidence.

Also give the persisted cursor one complete typed decoder for identity, scope,
position and reset flag, used at startup and live reads. Keep SQL maxima legal;
test text, negative and out-of-range positions before any effects. Expand the
ordering table before implementation, turn these bug-demonstration probes into
refusal regressions, mutate the new rules, then obtain a new owner-directed
review plan. This is a proposal for the parent, not implementation approval.

## What is built and what is not

Canonical 61 webhook tests, required seven-package all-target Clippy, 108
architecture tests/checker, formatting and diff whitespace passed before this
independent review. The test counts do not resolve these new findings.
Admission already reuses actual credential and grant owners and Cedar. Private
outbox identity/checkpoint/CAS, bounded workers/queues, source invalidation,
stop cleanup, unknown recovery and owed publication are implemented, with the
above unresolved restore defects. Nessa Docs source checkpoint and diagram are
browsable; docs rendering is not gateway browser acceptance.

Product registration/configuration, principal share target resolution,
credential stop/first-expiry-observer coordination, authorized source replay,
owner cursor reset, supervised sender and live known-outcome reconciliation,
feature capability/revoke TLS E2E and stress/chaos steps remain **unimplemented**.
Future shared files include product state/socket/credential.issue protocol and
generator, LocalAdmin, conversation share-target/grant index, runtime lifecycle
composition and generated limits catalog. Parent must coordinate those with
#1088 and #1052; no inbound workflow engine or application install was added.

The foundation CI browser dependency setup timed out before assertions. Parent
confirmed its narrow retry was denied, including an evidence-backed retry; no
lookup, dispatch, CLI route, no-op push or equivalent trigger followed. Exact
held source/ref inputs are in the [validation handoff](../webhook-validation-gates/README.md).
No live registration, production data, real persistent key, public listener,
security/network change, merge or force-push occurred.
