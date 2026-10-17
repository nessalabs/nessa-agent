# Export cut and deletion inventory

The gateway backup is one cut of the stores that own its records, taken while
admitted mutations are joined and released afterwards (#271). The deletion
inventory is the authenticated list of fences that a later restore must not
undo (#272). This document is the ordering table for both. Restore,
quarantine, and upload are not this cut: they wait on the cut landing.

[Standalone verification](backup-verification.md) checks supplied cuts and an
explicit trusted MAC key. It implements format and integrity verification only;
restore, quarantine, key recovery and inventory freshness remain separate work.

```text
caller ── hold ──▶ conversation admission (write)
                      │
                      ├── pin each writer below: watermark, sqlite read
                      │   snapshot, a copy of a small file, or an open
                      │   descriptor and its length
                      ├── read both outboxes; a failure or a skipped row
                      │   aborts here, before fences are appended
                      └── deletion inventory append (fences already committed)
                   release (drop), including failure and retirement
                      │
                      ├── read records from those snapshots
                      └── manifest + mac + publish, or abort with nothing published
```

Arrows are the order one capture runs. The hold is the conversation
service's existing admission lock, taken for write so admitted commands
finish first. It is not a second lock. It lasts only for the pin. Reading
records, taking the mac, and publishing happen after it is released, from
the snapshots, so a later write is not in the cut. A command takes its
read permit then. The snapshot keeps SQLite's shared lock until that
store's records have been copied, so a writer of that same file waits for
the copy and not for the mac or the publish. A directory, JSONL, or relay
file is opened during the pin and its length is recorded; the bytes are
read after the hold, from that descriptor, and stop at that length. The
hold does not grow with the size of those files. An outbox is the
exception: its rows are read during the pin, each file only up to the
artifact bound, and only as many files and bytes as one lease may keep,
so a bad row aborts before the fence append. That read
holds `outbox.lock` shared for the whole directory, and the writer holds
it exclusively, so the pin does not copy a file that is still being staged
or removed.

## Lock order

Admission is taken before any per-(conversation, receiver) grant gate. PR
#843 adds those gates on the command path: a command takes admission's read
guard, then the gate for that conversation and receiver. The export cut
takes admission's write guard and never takes a grant gate. It does not
call into a command, and a command does not call into the cut while it
holds a grant gate.

Inventory captures take `backup/reconciliation.lock` across the log load,
current fence reads and the final no-op or replacement. Export acquires this
lease after admission. The short `backup/deletion-inventory.lock` protects
publication of both the inventory and its decision audit, including temporary
cleanup and rollback. The audit releases that lock before reconciliation starts;
load and store acquire it inside the reconciliation lease. Neither inventory
adapter calls back into conversation admission.

Device pairing does not take conversation admission. It holds
`receiver-access/publication.lock` from the receiver write through the
credential publish. During the pin, after admission, the cut holds that
same lock while it reads the credential registry and opens the receiver
snapshot. Pairing does not take admission while it holds the lock.

The environment outbox writer does not take conversation admission. It
holds `environment/outbox.lock` exclusively across a stage, a discard, a
lease close, and the sweep when serving starts. During the pin, after
admission, the cut holds that same file shared for the whole outbox read.
The device outbox uses `device/outbox.lock` the same way. The writer does
not take admission while it holds the lock.

## Writer inventory

The cut names every store below. A store that is absent is an empty
watermark. The device outbox (`device/outbox/`) and the environment outbox
(`environment/outbox/`) are stores. Each row is one regular file. The
watermark is the hex SHA-256 of the pinned `(identity, byte length)` pairs
sorted by identity. A missing directory's watermark is `absent`.

| Store | What the cut keeps |
| --- | --- |
| Canonical records | The quiescent session database, one record for the file, when `page_count * page_size` is at most 64 MiB. A file that exists but is not a database is copied only when it is within that same limit; a larger one is named `skipped ... unreadable` and is not read. The snapshot is copied in memory, not into a shared temp directory. The copy turns `secure_delete` on and runs `VACUUM` before the bytes are kept, so free pages are not in the published file, and the in-memory image is dropped before that `Vec<u8>` is built. The record is published when those bytes hold no private-key PEM header and none of the secret JSON keys `verifier`, `privateKey`, or `private_key`. A private-key header is `-----BEGIN` through the end of that line when the line contains `PRIVATE KEY`. A file that holds one is fingerprinted and named `skipped ... secret`. A file above 64 MiB is not copied. Each active conversation is a deferred entry: conversation id, the sum of its payload bytes, and a digest of that stream's incarnation, floor, tail, and records at the cut. The digest reads each payload through SQLite's blob interface, in chunks, so one record is not loaded whole. The watermark is `deferred`. Opening the conversation later can fetch those bytes; this cut does not |
| Conversation metadata | Ownership, summaries, tombstones, catalogue heads |
| Share grants | Role, tool policy, and policy revision |
| Notes | Note records the canonical stream holds, named on their own. When the session file is above 64 MiB, notes carry the same deferred entries and the watermark `deferred` |
| Drive tool audit | Each Drive tool decision |
| Environment audit | Environment tool and lease decisions kept on the gateway. `grants-audit.jsonl` is copied with `grants.json` while `grants.lock` is held, and parsed after admission is released. A public file above 8 MiB is named `skipped ... bounded` and is not read |
| Conversation audit | Creation, deletion, mode, file-link, and execution audit. Each record's identity includes its source directory, so `creation/<id>.json` and `deletion/<id>.json` stay two records |
| Peer pairings | Pinned public key, address, phase, credential id, relay |
| Environment-peer pairings | Environment-serve enrollments on this gateway |
| Worker enrollments | A worker's local pairing, public facts only |
| Environment grants | Per-peer grant file, copied under `grants.lock` in the same snapshot as `grants-audit.jsonl` |
| Relay configuration | The gateway relay address and each peer's relay. A peer file that does not parse is named `skipped ... unreadable` |
| Lease records | `environment/leases.jsonl`, one record per readable line |
| Artifact holds | Hold identity and digest |
| Device credentials | Public id, revocation, fingerprint |
| Peer credentials | The same, for a reading peer |
| Environment-peer credentials | The same, for `environment.serve` |
| Worker credentials | The same, when a fence names a worker |
| Receiver access | Receiver id, epoch, active flag |
| Device outbox | Each regular file under `device/outbox/`, read during the pin. The manifest watermark is that snapshot's exact cursor. A symlink, a file that is not a directory at the outbox path, a short read, a name this cut cannot keep, a file larger than the artifact bound, or more files or bytes than one lease may keep fails the source. Key material in a row is fingerprinted and the bytes are not published; that row is not a skip. `an_outbox_private_key_is_fingerprinted_and_the_cut_continues` |
| Environment outbox | The same, for `environment/outbox/`, in the same pin and the same hold. The read holds `environment/outbox.lock` shared, which the writer holds exclusively around stage, discard, close, and the sweep |
| Deletion inventory | The inventory as of this cut |

`ConsentClass::PeerEnvironment` is the one stored credential for an
environment peer and a hosted worker. The registry capture records that
class as an environment-peer credential. A worker fence is recorded as
`worker` when the caller knows the enrollment is a worker. Restore, which
this change does not build, has to treat those two labels as one credential
until the registry stores them apart.

## Export lease

States: **Idle**, **Joining**, **Held**, **Released**, **Captured**,
**Aborted**. Held is the pin. Released follows Held, and the read, mac, and
publish run after that. Captured is the published manifest. Aborted publishes
nothing and releases the hold too. A later attempt starts from Idle.

| Row | Order | Decision |
| --- | --- | --- |
| Admitted mutation, then export | The mutation holds a read permit | The cut includes the mutation. `an_admitted_mutation_is_in_the_cut` |
| Export, then mutation | The export holds the write permit for the pin | The mutation waits for that pin, and is absent from the cut. It can finish while records are read. `a_mutation_during_the_hold_waits_until_release` |
| Read and send during the read | Admission has been released; a snapshot read or the publisher is still running | `read_at` and a send on another conversation both finish. `a_read_and_a_send_finish_while_the_export_is_still_reading` |
| Deletion during the hold | Same admission as any command | The cut shows the conversation either still present or already fenced, never half-erased. `a_deletion_during_the_hold_is_entirely_in_or_entirely_out` |
| Unreadable record | Other records in that store read | That record is skipped and named. The cut is still published. `an_unreadable_record_is_skipped_and_named` |
| Unreadable store | The source cannot list | The store is skipped and named. Other stores are still cut. `an_unavailable_store_is_skipped_and_the_cut_continues` |
| Secret material | A record is marked secret, a path is not one of the named public store files, or a copied file's bytes hold a private-key PEM header or a secret JSON key (`verifier`, `privateKey`, `private_key`) | The manifest keeps a fingerprint. Bytes that hold key material are also named `skipped ... secret`. The bytes are not in the bundle. A quoted `"token"` or `"secret"` in a session file stays in the cut. A public-key or certificate header stays too. `a_secret_is_fingerprinted_and_left_out_of_the_bundle`, `an_unknown_gateway_file_is_not_copied`, `a_session_database_holding_a_private_key_is_not_copied`, `a_session_database_holding_an_rsa_ec_or_encrypted_private_key_is_not_copied`, `a_plaintext_public_file_holding_a_private_key_is_not_copied`, `a_json_record_holding_an_rsa_private_key_is_not_copied`, `a_public_jsonl_holding_a_private_key_is_not_copied`, `a_session_database_quoting_token_is_still_copied`, `a_public_key_header_in_a_session_database_is_copied`, `an_undashed_private_key_phrase_is_copied` |
| Deferred session | The session file's `page_count * page_size` is above 64 MiB | The file stays out of the in-memory snapshot. Each active conversation is a `deferred` line on canonical records and on notes: conversation id, payload-byte size, and the digest of incarnation, floor, tail, and records. Payload bytes are hashed in chunks. The cut is still published. Fetching those bytes when the conversation is opened is not this cut. `an_oversized_session_is_deferred_and_the_cut_continues`, `a_deferred_digest_hashes_each_payload_in_chunks` |
| Two publishes, one absent root | Concurrent | Each attempt stages in its own directory. One publish lands, and its manifest matches its files. The other attempt's staging directory is removed. An existing root is not replaced. `concurrent_publishes_of_one_root_do_not_mix`, `each_publish_attempt_has_its_own_staging_directory` |
| Published bundle | After the mac | On Unix, the bundle directory and each directory under it are mode `0700`. Each file is mode `0600`. `a_published_bundle_is_owner_only` |
| Raw session fallback | `records.sqlite3` exists and is not a database | A file within 64 MiB is copied. A larger file is named `skipped ... unreadable` and is not read. `an_oversized_non_database_session_is_not_read` |
| Public file bound | A public file is above 8 MiB | The file is named `skipped ... bounded` and is not read during the admission hold. `a_public_file_over_the_bound_is_named_and_not_read` |
| Directory body | An audit, JSONL, or relay file is inside the pin | The pin opens it and records its length. The body is read after admission is released, and a rewrite that replaces the file is not what the cut publishes. `a_directory_pin_does_not_read_the_body` |
| Environment grant writer | A grant command holds `grants.lock`, not conversation admission | The cut copies `grants.json` and `grants-audit.jsonl` together under that lock, and parses them after admission is released. `an_environment_grant_cut_waits_for_the_writer_lock` |
| Audit identity | Creation and deletion audits share a file name | The identity includes the source directory, so both records are in the bundle. `creation_and_deletion_audits_keep_separate_identities` |
| Malformed peer relay | A peer gateway file is readable and not JSON | The peer is named `skipped ... unreadable`. A readable peer relay is still cut. `a_malformed_peer_relay_is_named_skipped` |
| Deleted bytes | A row was removed and its old pages are still in the session file | The published copy is vacuumed, so those bytes are absent from the bundle. `a_deleted_secret_is_absent_from_the_session_backup` |
| Export publish fails | After the hold | The hold is released and nothing is published. `a_failed_export_releases_admission_and_publishes_nothing` |
| Retirement before the hold | The service is already retired | Explicit abort, no manifest. `retirement_before_the_hold_aborts` |
| Retirement during the hold | The fence is set, cleanup is waiting on the same lock | Explicit abort, no manifest, then the hold drops so retirement can finish. `retirement_during_the_hold_aborts_and_releases` |
| Missing store in the inventory | Before the hold | The attempt is refused and admission is not taken. `a_cut_refuses_to_start_without_every_store` |
| Outbox append after the pin | The append runs as admission is released | The manifest watermark is the pinned lengths. The appended bytes and a file created then are not in the bundle. `a_concurrent_outbox_append_is_outside_the_cut` |
| Outbox writer during the pin | `outbox.lock` is held exclusively by a stage, discard, close, or sweep | The pin waits, then reads the tree that lock released. A prefix of a copy still in progress is not the published row, and a removal does not abort a read that has not started. `an_environment_outbox_cut_waits_for_the_writer_lock`, `a_stage_and_a_close_wait_for_the_outbox_lock` |
| Outbox source fails | During the pin, before fences | The cut aborts. Admission is released. The deletion log is not written and nothing is published, including the other outbox. `a_failed_outbox_source_aborts_the_cut`, `an_invalid_outbox_is_refused_and_nothing_is_committed` |
| Outbox snapshot bound | During the pin, before any outbox body past the budget is read | One outbox's files or bytes are past what one lease may keep (`ArtifactBudget::LEASE` in `crates/nessa-server/src/conversation/application/artifacts.rs`, rows `artifact.environment_files` and `artifact.environment_bytes`). The cut aborts the same way. The other outbox is not published. `an_outbox_past_the_lease_budget_aborts_the_cut` |
| Outbox row skipped | During the pin, before fences | The cut aborts the same way. A kept row from that outbox is not published on its own. `a_skipped_outbox_row_aborts_the_cut` |

## Deletion inventory

Authenticated with the same mac key the cut uses. The key stays beside the
inventory and is not part of the bundle. A new key is synced, file then
directory, before it is used to seal anything. The published bundle syncs
each staged file and its directories before the rename, then the parent
directory after the rename. A fence already committed is the input.
A failed append leaves the file and the fence as they were. The
replacement is synced — the temporary file, then its directory — before the
append is acknowledged. Each attempt uses its own temporary file. A directory
sync that fails after the rename restores the previous bytes, or removes the
new file when there was none, and then returns the error.
`a_directory_sync_failure_restores_the_previous_log` and
`a_directory_sync_failure_removes_a_log_that_was_absent`. Two writers compare
the log they read. The one that lost reconciles again, so a conversation
or credential fence the winner stored stays. `a_stale_log_is_reconciled_again`,
`a_stale_writer_does_not_replace_the_log`, and
`concurrent_stores_keep_one_complete_log`. Each attempt loads the log and
then reads the fence sources again. A share-grant revoke stored before
that load stays when the new read still names it.
`a_concurrent_revoke_is_not_dropped_by_a_stale_fence_snapshot`.

A command that has already committed a fence records it without taking
admission and without publishing a bundle. That command is a deletion, an
unshare, a credential revocation, or a pairing cancellation. The gateway
asks Cedar and stores that decision, allow or deny, before it reads the
log. An audit that cannot be stored does not read the log. A refusal or a
failed append leaves the fence and the previous log, and the command's
result is unchanged. A device, peer, environment peer, or worker is
refused and the inventory log is not written. A person or the gateway's
own surface may capture when they ask.

Either fence source failing leaves the log
unwritten, including when the other source already has rows. A metadata
tombstone or revocation that cannot be decoded or named fails that fence
source the same way. Absence of the row would be read as a re-grant. The
metadata and grant cuts still name the row. A credential row the registry
read skips fails that fence source the same way. A share-grant
fence is removed when a later read's latest journal row is a grant again.
Conversation and credential fences stay. Survivors are rewritten at
sequences 1, 2, 3, … and are still matched by key.

| Row | Order | Decision |
| --- | --- | --- |
| Revoked grant | Journal's latest row for that receiver is `after = none` | The entry names the role that was taken, for Read, Comment, and Drive. `revoked_grants_of_every_role_are_inventoried` |
| Re-grant | A later journal row grants that receiver again | The revoke fence is not in the cut. Other fences stay. `a_regrant_drops_the_revoke_fence_and_keeps_other_fences` |
| One fence source fails | Metadata or the credential registry cannot be read | No fence from the other source is committed. The cut names the append unavailable. `a_failed_fence_source_does_not_commit_the_other_source` |
| Damaged fence row | A tombstone or revocation cannot be decoded or named | The fence source fails. A revoke already in the log stays. The grant cut names the row. `a_damaged_fence_row_leaves_the_stored_revoke` |
| Damaged credential row | The registry parses and one credential row does not | The fence source fails. No credential fence from that read is committed. The credential cut names the row. `a_skipped_credential_row_does_not_commit_fences` |
| Device pairing | A receiver is written, then its credential is published, without conversation admission | The cut reads both under `publication.lock`, which pairing holds across those two writes. `a_publication_lock_snapshots_the_credential_and_the_receiver_together` |
| Revoked device, peer, environment-serve, or worker credential | After the registry fence | The entry names the kind and a fingerprint, never a verifier or private key. `revoked_credentials_are_inventoried_without_secrets` |
| Same fence again | A later capture | The original sequence is kept. `a_repeated_fence_does_not_advance_the_sequence` |
| Append fails | The fence is already committed | The failure is returned. The previous inventory and the fence stay. `a_failed_inventory_append_leaves_the_fence_and_the_log` |
| Directory sync fails after rename | The new file is already at the inventory path | The previous bytes are restored, or the new file is removed when the path was absent, before the error is returned. `a_directory_sync_failure_restores_the_previous_log`, `a_directory_sync_failure_removes_a_log_that_was_absent` |
| Two inventory writers | Each read the log, then both store | One store lands a complete log. The other is refused and reconciles again, so a conversation or credential fence already stored stays. `concurrent_stores_keep_one_complete_log`, `a_stale_writer_does_not_replace_the_log`, `a_stale_log_is_reconciled_again` |
| Concurrent revoke | Another capture stored a share-grant revoke before this one loads | This attempt loads that log, then reads fences again, and keeps the revoke when the new read still names it. `a_concurrent_revoke_is_not_dropped_by_a_stale_fence_snapshot` |
| Capture during export | During the brief hold, after the append | The manifest's deletion-inventory watermark is the log after that append. `the_cut_includes_the_deletion_inventory` |
| Command, then inventory | After a deletion, unshare, credential revocation, or pairing cancellation commits | The gateway asks Cedar, stores that decision, then reconciles. It does not take admission and does not publish a bundle. A refusal or a failed append leaves the fence and the previous log. The command's result is unchanged. A capture failure after `pairing.cancel` leaves the cancellation. A worker enrollment names an `environment.serve` credential as `worker`; a device or peer id in that file stays what the registry said. A damaged enrollment commits nothing from that read. `a_failed_inventory_capture_leaves_the_deletion`, `a_revoked_worker_credential_is_inventoried`, `a_damaged_worker_enrollment_does_not_commit_fences`, `an_unshare_reconciles_the_revoked_grant`, `pairing_cancel_records_the_inventory_and_a_failure_still_cancels` |
| Who may record | Before the log is read | Cedar permits the gateway to record a committed fence, and a person or the gateway's own surface to capture when they ask. A device, peer, environment peer, or worker is refused. The allow or the deny is stored first. A failed decision store does not read the log, and a refusal does not create it. `the_gateway_may_record_a_committed_fence`, `a_device_peer_environment_or_worker_may_not_capture`, `a_denied_capture_does_not_write`, `the_inventory_decision_is_stored_before_the_log`, `a_failed_decision_audit_does_not_write` |

## Integrity follow-up (#901)

The existing export, deletion fences, and Cedar decisions remain the owners of
these facts. The follow-up orders their existing adapters; it does not implement
restore, upload, authority transfer, or deferred-session recovery (#854).

| Order | Required behavior | Regression evidence |
| --- | --- | --- |
| A capture holds reconciliation while its snapshot needs the blocking pool; another capture waits for the same lease | File opening uses short blocking work. Contention uses nonblocking OS lock attempts and asynchronous retry, leaving pool workers available to the holder. Cancellation drops the open file or acquired lease. | `waiting_captures_leave_a_blocking_worker_for_the_holder`, `a_cancelled_reconciliation_waiter_releases_its_eventual_lock` |
| Old capture reads a revoke from an empty inventory; a newer re-grant capture would leave that inventory unchanged | Hold one reconciliation lease over load, source reads, no-op and replacement. The newer capture waits for the old one and then removes the obsolete revoke; a no-op cannot bypass a pending older replacement. The adapter provides a separate process-safe lock; the short publication lock remains inside it. | `a_noop_regrant_capture_waits_for_an_older_revoke_read`, `a_failed_reconciliation_lease_does_not_read_sources_or_the_log`, `separately_opened_inventories_share_the_reconciliation_lease`, `a_cancelled_reconciliation_waiter_releases_its_eventual_lock` |
| Offline owner or surface rotation commits while the gateway is stopped; next startup | Reconcile committed fences after opening the inventory and before publishing product listeners. A failed capture refuses startup; repeated startup retains the same fences. | `startup_records_offline_credential_rotations_once`, `startup_refuses_an_unreadable_committed_fence_source` |
| A re-grant commits, its inventory capture fails, then an equivalent grant is retried with a new request identity | Retry inventory reconciliation even if the grant receipt says it did not apply another change. The current grant removes the old revoke; denied grants still do not capture. | `a_retried_share_reconciles_after_a_failed_capture` |
| Inventory replacement is staged or its rollback copy is live; decision audit starts | Both publishers take the same directory lock before stale temporary cleanup, replacement, and rollback. The audit waits until inventory rollback finishes. | `a_decision_write_waits_for_inventory_rollback` |
| Outbox or receiver-access parent is initially absent; its first writer starts | Establish the writer's lock before observing the stores. Parent absence cannot authorize reading a later-created tree without the lock. Missing stores remain represented as absent. | `absent_parent_outbox_cut_excludes_first_writer`, `absent_receiver_parent_cut_excludes_first_publisher`; file/symlink refusal tests beside them |

The synchronous capture await on committed delete, unshare, revoke, and pairing
cancel remains intentional for this scope: commands attempt local inventory
reconciliation before replying; they do not await an upload. Failure preserves
the committed operation and is logged. Moving this work into a durable retrying
reconciler needs queue ownership and shutdown semantics and is separate work.
Decision-log growth and rewrite cost remain a known performance limitation.


### Recovery acceptance still outstanding

#270 remains open for replacement identity, independently verified manifest and
deletion inventory, quarantine, old-source/remote-lease fencing, explicit
reconciliation and a synthetic restore drill that proves historical commands do
not execute. This integrity follow-up does not supply that restore path.

#854 remains separate: the cut records deferred conversation identities, byte
sizes and exact source-state digests for oversized sessions; it does not retain
or fetch the recorded version. Recovery needs a source that can serve that
immutable version, a bounded import into the restore path, progress on open, and
an explicit unavailable/changed-source error that leaves other conversations
usable. Acceptance must exercise successful recovery of a synthetic session
over 64 MiB, source disappearance/change, identity mismatch, interrupted fetch
and retry, while preserving quarantine and command non-execution. The existing
deferred-entry/digest tests prove export metadata only.
