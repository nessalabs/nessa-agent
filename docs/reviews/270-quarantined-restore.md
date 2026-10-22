# Review brief: quarantined restore (#270, #854)

Reviewed before implementation, against the written design only. No restore
code existed at this review. Base is `origin/main` at the branch point.
Head is the design commit on `cursor/quarantined-restore-3cb9`.

## Intended behavior

Stage an authenticated export into a quarantine directory, verify it, and
rename that directory onto an empty live path only after acceptance. A
failure leaves live data untouched. Opening a deferred conversation fetches
that exact stream and publishes it only when the size and fingerprint match.
Audit records are part of each transition. Scope is the backup context.
`nessa-backup-plan` stays inspection-only.

## Owned scope

`crates/nessa-server/src/backup/` (domain, application, infrastructure,
Cedar), the ordering table in `docs/design/quarantined-restore.md`, and ADR
270. Not owned: conversation product handlers, desktop UI, issues #965,
#981, #1038, #1039, #1030, #1034, #345, #337, #746.

## Agreement across fields

The authoritative facts and where they are enforced:

| Fact | Owner |
| --- | --- |
| Bundle authenticity | Existing `verify_bundle` and the supplied mac key |
| Fence set | Cut inventory, replaced only by an authenticated superset |
| Quarantine bytes | Journal seal computed from file hashes |
| Live versus quarantined | The directory path after a successful rename |
| Open leases | Domain fold of `granted` / `command_granted` / `ended` |
| Executor fence | Operator receipt required at accept, audited, not a network call |
| Deferred payload | Sealed fingerprint, recomputed from the session file after fetch |
| Who may restore | Cedar policies on `backup.restore.stage`, `backup.restore.accept`, `backup.deferred.fetch` |

Checked contradictions the design refuses: a successful audit with no
effect, a renamed live tree whose seal was not checked, a fetch that
publishes before the fingerprint matches, a later inventory that drops a
fence the cut has, and an accept that writes into an existing live root.

## Dimensions applied to the design

- Authority: one rename owns promotion. The plan command is not a second
  owner. Cedar is the only allow/deny.
- Domain: phase, seal, fence subset, and lease fold are domain functions.
  JSON parsing stays in infrastructure.
- Admission: decision audit before the effect; effect audit failure rolls
  back a quarantine that has not been renamed.
- Failure: partial directory is dropped; live path is not created.
- Restoration: fetch is a separate publish with its own temp file.
- Audit: target, before, after, cause, initiator, policy ids. A failed sink
  is a failed transition.
- Bounds: source copying streams payload chunks. The Unix held-inode
  verifier subsequently deserializes the entire SQLite image into memory
  (`inspect_open_file`); recovery does not yet have a bounded-memory contract.
- Organization: the work stays in the backup context. No new top-level crate.

## Findings folded into the design before code

1. Importing the environment ledger type would make restore Unix-only. The
   ended line is checked on Unix against that type's JSON, and the generator
   does not import the type.
2. Hashing the whole tree at accept would reject a session file fetch wrote
   while quarantined. The seal lists the files it covers, and `sessions/`
   is the only extra directory accept allows.
3. Marking a lease `Interrupted` in a kind the ledger reader ignores would
   leave it granted. The appended line is `ended` with uncertain cleanup,
   which is what that reader treats as not still running. The audit cause
   remains restore.
4. Giving the plan command a destination would break its inspection
   contract. It is unchanged.

That review covered the design, not the implementation. Implementation
qualification and remaining findings are recorded below.

## Implementation review, 2026-10-21

The review started at `dc49b2f53a3c62939932c0eb7c7f8a131bf52cc4`, against
merge base `cf5c5200818d0e2217e8e0666696e5af61fc358d` (33 changed files).
Independent reviewers inspected application failure and audit recovery,
filesystem races and durability, and domain/API authorization contracts.

The application and adapter corrections are:

- Bind deferred fetch to durable journal authority, including readiness for a
  staged root. Rewriting both `deferred.txt` and `journal.txt` is refused.
  `a_resealed_deferred_line_is_not_fetched_in_quarantine` and
  `a_resealed_deferred_line_is_not_fetched_after_acceptance` both failed before
  the correction by publishing the replacement session.
