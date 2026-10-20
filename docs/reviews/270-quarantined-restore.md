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
- Bounds: deferred payload is streamed in chunks; it is not a second
  in-memory snapshot.
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

No code finding is open. Implementation follows this table; a row without
the named test is unfinished.
