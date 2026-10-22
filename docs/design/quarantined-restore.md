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
directory the anchored root holds. The session name is a new file of the
bytes that matched, not a rename of the temp inode.

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
is Unicode. The anchor keeps the ancestor directory open. Lock, partial
directory, fetch, and promotion use that handle. A caller spelling,
including an alias such as `other/../live`, is not an identity. Stage,
accept, and fetch anchor before the decision line. A path that cannot be
anchored is refused under its Unicode caller spelling. A path that is not Unicode is refused with no audit line:
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
stage wrote, and a staging id from the staging-id port. The sealed effect
stores both. The readiness marker stores that same id. Accept trusts the
latest unspent id for that digest at that quarantine path. A later effect
that names the id spends it, including the accept itself and a readiness
failure. A historical seal, a spent id, or the constant `ready` marker is
not a generation. A journal rewritten so its own file hashes match is still
a different digest. The OS adapter reads sixteen random bytes. A test
supplies the id.

| What happened | Who owns the bytes | Audit | Caller is told |
| --- | --- | --- | --- |
| Rename not done | The quarantine, when it still exists. The live path is unchanged | `unchanged → unchanged` | The error |
| Rename done, the staging id recorded spent, then the live line stored | The live path. That staging id is spent | `verified → spent`, cause `spent`, then `verified → live` | Live |
| Rename done, a parent sync failed, and the id was spent | The live path | `verified → live`, cause `unsynced` | Live |
| Rename done, then source retirement failed | The live path holds the checked copy; the source may remain acceptable until its id is spent | Spend the staging id, then `verified → live`, cause `retained` | Retained. The live rename is preserved and both parent syncs are attempted |
| Rename done, the spend line not stored | The live path. The id may still be unspent | The spend line is missing | Audit. The tree stays live |
| Rename done, live audit not stored | The live path. The id is spent | The live line is missing | Audit failure. The tree stays live |
| Fetch publish not done | No session file | The refusal, or the match line only | The error |
| File and publication stored, and the available line names that publication | That publication | `held → available`. Attestations are the publication. Target is the anchored root and the conversation | The session |
| A newer stage or completed live rename reuses an earlier root path | One audit-generation fold owns both accept and fetch. A new generation replaces the previous authority; spending it leaves no fallback to an older generation | The old staging id and journal digest are refused | Tamper |
| Journal and deferred metadata are rewritten together before open | The bytes no longer name the journal recorded by stage or acceptance | Refused before reading the session source | Tamper |
| Deferred metadata changes after the layout hashes the tree | The second read must still match the journal's deferred-file fingerprint | Refused before reading the session source | Tamper |
| Open on a staged root whose readiness was retired | No current ready staging id, even if its sealed audit remains | Refused before reading the session source | Tamper |
| Live rename completed but source retirement failed | The live directory owns the validated copy; source retirement remains uncertain | The staging id is spent, then `verified → live`, cause `retained` (or `retained-unsynced` when a parent sync also failed), with the journal digest and lease receipts | Retained; no unchanged refusal |
| Available line not stored, and the file was removed or emptied | No readable publication | The available line is absent | Audit |
| Available line not stored, and the file is still readable | The file. Not a publication the audit named | No available line for this publication | Retained. A later open does not serve it |
| Open through another spelling of the same root | The publication already stored for that anchored root | The same target as the first open | Already present |
| Fetched in quarantine, then the folder is renamed to live | The same publication, now at the live path | The available line still names that publication and conversation. The root on the line is where the fetch ran | Already present. The file is not fetched again |
| A session extra whose publication has no available line | The quarantine. Accept does not rename | `unchanged → unchanged` | The error. The live path is not created |
| Two roots that differ only by the case rule of the volume that will hold them | One filesystem object. Linux vfat, exFAT, NTFS, and casefolded ext4 fold. macOS folds and decomposes when the volume does. Windows folds. The drive letter folds with the name | Refused before either lock is taken | Paths |
| Publish fails after the bytes may already have the final name | Neither name. The final name is retired even when retiring the temp name fails | The publish error when both retire. Retained when either name cannot | The error. A file that remains is retained |
| Publication id | The id from the publication-id port. The OS adapter reads 16 random bytes | Attestations are that id | Entropy failure retires the published file |
| Sealed journal | The audit. Attestations are the journal digest and the staging id | `absent → verified`, cause `sealed` | |
| The same journal bytes staged again after that id was spent | A new staging id. The old marker is not this generation | The latest unspent seal. A spent id is not current | Tamper when the marker is the spent id or `ready` |
| Readiness failed and the marker still names the id | The quarantine, unready. The id is spent | `verified → unready` or `verified → absent`. Attestations are the staging id | The error. A later accept does not promote that marker |
| Readiness failed, the tree remains, and the spend line is not stored | The quarantine, without its marker. The id is not spent | The spend line is missing | Audit. A later accept does not promote that tree |
| Readiness failed, the spend line is not stored, and the marker still names the id | The quarantine. The marker is still that id | The spend line is missing | Retained |
| Staging id fails and the published tree is removed | No quarantine | `unchanged → unchanged` | The id error |
| Staging id fails and the published tree remains | The quarantine, unready | `absent → unready`, cause `retained` | Retained. Not an unchanged refusal |
| The quarantine directory is replaced after it was read | The substitute stays at the quarantine path. The verified directory is not renamed | `unchanged → unchanged` | Tamper |
| A sealed file changes after it was read and before the rename | The quarantine. The live path is not created | `unchanged → unchanged` | Tamper |
| Rename finished, then the live path is not that directory, and moving it back clears live | The substitute is not live | `unchanged → unchanged` | Tamper |
| Rename finished, then the live path is not that directory, and it remains | The substitute at the live path | No unchanged line | Retained |
| Journal replaced and recomputed so its own seal matches | The quarantine. Accept does not rename | `unchanged → unchanged` | Tamper. The audit still names the journal stage wrote |
| A partial name that already exists, or that is the live path | The existing directory. This attempt creates a different name and does not delete the old one | The planted directory is not an audit target | The new partial directory |
| One root's lock file is the other root | Neither root is created as a lock file | `unchanged → unchanged` | Paths |
| A file whose name ends in `.publication` outside `sessions/<id>.sqlite3` | The quarantine. Accept does not rename | `unchanged → unchanged` | Tamper. The file is part of the seal |
| Readiness bytes synced, then renamed to `accept-ready`, then the quarantine directory synced | The marker, whose bytes are the staging id. A file sync failure retires the temporary file. A directory sync failure retires the marker | The sealed line stays when retirement succeeds. A marker that still names an id is retained, and the id is spent | The error. A partial file is not readiness. Retained when the id remains |
| Empty directory at the live path when the rename runs | The quarantine. The empty directory is unchanged | `unchanged → unchanged` | LiveOccupied |
| A root that is not Unicode | Neither path is audited under a lossy spelling | No line | Paths |
| Fetch wrote a partial session and that name is a symlink | The symlink. Its target is not created | `unchanged → unchanged`, or retained when the name remains | The error. SQLite opens only a file this attempt created |
| A directory inside the partial tree is a symlink when a file is written there | The symlink. The target is not created | `unchanged → unchanged`, or `absent → unready` when the partial directory remains | Unreadable. The file is created beneath the partial directory |
| `sessions/` is a symlink when the session file is reserved | The symlink. The target is not created | `unchanged → unchanged`, or retained when a partial name remains | Unreadable. The reservation is created beneath that directory |
| The ancestor is renamed and a symlink takes its name after the root is anchored | The held ancestor. The symlink's target is not created | The symlink target is unchanged | The lock and the partial directory are created under the held ancestor |
| A writer still holds the temp session and writes after the matching bytes were copied | The new session file. The temp inode is not that file | `held → available` names the copied bytes | The published file is those bytes. A different byte is not published |
| A writer opens the predictable verified name after the stream matches | That name is not a file. The session name is the inode that was checked | `held → available` names that inode | The published file is those bytes |
| A writer tries to open a file under the predictable sealed directory before its link | Those bytes have no name until they are linked for the rename | No live effect yet | The pre-link open cannot acquire that inode. After the link, same-account mutation before the directory rename remains an open implementation finding (see the implementation review) |
| Sealed files are linked into their nested directories | The new sealed tree, before its live rename | No live effect yet | Each directory is synced after its file names exist; a sync failure removes the copy and refuses promotion |
| The quarantine directory is renamed aside and the original path is gone | The directory the open handle names, removed, including its readiness marker | `verified → spent` when that retirement succeeds | The missing original path is not success |
| mountinfo encodes a space in the mount point | That mount's filesystem, after the octal escape is decoded | One lock order for the two spellings | The case rule of that filesystem, not of `/` |
| Two restores whose roots differ only by case on a folding volume | One lock order, from the folded identity | Each waits for the same first lock | Neither deadlocks on the other spelling |
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
- Accept renames a new directory written from the sealed bytes, not the
  directory that was read. An open descriptor on a quarantined file does not
  change the live file. A different directory placed at the quarantine path
  after that read is not promoted. If the live path is a different directory
  after the rename, it is moved back; when it remains, the result is retained
  and no unchanged line is written.
  `an_open_descriptor_does_not_change_the_promoted_file`,
  `a_swapped_quarantine_directory_is_not_promoted`
