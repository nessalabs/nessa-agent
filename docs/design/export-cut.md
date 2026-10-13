# Export cut and deletion inventory

The gateway backup is one cut of the stores that own its records, taken while
admitted mutations are joined and released afterwards (#271). The deletion
inventory is the authenticated list of fences that a later restore must not
undo (#272). This document is the ordering table for both. Restore,
quarantine, and upload are not this cut: they wait on the cut landing.

```text
caller ── hold ──▶ conversation admission (write)
                      │
                      ├── each writer below, while the hold lasts
                      ├── deletion inventory append (fences already committed)
                      └── manifest + mac, or abort with nothing published
                   release (drop), including failure and retirement
```

Arrows are the order one capture runs. The hold is the conversation
service's existing admission lock, taken for write so admitted commands
finish first. It is not a second lock.

## Writer inventory

The cut names every store below. A store that is absent is an empty
watermark. A local outbox is not a store: the device outbox (#269) and the
environment's ephemeral outbox are not backup state.

| Store | What the cut keeps |
| --- | --- |
| Canonical records | The quiescent session database, one record for the file |
| Conversation metadata | Ownership, summaries, tombstones, catalogue heads |
| Share grants | Role, tool policy, and policy revision |
| Notes | Note records the canonical stream holds, named on their own |
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

States: **Idle**, **Joining**, **Held**, **Captured**, **Aborted**,
**Released**. Released follows Captured or Aborted. A later attempt starts
from Idle.

| Row | Order | Decision |
| --- | --- | --- |
| Admitted mutation, then export | The mutation holds a read permit | The cut includes the mutation. `an_admitted_mutation_is_in_the_cut` |
| Export, then mutation | The export holds the write permit | The mutation waits, and is absent from the cut. `a_mutation_during_the_hold_waits_until_release` |
| Deletion during the hold | Same admission as any command | The cut shows the conversation either still present or already fenced, never half-erased. `a_deletion_during_the_hold_is_entirely_in_or_entirely_out` |
| Unreadable record | Other records in that store read | That record is skipped and named. The cut is still published. `an_unreadable_record_is_skipped_and_named` |
| Unreadable store | The source cannot list | The store is skipped and named. Other stores are still cut. `an_unavailable_store_is_skipped_and_the_cut_continues` |
| Secret material | A record is marked secret, or a secret-bearing path is read | The manifest keeps a fingerprint. The bytes are not in the bundle. `a_secret_is_fingerprinted_and_left_out_of_the_bundle` |
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
| Capture during export | While the hold lasts | The manifest's deletion-inventory watermark is the log after that append. `the_cut_includes_the_deletion_inventory` |
