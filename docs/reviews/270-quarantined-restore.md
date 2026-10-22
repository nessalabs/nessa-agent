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

The combined backup suite passed 263 tests on Linux with Rust 1.99.1. Final
aggregate checks and filesystem negative controls are recorded on the PR.

Remaining limits requiring further work:

- On Unix, `inspect_open_file` allocates a private SQLite image of the whole
  session database. An oversized recovery is not bounded-memory end to end.
- The checked files become writable directory entries during `publish_held`,
  before the sealed directory is renamed live. A same-account writer can open
  such an entry after its link and modify the inode through the rename. Holding
  the original file descriptor does not exclude that writer on Unix. A Linux
  primitive probe using `O_TMPFILE`, `linkat`, a second writable open, and
  `pwrite` demonstrated this property; no full promotion race test was run.
  The internal-sealing guarantee needs a clear threat boundary or a stronger
  publication design; an extra hash check would leave another race.
- Desktop and `conversation.read` do not call these use cases. `identity.txt`
  and `replay.blocked` have no runtime consumers. This PR supplies backup
  primitives, not completion of the user-facing restore and fetch-on-open
  requirements in #270 and #854.
- Journal authority still identifies a live generation by its audited path and
  digest. If a later replacement at a reused live path succeeds but cannot store
  its live audit, the old live record remains the latest durable evidence.
  Rolling all bytes back to that older digest is outside the new rewrite tests;
  the implementation does not establish an independent rollback-proof root
  identity. No complete runtime reproduction of this combined failure was run.

These limits are not waived by a passing unit suite or prior-head CI. No
external review agent was contacted during this review.
