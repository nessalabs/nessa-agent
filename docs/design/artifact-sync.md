# Artifact source ownership and verified synchronization

Owner: [#273](https://github.com/nessalabs/nessa-agent/issues/273).
Status: local persistence, manifest facts and tracked bounded range reads are
implemented; assembled author preflight and independent review remain pending.
Protected transport is not active. The shared worker producer is `51bfcc01`,
assembled locally as `bd7abd02`, on merged `c2f3b0ec`. The linked-device
integration plan owns cross-feature activation; this document owns the artifact
source state and persistence proposal. [Architecture](../ARCHITECTURE.md),
[dependency injection](dependency-injection.md) and the
[coding standards](../../CODING_STANDARDS.md) remain their existing owners.

## Merged baseline before this slice

The [attachment module](../../crates/nessa-server/src/attachments/mod.rs) keeps
bytes once per stored digest and one hold record per organization, conversation,
stored digest and media type. `LocalAttachmentStore` atomically publishes each
record through a private temporary file, file sync, replacement, and directory
sync under its change lock. Pending records are invisible but retain bytes.
Creation audit and a conversation ownership recheck precede confirmation.
Confirmation may take over a pending claim; a Kept record is not replaced.
Release currently removes records and then purges unheld blobs. A corrupt record
protects bytes conservatively and reports failure. Startup removes temporary
files; it does not reconstruct missing holds from audit.

`FilePath` and tool content in the SDK are observations, not permission to capture
host files. This source proposal covers existing held uploads. It adds no
provider-path capture, product methods, paired receiver or runtime activation.
The reusable artifact contract is implemented at pinned `nessa-sync`
`f2a05ef24fcff66df9d55e508539cec718e2d805`: manifests, exact bounded chunks,
durable resume and verified publication already have owners there.

## Identity and content

Each usable held lifetime has one immutable registration. Its content describes
`Hold.stored`: normalized image bytes when normalization occurred, not the original
upload. Live has revision1; retirement has revision2 Deleted. A subsequent upload of
the same bytes creates a new lifetime and identity. The digest, filename, media
type and display name do not serve as registration identity.

`ArtifactId` is a fixed hexadecimal encoding of SHA-256 over the explicit
`nessa attachment registration` domain separator and the exact existing saved
claim-generation bytes. This is an encoding of the identity minted at the hold's
pending write, not a hash of file attributes. The durable generation remains the
claim owner; no separately persisted artifact id can disagree with it. It becomes
externally usable only when confirmation chooses the final generation. A pending
claim can change; an advertised Kept generation cannot. Production currently
mints UUID generations. Existing valid records retain their exact generation;
no ID is reminted during restart or adoption. Exact-id lookup also detects
conflicting existing generations or multiple active records rather than selecting
one by enumeration order.

New generation allocation checks for a conflicting current or archived identity
in that conversation before publication. An archive name collision must match
exact generation and retired record; otherwise fail as corrupt, never overwrite.
The fixed encoding also makes filenames independent of legacy generation syntax.
The identity constructor owns this encoding; adapters consume it rather than
reimplementing it.

Artifact scope consumes the reusable receiver, source, conversation, artifact
storage incarnation, schema and current access epoch. Native/auth producers must
publish the concrete source/receiver/epoch mapping before wire activation.
Attachment storage incarnation is a separate durable storage lifetime, not an
alias for a transcript incarnation. Its initial durable publication and restore
reset are queued with the recovery/integration owner. Local source results carry
registration and content facts; they do not invent a receiver or authorization
scope.

## One durable owner, two locations for one retired record

Keep the existing primary hold path:

```text
attachments/
  blobs/<stored digest>
  incoming/<private transfer temporary>
  holds/<organization hash>/<conversation>/
    <stored digest>-<media type tag>.json       Pending, Kept, or Retired
    retired-<artifact id>.json                Retired only
```

The primary record remains the sole active hold authority. Add a Retired variant
in the same canonical saved-record codec. Pending and Kept retain their current
saved fields and state spelling; Retired carries the original hold/generation,
the prior Pending/Kept state and exhaustive retirement evidence: an explicit release carries validated initiator,
cause, correlation and requested time; an upload reversal carries its actual
RevertCause and original verified hold caller/correlation. Revision is derived from state, not an independent
integer that can disagree. State-specific decoding is exhaustive, without aliases
or a fallback legacy decoder. Existing valid Pending/Kept records are therefore
already canonical records, not converted through a compatibility shim.

Primary names start with the stored digest; archive names start with `retired-`.
These name forms are disjoint. The archive is completed release metadata. It contains no Live
record, active boolean, byte owner or independently maintained active index.
Lookup incrementally scans current primary records, retaining one bounded decode
and exact match rather than a collection of every hold. It completes corruption
and conflicting-identity checks before returning Live. The number of scanned
files and its runtime are separate from encoded manifest size; no bounded scan
latency is claimed. Lookup examines current primary records for an exact generation-derived identity,
and looks directly at the identity-named tombstone archive file. No digest-only request reaches bytes.
An archived non-Retired record is corrupt and cannot grant content.

### Retirement publication

Under the existing change lock, replace each Pending/Kept primary record with its
Retired variant, using private temporary file, file sync, atomic replacement
and parent-directory sync. The acknowledged source transition is this durable
record publication. Only then may that record stop retaining its blob. Purge
still computes retention over all conversations; an unreadable primary record
retains its filename's digest. A valid Retired primary record does not retain
bytes. Tombstone metadata does not prevent another conversation from retaining
its own hold of the same digest.

The application passes its already validated `ReleaseEvidence` into explicit
release, and passes actual `RevertCause` into take-back. Owned discard of Pending
and Kept both publishes Retired; Pending had no prior Live content. A late discard
of Retired returns NotMine. Private failed-pending-write cleanup can remove only
a still-Pending exact generation, never Kept or Retired. `ReleaseReport` keeps actual hold/blob outcomes; existing audit sink
failure does not resurrect the hold. Durable Retired metadata retains the
transition's cause and initiator when the independent audit write fails. This is
state evidence, not another audit dispatch queue: no automatic audit replay or
exactly-once audit claim is introduced. Blob removal remains a separate outcome
in the existing report/audit. A crash can leave extra unheld bytes; it cannot make
a Retired registration Live. Recovery must not claim blob removal without the
actual removal result.

If replacement or directory sync fails, report storage failure and do not grant
bytes from a possibly observed Retired state. Reload the authoritative file on
the next operation. A file-sync success followed by directory-sync failure is an
uncertain durable transition, not proof that release did not happen. A source read must sync the authoritative record directory before returning a
manifest derived from a state whose publication may be uncertain. A sync failure
is Unavailable. This prevents advertising Deleted before it is durable and then
advertising the preceding Live state after restart. Purge waits for confirmed
publication. This can retain extra bytes.

### Reupload and archive publication

A repeated upload while the primary record is Kept returns that existing hold and
registration. Its original creation audit/caller is preserved. A primary Pending
record can be superseded under existing claim semantics; nothing advertised that
unconfirmed identity as Live. Confirmation additionally checks that the proposed
claim generation has not already been released at either canonical name form. A
late claim from before release cannot take over a later Pending record, even
though current confirmation permits Pending takeover. It returns Gone without
changing the later Pending record. This is necessary to prevent resurrecting a
retained Deleted identity; generation allocation checks alone are insufficient.

Before replacing a Retired primary record, atomically rename that exact Retired
file to `retired-<artifact id>.json` in the **same conversation directory**
while holding the same change lock, then sync that directory. The record's contents do not change
in this move. If the destination already exists, require exact identity/evidence
agreement; a disagreeing record is corruption. After confirmed archive publication,
publish a new Pending record with a fresh generation through the existing upload
path. Confirmation/audit/ownership recheck retains its current order.

A crash during archive movement leaves the same Retired state at the old or new
location. It grants no bytes in either location. If durability is uncertain, do
not publish the new Pending record. After reopen, exact-id lookup examines both name forms; a same-id duplicate
must agree and is still Retired, not a second active owner. Startup clears
private temporary files without decoding authoritative records.
A new Pending record cannot be persisted until the old tombstone's archive
publication is confirmed. This ordering replaces a cross-file transaction with
one durable release followed by an identity-preserving move and then creation;
no independently writable active manifest exists. A reupload retry also syncs the
conversation directory before publishing Pending when the primary is absent: an
earlier uncertain rename may have left its tombstone visible only in the archive.

### Adoption and recovery

Existing Kept records expose revision1 using the saved immutable generation and
exact stored attachment, once the source API is active. Existing Pending records
remain invisible and retain bytes. No startup live registration journal is
created, no provider is opened and no old audit is interpreted as permission.
Retired records retain revision2 across restart and later access-epoch resets.
Corrupt/unreadable primary records preserve their current conservative physical
retention; corruption is typed unavailable, not synthetic deletion or permission.
Archive corruption refuses that identity and reports failure. Temporary files are
cleared using the existing startup policy; authoritative records are not inferred
from temporary names. Actual crash tests must establish supported filesystem
replacement/move durability; this design is not platform proof.

## Source consumer and API

The first real consumer is a Nessa artifact source adapter for the existing held
content, substitutable in tests. It consumes an attachment application port,
`AttachmentArtifacts`, with local Nessa-owned identities/results:

- `manifest(organization, conversation, artifact_id)` returns Live revision1 with
  stored identity, Deleted revision2, Missing or typed Unavailable.
- `chunk(organization, conversation, expected_live_manifest, offset, max_bytes)`
  returns the exact bounded requested range or typed Deleted, Changed, Missing,
  InvalidRequest, Busy, Unavailable or WorkerPanicked. `ArtifactRange` owns the exact immutable expected id/revision/stored attachment and range; `ArtifactBytes` exposes only borrowed bounded bytes. A caller cannot grow its owned byte buffer through public fields.
- The lifetime owner exposes admission fencing and actual `shutdown` completion;
  callers do not infer physical release from typed request failure.

A future protected protocol adapter will map these local facts to reusable
ManifestSource/ChunkSource validation and outcomes after actual admission. This
local port consumes the reusable published chunk bound; it neither implements
those authorized scope ports nor constructs a receiving-device scope. It does
not independently decide permissions. The
existing current auth/conversation owner supplies verified admission per operation
before any future product method calls this source. A manifest is not a grant.
The first local consumer does not manufacture authenticated network access or
invent an authorization snapshot API before the native producer publishes it.

For chunks, while ordered against release under the store change lock, find the
exact Kept registration/generation/content, validate the expected identity and
open its private blob. Read only the requested range outside the change lock.
Consume core MAX_CHUNK_BYTES rather than copying the bound. An already admitted
open file can outlive unlink; new requests after release are refused. The expected
manifest must match revision/content before opening. Missing bytes are unavailable,
not a fabricated tombstone. Whole64MiB reads followed by truncation are excluded.

Physical range work must use the existing tracked read-worker implementation,
now owned by `core::read_workers`, consumed from reviewed producer `51bfcc01`. `LocalAttachmentStore` owns one source admission semaphore shared by manifest and chunk. `ReadWorkers` alone owns closure tracking, sticky worker fault and retained drain; no source closed/failure ledger is added. A nonwaiting slot permit travels with the closure and its result wrapper until actual work and result handoff/drop complete. The owning
worker retains the OS handle and source result through actual join, including
caller loss and result-drop panic. Composition's admission fence/drain-before-store
cleanup remains queued integration. The source adapter cannot claim complete
shutdown before that composition wiring is active. Initial physical range admission is one worker per source instance, with typed Busy
and no waiter queue. That capacity remains held through actual OS read and result
completion even if the caller cancels or its deadline expires. It is distinct from
socket delivery and global record permits, and is not a whole-gateway heap bound.
The extraction is consumed; local range checks remain in progress. Network capacity, socket
permit and delivery policy are likewise integration-owned; no artifact-specific
second socket ledger or guessed gateway capacity is added here.

## Required feature state/order evidence

This table is the target contract. Current tests cover local persistence, manifest
facts and audit mapping. Local physical chunk and source drain evidence is being implemented against the shared worker owner. Protected transport and composed drain rows remain queued until their actual owners are integrated.

| Row | Event ordering | Source result and durable owner | Regression to implement |
| --- | --- | --- | --- |
| A1 | Existing Kept record adopted, restart, same upload retried | Same saved generation, exact stored bytes, Live1 | Preexisting-record fixture and repeated reopen/retry |
| A2 | Pending takeover before confirmation | Only final confirmed generation visible; previous claim cannot undo it | Two uploads with creation audit/confirmation interleaved |
| A3 | Creation audit fails / ownership becomes Deleted | No Live publication; take back only owned pending claim | Actual service/store failure path |
| A4 | Kept release succeeds before manifest/chunk admission | Retained Deleted2, no new blob open | Local source through actual release |
| A5 | Release replacement succeeds, directory sync fails | Uncertain release; source syncs directory before manifest or returns Unavailable; no purge inferred | Inject failure plus source-read/restart boundaries |
| A6 | Release state publishes, independent audit fails | Deleted remains; durable cause/initiator retained; audit failure reported | Independent audit/store outcomes |
| A7 | Shared digest under two conversations, one releases | Only its registration Deleted; other hold protects bytes | Cross-conversation source isolation |
| A8 | Retired primary archived, crash before new Pending | Old identity remains Deleted; no current Live | Restart at same-directory move/directory-sync boundaries |
| A9 | Reupload after confirmed archive | New generation/id; old remains Deleted | Exact same bytes/media after release |
| A10 | Archive destination exists | Exact retired duplicate is idempotent; conflicting generation/evidence fails | Collision and duplicate archive fixtures |
| A11 | Corrupt primary / corrupt archive | Primary protects bytes; neither grants content or invents Deleted | Corrupt codec/path fixtures |
| A12 | Chunk open admits before release, OS read held | Admitted read may finish; new reads refused; worker/handle remains until join | Real held source + cancelled waiter + release/drain |
| A13 | Cancelled shutdown waiter / source panic / result-drop panic | Same actual drain outcome retained; later waiter cannot report false success | Existing worker owner contract consumed by source |
| A14 | Caller requests wrong content/revision/id/range | Typed refusal before byte read | Actual source API boundary cases |
| A15 | Blob missing or changes/corrupts outside owner | Unavailable or downstream hash mismatch; no verified content claim | Missing/truncated/corrupt stored-byte fixtures |
| A16 | Old audited confirmation arrives after release and a new Pending upload | Retired claim cannot confirm/take over later lifetime; new Pending untouched | Real service/store late-confirm ordering |
| A17 | Late discard_generation or discard after release | Retired primary/archive remains; cleanup reports NotMine | Exact old claim cleanup after release and after reupload |
| A18 | Live manifest exposed, then matching Kept claim discarded during confirmation rollback | Retain Deleted with actual revert cause/hold initiator; do not erase observed lifetime | Manifest/late take-back through actual service/store |
| A19 | Retirement publishes, then blob cleanup fails; independent audit succeeds/fails | Typed retirement-success/cleanup-incomplete; actual HoldReverted attempt; preserve audit result and first retirement evidence | Real-store removal failure plus application recorded/unavailable mappings |
| A20 | Pending or Kept rollback retires; cleanup completes/fails and independent audit succeeds/fails | Domain RetiredFrom constrains discard, saved retirement and HoldReverted to actual Pending/Held predecessors; its sole HoldState conversion preserves pending/held to absent | Two outcome compile-fail examples exclude Absent; substitute 2×2×2 prior-state/cleanup/audit matrix; real-store result and audit before/after regressions; matched conversion-owner mutation |
| A21 | Correct generation supplied with another hold description | Store refuses retirement; audit cannot describe a different caller/content than stored owner | Exact generation/wrong-hold local port regression |
| A22 | Archive rename failed its sync, then reupload retries in same process with primary absent | Confirm conversation-directory durability before any fresh Pending publication; failure creates no Pending | Actual rename-failure plus pending-directory-sync retry fixture |
| A23 | Duplicate keys, explicit null retirement fields, or state/evidence disagreement in saved input | One typed codec rejects ambiguous/contradictory records; no collapsed-field acceptance or alternate decoder | Top-level/nested duplicate and presence-state fixtures |
| A24 | One physical manifest or chunk worker remains held after caller cancellation | Both source methods return Busy without queue; shutdown fences through the sole worker owner and retains actual joins | Held source/cancelled observer/combined admission tests |
| A25 | Record read completes, then its directory locator disappears before publication acknowledgement | Actual directory-sync refusal prevents manifest/range facts publication; no Live inferred from a previously read record | Real directory relocation at the source publication boundary |

## Later activation and limits

Produced-file registration requires an explicit trusted capture/export owner; tool
paths and copied transcript references do not supply one. Receiver cache capacities,
bounded verified consumption, gateway/network capacity, descriptor discovery, native wire
methods and generated codecs are later contracts, not implemented guarantees.
Artifact work uses one reusable transfer quantum at a time and yields to foreground
transcript scheduling after the currently admitted physical quantum completes.

The canonical native pairing design already approves a measured revised finite
composition ceiling. Remaining acceptance work is actual allocation/lifetime and
platform proof; it is not a decision to preserve the former128KiB whole-owner
target. Wire sizing still consumes published bounds and measures worst-valid
encoding/retained allocations before paired activation.

A held-upload source slice does not close #273. Completion includes actual trusted
produced-artifact discovery, protected paired verified fetch/resume, revocation and
deletion refusal, honest sleeping-source metadata and transfer priority/data-cost
evidence against active transcript work.
