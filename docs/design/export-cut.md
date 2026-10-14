# Export cut and deletion inventory

The gateway backup is one cut of the stores that own its records, taken while
admitted mutations are joined and released afterwards (#271). The deletion
inventory is the authenticated list of fences that a later restore must not
undo (#272). This document is the ordering table for both. Restore,
quarantine, and upload are not this cut: they wait on the cut landing.

```text
caller ── hold ──▶ conversation admission (write)
                      │
                      ├── pin each writer below: watermark, sqlite read
                      │   snapshot or a copy of a small file
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
the copy and not for the mac or the publish.

## Lock order

Admission is taken before any per-(conversation, receiver) grant gate. PR
#843 adds those gates on the command path: a command takes admission's read
guard, then the gate for that conversation and receiver. The export cut
takes admission's write guard and never takes a grant gate. It does not
call into a command, and a command does not call into the cut while it
holds a grant gate.

## Writer inventory

The cut names every store below. A store that is absent is an empty
watermark. A local outbox is not a store: the device outbox (#269) and the
environment's ephemeral outbox are not backup state.

| Store | What the cut keeps |
| --- | --- |
| Canonical records | The quiescent session database, one record for the file, when `page_count * page_size` is at most 64 MiB. The snapshot is copied in memory, not into a shared temp directory. The copy turns `secure_delete` on and runs `VACUUM` before the bytes are kept, so free pages are not in the published file, and the in-memory image is dropped before that `Vec<u8>` is built. The record is published when those bytes hold no private-key PEM header and none of the secret JSON keys `verifier`, `privateKey`, or `private_key`. A private-key header is `-----BEGIN` through the end of that line when the line contains `PRIVATE KEY`. A file that holds one is fingerprinted and named `skipped ... secret`. A file above 64 MiB is not copied. Each active conversation is a deferred entry: conversation id, the sum of its payload bytes, and a digest of that stream's incarnation, floor, tail, and records at the cut. The watermark is `deferred`. Opening the conversation later can fetch those bytes; this cut does not |
| Conversation metadata | Ownership, summaries, tombstones, catalogue heads |
| Share grants | Role, tool policy, and policy revision |
| Notes | Note records the canonical stream holds, named on their own. When the session file is above 64 MiB, notes carry the same deferred entries and the watermark `deferred` |
| Drive tool audit | Each Drive tool decision |
| Environment audit | Environment tool and lease decisions kept on the gateway |
| Conversation audit | Creation, deletion, mode, file-link, and execution audit |
| Peer pairings | Pinned public key, address, phase, credential id, relay |
| Environment-peer pairings | Environment-serve enrollments on this gateway |
| Worker enrollments | A worker's local pairing, public facts only |
| Environment grants | Per-peer grant file |
| Relay configuration | The gateway relay address and each peer's relay |
| Lease records | `environment/leases.jsonl`, one record per readable line |
| Artifact holds | Hold identity and digest |
| Device credentials | Public id, revocation, fingerprint |
| Peer credentials | The same, for a reading peer |
| Environment-peer credentials | The same, for `environment.serve` |
| Worker credentials | The same, when a fence names a worker |
| Receiver access | Receiver id, epoch, active flag |
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
| Deferred session | The session file's `page_count * page_size` is above 64 MiB | The file stays out of the in-memory snapshot. Each active conversation is a `deferred` line on canonical records and on notes: conversation id, payload-byte size, and the digest of incarnation, floor, tail, and records. The cut is still published. Fetching those bytes when the conversation is opened is not this cut. `an_oversized_session_is_deferred_and_the_cut_continues` |
| Deleted bytes | A row was removed and its old pages are still in the session file | The published copy is vacuumed, so those bytes are absent from the bundle. `a_deleted_secret_is_absent_from_the_session_backup` |
| Export publish fails | After the hold | The hold is released and nothing is published. `a_failed_export_releases_admission_and_publishes_nothing` |
| Retirement before the hold | The service is already retired | Explicit abort, no manifest. `retirement_before_the_hold_aborts` |
| Retirement during the hold | The fence is set, cleanup is waiting on the same lock | Explicit abort, no manifest, then the hold drops so retirement can finish. `retirement_during_the_hold_aborts_and_releases` |
| Missing store in the inventory | Before the hold | The attempt is refused and admission is not taken. `a_cut_refuses_to_start_without_every_store` |
| Local outbox | Any cut | The inventory has no outbox store. `the_writer_inventory_omits_local_outboxes` |

## Deletion inventory

Append-only, monotonic, authenticated with the same mac key the cut uses.
The key stays beside the inventory and is not part of the bundle. A fence
already committed is the input. A failed append leaves the file and the
fence as they were.

| Row | Order | Decision |
| --- | --- | --- |
| Revoked grant | Journal's latest row for that receiver is `after = none` | The entry names the role that was taken, for Read, Comment, and Drive. `revoked_grants_of_every_role_are_inventoried` |
| Revoked device, peer, environment-serve, or worker credential | After the registry fence | The entry names the kind and a fingerprint, never a verifier or private key. `revoked_credentials_are_inventoried_without_secrets` |
| Same fence again | A later capture | The original sequence is kept. `a_repeated_fence_does_not_advance_the_sequence` |
| Append fails | The fence is already committed | The failure is returned. The previous inventory and the fence stay. `a_failed_inventory_append_leaves_the_fence_and_the_log` |
| Capture during export | During the brief hold, after the append | The manifest's deletion-inventory watermark is the log after that append. `the_cut_includes_the_deletion_inventory` |
