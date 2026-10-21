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
open ──▶ anchor the root, hold that same lock, stream one deferred conversation
            └── store the match, then publish, then name that publication, then store available
```

Arrows are the order one call runs. The live path is not created, opened for
write, or renamed until accept. A failed stage deletes its partial directory.
Fetch writes `sessions/<id>.sqlite3` and the publication beside it, under the
anchored root, and only by renaming a temp file after the check.

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

## Who owns the name

The audit identity of a path is the anchored path, and only when that path
is Unicode. A caller spelling, including an alias such as `other/../live`,
is not an identity. Stage, accept, and fetch anchor before the decision
line. A path that cannot be anchored is refused under its Unicode caller
spelling. A path that is not Unicode is refused with no audit line:
`Path::display` would replace those bytes, and two roots could share one line.

A published name is an exclusive rename. An empty directory at the
destination is occupied. The source stays. A leftover name is either absent
or recorded retained, and it is not acceptable. A `.partial` session file
is not a helper. A lease and its receipt are percent-encoded, so a space in
either value is not the separator.

A fetched session has a second identity: the publication written beside the
session file in that same publish. An `available` effect names that
publication. An earlier line does not make a different file available. Every
fetch line for one open uses the anchored root and the conversation id as
its target. The publication id comes from the publication-id port. The OS
adapter reads sixteen random bytes. A test supplies the id, or a failure.

A staged quarantine has a third identity: the digest of the journal bytes
stage wrote, stored as the attestations of the sealed effect. Accept trusts
that line. The readiness marker records that the line was stored. It is not
the seal. A journal rewritten so its own file hashes match is still a
different digest.

| What happened | Who owns the bytes | Audit | Caller is told |
| --- | --- | --- | --- |
| Rename not done | The quarantine, when it still exists. The live path is unchanged | `unchanged → unchanged` | The error |
| Rename done, a parent sync failed | The live path | `verified → live`, cause `unsynced` | Live |
| Rename done, sync succeeded, live audit stored | The live path | `verified → live`, cause `accepted` | Live |
| Rename done, live audit not stored | The live path | The live line is missing | Audit failure. The tree stays live |
| Fetch publish not done | No session file | The refusal, or the match line only | The error |
| File and publication stored, and the available line names that publication | That publication | `held → available`. Attestations are the publication. Target is the anchored root and the conversation | The session |
| Available line not stored, and the file was removed or emptied | No readable publication | The available line is absent | Audit |
| Available line not stored, and the file is still readable | The file. Not a publication the audit named | No available line for this publication | Retained. A later open does not serve it |
| Open through another spelling of the same root | The publication already stored for that anchored root | The same target as the first open | Already present |
| Fetched in quarantine, then the folder is renamed to live | The same publication, now at the live path | The available line still names that publication and conversation. The root on the line is where the fetch ran | Already present. The file is not fetched again |
| A session extra whose publication has no available line | The quarantine. Accept does not rename | `unchanged → unchanged` | The error. The live path is not created |
| Two roots that differ only by ASCII case on Windows or macOS | One filesystem object | Refused before either lock is taken | Paths |
| Publish fails after the bytes may already have the final name | Neither name. The final name is retired even when retiring the temp name fails | The publish error when both retire. Retained when either name cannot | The error. A file that remains is retained |
| Publication id | The id from the publication-id port. The OS adapter reads 16 random bytes | Attestations are that id | Entropy failure retires the published file |
| Sealed journal | The audit. Attestations are the digest of the journal bytes stage wrote | `absent → verified`, cause `sealed` | |
| Journal replaced and recomputed so its own seal matches | The quarantine. Accept does not rename | `unchanged → unchanged` | Tamper. The audit still names the journal stage wrote |
| Derived partial directory is the live path, including an ASCII case alias on Windows or macOS | The live path. The partial directory is not deleted | `unchanged → unchanged` | Paths |
| One root's lock file is the other root | Neither root is created as a lock file | `unchanged → unchanged` | Paths |
| A file whose name ends in `.publication` outside `sessions/<id>.sqlite3` | The quarantine. Accept does not rename | `unchanged → unchanged` | Tamper. The file is part of the seal |
| Readiness bytes synced, then renamed to `accept-ready`, then the quarantine directory synced | The marker. A file sync failure deletes the temporary file. A directory sync failure clears the marker | The sealed line stays. The marker is absent | The error. A partial file is not readiness |
| Empty directory at the live path when the rename runs | The quarantine. The empty directory is unchanged | `unchanged → unchanged` | LiveOccupied |
| A root that is not Unicode | Neither path is audited under a lossy spelling | No line | Paths |
| Fetch wrote a partial session and that name is gone | No partial file | `unchanged → unchanged` with the source cause | The source error |
| Fetch wrote a partial session and that name remains | The partial file. Not a helper | No unchanged line | Retained. A later open treats the file as an extra |
| Staging write failed and the partial directory is gone | No partial directory | `unchanged → unchanged` | The write error |
| Staging write failed and the partial directory remains | The partial directory | `absent → unready`, cause `retained` | Retained |
| Lease `a b` with receipt `c`, and lease `a` with receipt `b c` | Two attestations | `a%20b c` and `a b%20c` | Distinct lines |
| Quarantine rename whose parent sync fails, and the tree is removed | No quarantine | `unchanged → unchanged` | Unreadable |
| Quarantine rename whose parent sync fails, and the tree remains | The quarantine, unready | `absent → unready`, cause `retained` | Retained. Not an unchanged refusal |

## Invariants

- The quarantine path and the live path are different absolute paths, and
  neither is a prefix of the other, after `.` and `..` are removed and an
  existing ancestor is resolved. `a_restore_refuses_to_use_live_data_as_quarantine`,
  `paths_and_receipts_are_constrained`
- A failure before the rename does not create the live path and does not
  modify a sentinel beside it. `a_tampered_bundle_does_not_touch_live_data`,
  `a_partial_copy_removes_quarantine_and_leaves_live_data`
- Accept refuses when the live path already exists, and leaves those bytes
  as they were. The rename itself refuses an empty directory, with no
  separate existence check that a created directory could slip past.
  `an_occupied_live_root_is_not_replaced`,
  `an_empty_live_directory_is_not_replaced`
- The seal is the set of file hashes in the journal, and accept requires the
  sealed audit line to name the digest of those journal bytes. A changed
  byte, a missing file, an unexpected file outside `sessions/`, or a journal
  rewritten to match a replaced file refuses accept.
  `a_tampered_quarantine_is_not_promoted`,
  `a_resealed_quarantine_is_not_promoted`
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
  aborts the stage. A worker fence and an environment-peer fence name the
  same stored credential. A grant whose `policyRevision` is newer than the
  fenced epoch is the reissued grant and is kept. `a_revoked_grant_is_absent_from_quarantine`,
  `a_worker_fence_omits_the_environment_peer_credential`,
  `a_reissued_grant_is_kept_and_the_revoked_epoch_is_not`
- A fenced conversation that still sits in a canonical or notes session
  database is deleted from the restored image, and the image is vacuumed.
  A session image that cannot be checked is not copied.
  `a_fenced_conversation_inside_the_session_is_not_restored`
- Stage, accept, and fetch take one lock on the restore root and hold it
  across publication and the audit that records it. Accept also holds the
  live path's lock, so a fetch of the live path waits through the rename.
  `accept_and_fetch_wait_for_the_restore_root`,
  `accept_holds_the_live_lock_across_promotion`
- A quarantine is acceptable only after the sealed audit is stored. The
  marker is a file inside that quarantine, not a sibling, and it is not part
  of the seal. A stage whose cleanup cannot delete the tree is still refused
  by accept. A failed readiness mark records whether that tree was removed
  or left unready. A finish that finds the quarantine already occupied does
  not delete it. `a_failed_stage_cleanup_still_refuses_acceptance`,
  `a_failed_readiness_mark_records_whether_the_tree_remains`,
  `a_quarantine_owned_by_someone_else_is_not_deleted`
- A fetched session is checked by the layout, not by the source that wrote
  it. The match is stored before the session name exists. The session is
  available only when the effect line names the publication stored beside
  that file. An earlier publication's line does not count. If the line for
  this publication is not stored, the file is removed, moved aside, or
  emptied. Emptying truncates the file directly: the storage opener refuses
  a copy it did not create. If none of those three leaves the file
  unreadable, the result is retained and a later open does not serve it.
  `a_failed_fetch_fallback_empties_the_session`,
  `a_failed_fetch_that_cannot_be_retired_is_not_served`
- A session body that cannot be inspected is not sealed while a conversation
  fence applies. `an_unreadable_session_body_is_not_sealed_when_a_conversation_is_fenced`
- A refusal whose audit line is not stored is an audit failure.
  `a_lost_refusal_audit_is_not_a_plain_refusal`
- Each entry point evaluates its own Cedar action. A caller-supplied action
  does not choose the policy. `a_caller_supplied_action_does_not_choose_the_policy`
- Accept syncs the live parent and, when it differs, the quarantine parent.
  A sync that fails after the rename leaves the tree live and records
  `unsynced`. The rename is not undone.
  `a_promoted_tree_stays_live_when_the_parent_sync_fails`
- A lease line `granted` or `command_granted` without a later `ended` gains
  one `ended` line with uncertain cleanup. Accept then requires an operator
  receipt for each such lease. No receipt leaves the tree quarantined.
  `an_open_lease_is_interrupted_in_quarantine`,
  `an_unfenced_executor_blocks_acceptance`
- Deferred lines are not payload. Opening one publishes a session file only
  when the written stream's size and fingerprint equal the sealed line.
  The fetch audit names the anchored root. The available line is stored
  after the publication is written beside that file. If that line is not
  stored, the session file is retired and the open does not succeed.
  `an_opened_deferred_conversation_matches_the_recorded_fingerprint`,
  `a_failed_fetch_audit_removes_the_session`
- Source missing, source changed, and an interrupted read delete the temp
  file and do not modify another conversation's session file.
  `a_changed_source_is_refused_and_other_conversations_stay`,
  `a_missing_source_is_refused_and_other_conversations_stay`,
  `an_interrupted_fetch_leaves_no_partial_session`
- A conversation that is not deferred does not read the source.
  `an_unknown_conversation_is_not_fetched`
- A session file whose publication is the one the audit recorded, and whose
  fingerprint matches, is not fetched again. Another spelling of that same
  anchored root is the same identity, including when the source is later
  missing. One that exists and does not match is left in place.
  `a_matching_session_is_not_fetched_again`,
  `a_different_session_file_is_not_replaced`,
  `an_alias_of_the_live_root_reuses_the_fetched_session`
- A fetch recorded against the quarantine folder is still that publication
  after accept renames the folder onto the live path. Accept refuses a
  session extra whose publication has no available line, and does not rename.
  `a_fetch_in_quarantine_stays_available_after_accept`,
  `an_unaudited_session_is_not_promoted`
- On Windows, `Restore` and `restore` are one root. The refusal happens
  before either lock is taken. `paths_and_receipts_are_constrained`
- A realistic session whose database is over 64 MiB is deferred by the
  export, then recovered by this open. `a_session_over_64_mib_is_restored_when_opened`
- A failed publish retires the temp name and the final name independently.
  Retiring an absent temp still retires the final name.
  `a_failed_publish_retires_the_final_name_when_the_temp_is_already_gone`
- The publication id is the one the port returns. When the port fails, the
  published file is retired and the open does not succeed.
  `a_port_supplied_publication_id_is_the_one_the_audit_names`,
  `a_failed_publication_id_retires_the_published_session`
- A derived name uses the same path identity as a root. The partial directory
  is not deleted when it is the live path. A root's lock file is not the
  other root. A helper suffix is a sidecar only beside `sessions/<id>.sqlite3`
  for a deferred conversation. A `.partial` session file is an extra. The
  readiness marker is the final name after its bytes are synced, and the
  quarantine directory is synced after that rename. A failed directory sync
  clears the marker. `a_lock_file_is_not_the_live_path`,
  `an_injected_publication_suffix_is_not_promoted`,
  `a_readiness_directory_sync_failure_clears_the_marker`,
  `paths_and_receipts_are_constrained`
- A path that is not Unicode is not an audit identity.
  `a_non_unicode_root_is_not_audited`, `paths_and_receipts_are_constrained`
- An interrupted fetch that cannot remove its partial session returns
  retained and does not record unchanged. A later open refuses that file.
  `an_interrupted_fetch_that_cannot_be_removed_is_retained`,
  `an_interrupted_fetch_leaves_no_partial_session`
- A staging write whose partial directory cannot be removed records
  `absent → unready` and returns retained.
  `a_partial_directory_that_survives_a_failed_write_is_unready`,
  `a_discarded_partial_directory_is_removed`
- A lease and its receipt are percent-encoded. `a b` with receipt `c` is
  not the same attestation as lease `a` with receipt `b c`.
  `paths_and_receipts_are_constrained`

## Decisions that bound this

| Question | Decision |
| --- | --- |
| Where is the mac key? | The operator supplies the same key the verifier uses. The bundle does not contain it. Without the key, restore does not start |
| What is fresh? | The cut's inventory, or a later file sealed with that key whose fence keys are a superset. This is not a clock of every revocation in the world |
| Which identity returns? | None. `identity.txt` says `re-pair`. Peers are not told that this process is the old gateway |
| What fences a remote executor? | Nothing on the network. An open lease without an operator receipt blocks accept. The receipt is audited; it is not a claim that we reached the host |
| Which omissions block accept? | A public body whose fingerprint does not match, an unreadable grant or lease line, or a failed seal. Deferred, redacted, and named skips do not |