- Hash the exact deferred metadata read for the operation, rather than trusting
  an earlier directory scan. `a_deferred_line_changed_after_loading_is_not_fetched`
  failed before the correction by publishing the replacement session.
- Use one root-generation fold for acceptance and fetch. A newer live generation
  supersedes an earlier stage at the same path, and a newer stage supersedes an
  earlier live generation. The generation regression failed when live
  supersession was deliberately disabled and passed after restoration.
- Sync nested directory entries after linking the checked files, before the live
  rename. `sealed_names_are_linked_before_the_tree_is_synced` checks that order;
  `a_linked_tree_sync_failure_refuses_publication` checks the typed failure.
- Preserve the live effect when source retirement fails after the rename.
  `a_retained_source_does_not_make_the_live_rename_unchanged` crosses source
  retirement failure with parent-sync success and failure. Both facts are
  preserved as `retained` or `retained-unsynced`; the caller gets `Retained`.

That earlier combined backup suite passed 263 tests on Linux with Rust 1.99.1.
It does not qualify the continuation changes below. Final aggregate checks and
filesystem negative controls are recorded on the PR against their tested tree.

## Continuation contracts, 2026-10-22

The next review identified a reused-path rollback after a live rename whose
completion could not be audited, namespace replacement during fetch cleanup,
the post-link writable-open interval, and the full-memory held-file inspector.
The design table was extended before the corresponding implementation changes.

- A durable live-target acceptance intent precedes promotion and invalidates
  earlier root authority. Its completion and the promoted marker must agree on
  the staging identity and journal digest. The fold fails closed for an orphan,
  conflicting, or missing completion, including after every byte of an older
  tree has been restored. Isolated contract regressions are
  `pending_acceptance_invalidates_an_older_live_generation`,
  `live_authority_requires_intent_digest_and_marker_agreement`, and
  `malformed_or_same_digest_new_intent_does_not_restore_old_authority`.
- Stage readiness has its own completion boundary. A sealed receipt creates
  only a pending stage; marker publication and the retained StageOwner's binding
  check precede the matching ready receipt. A marker left behind by a failed
  readiness operation cannot authorize acceptance or fetch when retirement and
  audit invalidation also fail. Four isolated generation regressions cover
  missing, orphaned, mismatched, superseded, and spent ready receipts. Their
  final execution is recorded with the combined tree.
- `PromotionCause` owns the mapping of three independent post-rename facts:
  parent durability, source retirement, and publication protection release.
  `every_promoted_outcome_retains_its_independent_facts_in_the_receipt` crosses
  all eight combinations and verifies that the receipt reader recognizes the
  same meanings. Lease attestations remain separate from generation identity.
- The deferred source receives the reserved file handle instead of a path.
  Partial and published cleanup retain the owning directory and inode through
  source writing, inspection, publication, sidecar writing, and audit storage.
  A replacement at the original name is not cleanup's target. Uncertain
  retirement returns `Retained` without an unchanged audit claim.
- Cached relative session inspection and publication verification read the held
  file on every platform. Converting its descriptor back to a pathname would
  reopen a lookup race. The regression unlinks the held session, replaces its
  ancestor and original name with a valid different database, and distinguishes
  the retained original digest from the pathname replacement.
- Publication protects each inode before other writers can acquire it: Linux
  keeps it unnamed until linking, while macOS uses an exclusive read-only name
  from birth. It keeps that protection through directory sync and the live rename, and
  releases it afterward. The threat boundary includes namespace replacement
  and ordinary writable opens; trusted ownership, permission metadata, and audit
  storage are stated in the design. A held writable descriptor by itself was
  not protection against a second writable open after linking.
- Retirement keeps exact-file ownership rather than recursively removing an
  old directory path. Failed stage cleanup can leave named empty files and
  directories and returns `Retained`. Successful acceptance empties and syncs
  the captured source readiness marker; the original quarantine's other bytes
  remain unready. On macOS an emptied source scratch sibling can remain outside
  the restored tree after fetch. These are stored leftovers, not claims of
  deletion or automatic disk reclamation.