- Cleanup removes the directory object this stage published. A different
  directory at that path is left in place, including its files.
  `a_different_published_directory_is_not_removed`
- The sealed copy is the directory this attempt created, in the live
  directory's parent, so the rename stays on the live volume. Its identity is
  that open. The copied bytes stay on the handles that wrote them. On Unix
  they have no directory entry until the link that publication uses. On
  Windows the names exist without write share, and those handles close after
  the check so the directory can be renamed. Immediately before
  that link, those handles still hold the seal, and the directory is still
  the one this attempt created. A replacement directory is not adopted and is
  not deleted. A changed file in the copy is not promoted. A file created at
  the name the copy will use is a different inode.
  `a_changed_sealed_copy_is_not_promoted`,
  `a_replaced_sealed_directory_is_not_published`,
  `the_sealed_copy_is_created_beside_live`,
  `a_held_file_is_not_a_directory_entry_until_publication`
- The sealed tree's file names are linked before its directories are synced.
  The recursive sync includes the nested directories that own those names;
  syncing an empty directory before the links does not persist them. A failed
  sync refuses promotion before the live rename.
  `sealed_names_are_linked_before_the_tree_is_synced`,
  `a_linked_tree_sync_failure_refuses_publication`
- Cleanup deletes the directory the open handle names. A directory renamed
  aside is the one removed. A replacement at the old path stays.
  `a_renamed_directory_is_removed_by_its_handle`
