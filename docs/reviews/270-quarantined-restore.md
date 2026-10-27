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

Windows qualification preserves failures rather than treating cross compilation as native proof. Native f4ce passed the empty-name cursor and exact-restoration controls, but existing-parent assignment still converted inherited child ACEs to explicit protected ACEs. The owned-tree control also had explicit children and did not reproduce its claimed hazard. The resulting structural repair is designed in the [retained-directory publication ordering table](../design/retained-directory-publication.md#windows-session-publication-lifetime-pins): sessions-parent ACL effects are removed, owned publication lifetimes are pinned, and the separate stage/copy initialization model and genuine inherited-child controls require native proof. The former public directory restoration effect API is removed; historical kernel controls remain test-private. Existing actual writer, rename, deletion, identity and audit-failure assertions remain required.

Native `74650438` then failed protected-directory integration 1 passed / 10 failed: NtCreateFile discarded the requested birth AI bit. Its scheduler did not reach the pinning or genuinely inherited-child unit controls, so neither is qualified by that run. The readonly initialization candidate establishes and verifies modern inheritance while the newly owned directory remains readonly. Native a582 proved that phase, but the following raw native append cleared AI despite its input AI bit. The next bounded transition uses SetSecurityInfo on the already-modern original MAX handle, with strict model readback and exact inherited-child controls, including the actual verified-reopen release path. The raw native model-clearing counterexample remains required. Both readonly phases must refuse actual additions and rename-into through original/preacquired authority; matching operations must succeed after the append grant. The documented MAX-handle propagation exception is a candidate requiring native proof for these exact handles and masks; no child-isolation guarantee is inferred from compilation or the input descriptor.

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

Native 0125274a reached and passed readonly-phase and leaf-pinning controls, but verified-reopen provider release changed genuine inherited child ACLs (storage53/3, directory integration11/0). The design now distinguishes qualified append/seal from a separately checked terminal raw-release candidate. Ordinary legacy hard-link and directory-move postures, external-alias preservation and failure cleanup are mandatory controls; no all-children-modern assumption or weaker ACL assertion qualifies release.


## macOS aggregate qualification: desktop stop ordering, 2026-10-22

Exact-head `90747960c8b779e379faec66593132ed40e8ac01` macOS server
qualification failed `a_desktop_stop_stops_the_owner_live_when_it_takes_the_lock`:
provider opens were one rather than two (3,603 passed, one failed, 12 ignored).
The receipt and submission FIFO position establish a new logical owner, but
provider attachment runs separately. This is a test ordering gap: the SDK
permits cancellation before provider open, as exercised by
`close_before_attachment_task_runs_preserves_close_without_failed_transition`.
The failed receipt remains evidence; the following test correction is not yet
qualified on macOS.

| Controlled ordering before the queued stop takes the submission lock | Required evidence |
| --- | --- |
| Person closes the old owner; a queued submission creates its replacement, then waits in its file-link audit while holding the lock; attachment is joined before that audit is released | Existing replacement-owner test retains its receipt, two provider opens, two provider closes, released slot and subsequent usability assertions |
| Same FIFO and held-audit ordering, but replacement runtime readiness stays gated until the stop cancels attachment | New `a_desktop_stop_can_cancel_the_replacement_before_provider_attachment` checks the accepted receipt, one provider open and close, released slot and subsequent usability; no provider is opened merely to close the logical owner |

Both cases use acknowledgements rather than sleeps to establish the attachment
ordering. The existing cooperative-budget/FIFO checks stay in place. A
counterfactual that prevents attachment but retains the first row's two-open
assertion must fail one versus two. A counterfactual stopping the captured old
owner must fail the first row's close or released-slot evidence. These controls
and final aggregate results must be recorded after execution, not inferred from
source inspection.

## Windows Clippy qualification: obsolete test adapters, 2026-10-22

Exact-head `40057ec4b9c11c3aa2396e852f107c11a818cb7c` Windows remaining
tests passed, including the legacy ACL and stored-protection controls. The
following Clippy step failed on four unused helpers. `rename_new`, `unlink_at`
and the `publish_new` import serve only the Unix namespace-sealing test and
are now compiled only for Unix tests. `rename_exclusive_at` serves only the
non-Windows production branch; its unused Windows arm and the corresponding
test adapter were removed. Windows publication continues to rename through
the retained source handle. No assertion, lint severity or public storage API
was changed. Independent caller and platform review confirmed these boundaries;
the corrected tree still requires aggregate and exact-head CI qualification.

The same head's Windows server aggregate reached the guard-cleanup test's
validation assertion and failed: `content_of` returned a fingerprint rather
than `Unreadable` (2,884 passed, one failed, eight ignored). Its ordinary
archive reader intentionally accepts linked regular files. Publication uses
`staged_tree_matches` to require the complete owned manifest before promotion.
The corrected fixture proves empty-manifest acceptance before injection,
ordinary reading of the foreign alias, and actual publication refusal with
`Retained` after sealing. Private-file multilink refusal and every native
security, identity, byte, write and guard-drop assertion remain. A portable
regression separately checks alias refusal and acceptance after its removal;
this correction does not add private-file policy to authenticated archive reads.
