# Quarantined restore and deferred open

The export cut stays the backup ([export cut](export-cut.md)). This document
is the restore that follows it (#270, #854). `nessa-backup-plan` does not
call it and still cannot activate a gateway.

```text
operator ──▶ Cedar (stage | accept | fetch)
               │
               ├── audit the decision
               └── effect, or stop when the audit was not stored
stage ──▶ verify the bundle and keep that authenticated listing
            ├── omit fenced grants, credentials, and live metadata rows
            ├── delete a fenced conversation from a session database, then vacuum
            ├── append an uncertain end on each still-open lease
            └── seal the quarantine directory
accept ──▶ hold the restore-root lock, seal still matches, live path absent
            └── rename quarantine → live, then store the audit, then release
open ──▶ hold that same lock, stream one deferred conversation into a temp file
            └── publish only when size and fingerprint match, then store the audit
```

Arrows are the order one call runs. The live path is not created, opened for
write, or renamed until accept. A failed stage deletes its partial directory.
Fetch writes only `sessions/<id>.sqlite3` under the root it was given, and
only by renaming a temp file after the check.

## States

| State | Where the bytes are | What a later start may do |
| --- | --- | --- |
| Absent | No quarantine directory | Nothing was restored |
| Partial | A sibling `*.partial-*` directory, no verified journal | Not a restore. Dropped on failure |
| Verified | The quarantine path, journal phase `verified`, seal matches | Reads and a deferred fetch. No replay. Not the live root |
| Live | The live path holds that same tree after the rename | A later start may issue new work. Replay stays blocked. This use case still starts nothing |
| Refused | The previous state, plus an audit line | Live bytes unchanged |

Live is the path, not a second flag written after the rename. A crash after
the rename and before a following audit leaves the tree at the live path;
the result is a visible audit failure, not a second rename back.

## Invariants

- The quarantine path and the live path are different absolute paths, and
  neither is a prefix of the other, after `.` and `..` are removed and an
  existing ancestor is resolved. `a_restore_refuses_to_use_live_data_as_quarantine`,
  `paths_and_receipts_are_constrained`
- A failure before the rename does not create the live path and does not
  modify a sentinel beside it. `a_tampered_bundle_does_not_touch_live_data`,
  `a_partial_copy_removes_quarantine_and_leaves_live_data`
- Accept refuses when the live path already exists, and leaves those bytes
  as they were. `an_occupied_live_root_is_not_replaced`
- The seal is the set of file hashes in the journal. A changed byte, a
  missing file, or an unexpected file outside `sessions/` refuses accept.
  `a_tampered_quarantine_is_not_promoted`
- A session file published by fetch is allowed only for a deferred
  conversation id, and only at `sessions/<id>.sqlite3`.
- Historical outbox bytes stay, and `replay.blocked` stays in the seal, so
  acceptance does not mean those commands run.
  `quarantine_does_not_permit_replay`
- Quarantine is built from the listing whose mac `authenticated_bundle` just
  checked. A later manifest read is not a second source.
  `a_replaced_manifest_is_not_what_quarantine_seals`
- Acceptance records each open lease and the operator receipt that covered
  it, on the accepted audit line, before success.
  `an_unfenced_executor_blocks_acceptance`
- The deletion-inventory body is checked against the authenticated fingerprint
  before its fences are applied. `a_changed_inventory_body_is_not_a_fence`
- Lease lines are one ledger in line order. A grant closed by a later line
  does not stay open. `a_closed_lease_across_lines_needs_no_receipt`
- Fetch checks the journal seal before it trusts `deferred.txt`.
  `a_changed_deferred_line_is_not_fetched`
- If the sealed audit is not stored and the quarantine cannot be removed, the
  journal is removed and the tree is not recorded as absent.
  `a_failed_cleanup_removes_the_journal`
- A fetch audit names the conversation as well as the root.
  `an_opened_deferred_conversation_matches_the_recorded_fingerprint`
- Restore audit lines share one directory lock for the sequence, the read,
  and the replacement. Two adapters cannot each replace `restore.jsonl`.
  `a_restore_audit_waits_for_the_directory_lock`
- Cedar decides stage, accept, and fetch. A device, peer, environment peer,
  or worker is forbidden. The decision is audited before the effect.
  `a_device_may_not_stage_a_restore`, `a_failed_decision_audit_does_not_stage`
- An audit failure of the effect deletes a quarantine that was not renamed,
  and does not report success. `a_failed_outcome_audit_discards_the_quarantine`
- The deletion inventory in the cut is the known fence set. A later inventory
  must authenticate with the same mac key and must still contain every fence
  key from the cut. A missing key is a rollback and stages nothing.
  `an_older_inventory_is_refused`, `a_newer_fence_omits_the_grant`
- A share grant, credential, or live metadata row named by a fence is not
  copied. A tombstone is. A row of those stores that cannot be checked
  aborts the stage. `a_revoked_grant_is_absent_from_quarantine`
- A fenced conversation that still sits in a canonical or notes session
  database is deleted from the restored image, and the image is vacuumed.
  A session image that cannot be checked is not copied.
  `a_fenced_conversation_inside_the_session_is_not_restored`
- Stage, accept, and fetch take one lock on the restore root and hold it
  across publication and the audit that records it. A refusal whose audit
  line is not stored is an audit failure.
  `accept_and_fetch_wait_for_the_restore_root`,
  `a_lost_refusal_audit_is_not_a_plain_refusal`
- A lease line `granted` or `command_granted` without a later `ended` gains
  one `ended` line with uncertain cleanup. Accept then requires an operator
  receipt for each such lease. No receipt leaves the tree quarantined.
  `an_open_lease_is_interrupted_in_quarantine`,
  `an_unfenced_executor_blocks_acceptance`
- Deferred lines are not payload. Opening one publishes a session file only
  when the written stream's size and fingerprint equal the sealed line.
  The fetched audit is stored after that rename. If that audit is not stored,
  the session file is removed and the open does not succeed.
  `an_opened_deferred_conversation_matches_the_recorded_fingerprint`,
  `a_failed_fetch_audit_removes_the_session`
- Source missing, source changed, and an interrupted read delete the temp
  file and do not modify another conversation's session file.
  `a_changed_source_is_refused_and_other_conversations_stay`,
  `a_missing_source_is_refused_and_other_conversations_stay`,
  `an_interrupted_fetch_leaves_no_partial_session`
- A conversation that is not deferred does not read the source.
  `an_unknown_conversation_is_not_fetched`
- A session file that already matches is not fetched again. One that exists
  and does not match is left in place. `a_matching_session_is_not_fetched_again`,
  `a_different_session_file_is_not_replaced`
- A realistic session whose database is over 64 MiB is deferred by the
  export, then recovered by this open. `a_session_over_64_mib_is_restored_when_opened`

## Decisions that bound this

| Question | Decision |
| --- | --- |
| Where is the mac key? | The operator supplies the same key the verifier uses. The bundle does not contain it. Without the key, restore does not start |
| What is fresh? | The cut's inventory, or a later file sealed with that key whose fence keys are a superset. This is not a clock of every revocation in the world |
| Which identity returns? | None. `identity.txt` says `re-pair`. Peers are not told that this process is the old gateway |
| What fences a remote executor? | Nothing on the network. An open lease without an operator receipt blocks accept. The receipt is audited; it is not a claim that we reached the host |
| Which omissions block accept? | A public body whose fingerprint does not match, an unreadable grant or lease line, or a failed seal. Deferred, redacted, and named skips do not |