- Promotion retires the quarantine directory it still holds. A missing
  original path is not success. That handle's directory is removed, and so
  is its readiness marker.
  `a_renamed_source_is_retired_through_its_handle`
- A source that is still the directory we read, and whose removal failed,
  loses its readiness marker before promotion succeeds. A different directory
  at that path keeps its marker. A marker that still names an id is retained.
  `a_source_that_remains_loses_its_readiness_marker`,
  `a_remaining_source_is_not_left_acceptable`
- A credential record is restored only when its JSON id is the manifest
  identity. The fence is applied to that id.
  `a_credential_body_must_name_the_manifest_identity`
- A file is created only beneath a directory this attempt holds. That
  directory is a new private directory; an existing path is not adopted.
  A symlink at a parent inside the partial directory is not that directory,
  and the write does not create the file in the link's target. The sealed
  copy uses the same create. `a_symlink_parent_is_not_a_staging_directory`,
  `an_existing_directory_is_not_a_new_private_directory`
- The anchored root keeps the ancestor directory open for the rest of the
  attempt. The lock, the partial directory, the fetch, and promotion run
  beneath that handle. A symlink placed at the old ancestor name is not
  where those effects land.
  `a_renamed_ancestor_is_not_where_later_effects_land`
- The session file is the regular file this attempt still holds when SQLite
  opens it. It is created beneath its parent directory, so a symlink at
  `sessions/` is not that directory and the file is not created in the
  target. The open does not follow a symlink, and the name must still be
  that file. Drop unlinks a planted symlink and does not write its target.
  `a_dangling_symlink_is_not_the_session_file`,
  `a_symlink_planted_after_reserve_is_not_opened`,
  `a_symlink_sessions_directory_is_not_the_reservation`