- The held-file inspection adapter uses positional SQLite file I/O, a configured
  one-MiB page cache, and no memory mapping; it no longer deserializes the whole
  session image. Payload reads use 64-KiB chunks and record heads are visited one
  at a time. Stored metadata is guarded before Rust extraction, generated columns
  and physical-rowid shadows are refused, and integer guards precede stream-key
  comparison. Temporary sorts request files; the held VFS refuses those opens.
  Final validation belongs to the combined tree, not the earlier streaming
  source-copy test. This is a bound on these image-copy, page-cache, payload,
  metadata, and sorter paths, not a universal bound on SQLite's parser or global
  heap for an arbitrary hostile database schema.

These continuation changes require their final combined regressions, negative
controls, and aggregate checks before qualification. A check has not passed
merely because its contract or test is listed here.

Ten isolated continuation negative controls executed on 2026-10-22 failed as
running tests at their intended assertions: premature stage authority, reused
older live authority, availability before release, skipped owned fetch cleanup,
writable linked output, an unsealed namespace, stale-path recursive retirement,
whole-image held-file deserialization, unguarded metadata, and eager record-head
collection. Exact source hashes were restored after each. The deserialization
control ran the 4-GiB sparse fixture in a child capped at a 1-GiB address space;
the bounded inspector had passed that fixture. The later retained-read regression
also passed in the 302-test focused backup suite, failed when its production
helper was changed back to pathname reopening, and had its exact source restored.
These controls do not establish native Windows protection or final aggregate
success; the PR records those separate receipts.

The aggregate process checks need a child reaper. The same compiled binaries
passed under the documented `/usr/local/bin/docker-init -s --` execution context;
the original failures remain failed receipts. The storage checks likewise have
a separate passing receipt in the supported root-UID context outside the UID
namespace. Those diagnoses do not establish a passing final aggregate on the
continuation tree.

## Product scope still open

ADR 270 describes a library slice. The production gateway, desktop, and
`conversation.read` do not invoke stage, acceptance, or deferred fetch. There is
no user flow that lists the restored deferred conversations, starts this fetch
when one is opened, or displays its progress and errors. `identity.txt` and
`replay.blocked` are stored declarations with no runtime consumers; a test that
checks those marker bytes does not prove identity replacement, re-pairing, or
replay suppression in a running gateway. This PR does not complete the product
requirements of #270 or #854, and both issues remain open. Their independent
freshness, authority, identity, and user-facing integration acceptance work is
not waived by the backup primitive tests.

These limits are not waived by a passing unit suite or prior-head CI. No
external review agent was contacted during this review.


The retained-source replay investigation reproduced a second successful acceptance
at another live target when both source-retirement operations failed and the
post-rename source-spend append was lost. The regression
`a_failed_postpromotion_audit_cannot_publish_a_retained_generation_twice` uses the
actual promotion adapter and rejects every append after the live intent. Source
consumption now precedes that intent and all filesystem effects under the root
locks. A consumption-audit failure leaves the source retryable without promotion;
a later intent failure leaves it spent and requires a fresh stage. The shared
root-authority fold's existing staging-id invalidation owns this decision.

## Native qualification and injected-source boundary

Native qualification exposed an obsolete Unix-wide unnamed-file assumption.
Linux retains its unnamed-inode check; macOS protected named copies instead
require exact named inode bindings, held bytes and journal agreement, and no
foreign entries. Source-only scratch outside the restored root is a separate
retirement scope: exact zeroing and sync must succeed before preserving a
primary source refusal. Empty external scratch names can remain. Named
publication candidates and failed zeroing or sync still return `Retained`.

Windows qualification preserves failures rather than treating cross compilation
as native proof. Corrections use the native DACL selector, preserve captured ACL
control bits, and request synchronization access for independent enumeration
cursors. Native assertions retain exact foreign-child ACL comparisons and
actual write, rename, and deletion controls. Fixtures distinguish Windows'
open-descendant directory-rename refusal from successful Unix ancestor swaps.

The public deferred source is not trusted to enforce the published database's
data boundary. A deterministic source returning the exact requested stream plus
another conversation reproduced successful whole-image publication before the
fix. Held inspection now rejects additional stream heads, names and unrelated
record rows. Publication reuses the canonical stream writer with a held source
and fresh protected destination, excluding source-side tables, columns, free
pages and trailing bytes. Separate regressions verify rejection, a one-stream
positive control, and exclusion of stored side-data sentinel bytes.

These are source-review conclusions and regression contracts. The PR records
the passing and failing receipts against each frozen tree, including the final
aggregate and exact-head native CI; this section does not imply pending native
runtime checks passed.