- The published session is a new file of the bytes whose digest matched.
  It is created beneath the held session directory. Those bytes have no
  directory entry until the check has passed and the session name is linked.
  A write through the temp file's descriptor does not change it. A different
  byte is not given the session name. The read handle closes before the
  partial name is removed, and no verified partial remains.
  `a_write_through_the_temp_inode_does_not_change_the_published_file`,
  `a_different_byte_is_not_published`,
  `a_session_cut_reads_the_unnamed_inode`
- That directory is opened as a directory. On Windows the open sets backup
  semantics, so the handle names the directory. A reparse point is not that
  directory. The volume of the temporary directory is read the same way.
  `case_for_a_reported_filesystem_follows_the_volume`
- A byte changed in the quarantine after that read is not promoted.
  `a_changed_quarantine_file_is_not_promoted`
- A staging id that cannot be minted removes the published tree when it can.
  When the tree remains, the audit says `absent → unready` and the result is
  retained. `a_failed_staging_id_keeps_an_unremoved_tree`
- A partial directory name that already exists is not deleted. The attempt
  creates a different name. `a_preexisting_partial_directory_is_not_deleted`
- The seal is the set of file hashes in the journal. Accept requires the
  latest unspent sealed line for that quarantine path to name the digest of
  those journal bytes and a staging id, and the readiness marker to store
  that same id. A changed byte, a missing file, an unexpected file outside
  `sessions/`, a journal rewritten to match a replaced file, a spent id, or
  the constant `ready` marker refuses accept.
  `a_tampered_quarantine_is_not_promoted`,
  `a_resealed_quarantine_is_not_promoted`,
  `a_spent_staging_id_is_not_promoted`,
  `a_constant_ready_marker_is_not_a_generation`
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
  of the seal. It stores the staging id from that seal. A stage whose cleanup
  cannot delete the tree is still refused by accept. A failed readiness mark
  spends that id and records whether the tree was removed or left unready.
  A marker that still names the id after retirement fails is retained. A
  finish that finds the quarantine already occupied does not delete it.
  `a_failed_stage_cleanup_still_refuses_acceptance`,
  `a_failed_readiness_mark_records_whether_the_tree_remains`,
  `a_marker_that_cannot_be_cleared_is_retained`,
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
- The case rule comes from the volume that will hold the final component, not
  from the host operating system. On that volume, `Restore` and `restore` are
  one root when the volume folds case, and so are `Ä` and `ä`, and so are
  sigma and final sigma. The fold is uppercase, then lowercase. On macOS, a
  composed name and its decomposition are one root when the volume is not
  case-sensitive. Linux vfat, exFAT, NTFS, and casefolded ext4 fold. Two
  paths on different volumes do not fold into each other. The drive letter
  folds with the name. The refusal happens before either lock is taken.
  `paths_and_receipts_are_constrained`,
  `a_folding_volume_orders_spellings_together`,
  `case_for_a_reported_filesystem_follows_the_volume`
- A mount point is compared after mountinfo octal escapes are decoded. A
  space in the mount point is that filesystem's case rule, not the rule of
  `/`. `a_mount_point_with_a_space_is_that_filesystem`
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
  readiness marker is the staging id, written under its final name after its
  bytes are synced, and the quarantine directory is synced after that rename.
  A failed directory sync retires the marker. Retirement that leaves a valid
  id returns retained. A readiness failure whose spend line is not stored
  removes the marker; a marker that still names the id is retained.
  `a_failed_spend_retires_the_readiness_marker`,
  `a_marker_that_survives_a_failed_spend_is_retained`,
  `a_lock_file_is_not_the_live_path`,
  `an_injected_publication_suffix_is_not_promoted`,
  `a_readiness_directory_sync_failure_clears_the_marker`,
  `a_marker_that_cannot_be_cleared_is_retained`,
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
