# 270. A restored gateway is quarantined until it is accepted

## Purpose

Make a lost gateway recoverable from an export cut without letting a partial
or failed restore touch live data, and without treating an oversized
conversation's fingerprint as the conversation itself.

- **Date:** 2026-10-20
- **Status:** accepted
- **Issue:** [#270](https://github.com/nessalabs/nessa-agent/issues/270), [#854](https://github.com/nessalabs/nessa-agent/issues/854)

## Context

The export cut and the offline verifier are already the backup. The verifier
authenticates a supplied bundle and stops. Private keys are not in the bundle.
An oversized session is not copied into the snapshot; the cut names each
conversation with its id, size, and stream fingerprint. `nessa-backup-plan`
stays an inspection command and does not gain a destination.

## Decision

Restore writes only under a quarantine directory that is not the live root.
The bundle is verified again, revoked grants and deleted conversations named
by the authenticated inventory are left out, and every still-open environment
lease is followed by an `ended` line the ledger already reads as uncertain.
The quarantine is sealed. Acceptance renames that directory onto an empty live
path after Cedar allows it and, when a lease was still open, the operator
supplies an attestation that the executor was stopped. A failure before that
rename deletes the partial tree and does not create or modify the live path.

The gateway's private key is not recovered. Acceptance is a fresh local
authority: devices, peers, environments, and workers re-pair. Historical
outbox bytes may be present and are marked so they are not replayed. This
use case starts no harness, command, or lease.

An oversized conversation stays a deferred line. Opening it streams that
conversation from a source the caller supplies, checks the stream fingerprint
and size against the sealed line, and only then publishes the session file.
A miss, a change, or an interruption publishes nothing and leaves every other
conversation's file as it was.

The ordering table is [quarantined restore](../../design/quarantined-restore.md).

## Alternatives considered

- **Copy the live tree, then repair it.** A failed repair has already
  replaced the only good copy. Quarantine makes the first live write the
  successful rename.
- **Promote when the manifest mac matches, in place.** The mac does not know
  about a grant revoked after the cut, an open lease, or a half-written
  directory. The seal and the inventory check are the promotion gate.
- **Put the oversized session in the snapshot.** That is the copy the 64 MiB
  bound exists to avoid, and it is the opposite of the decision that the
  backup lists the exact version instead. Fetch-on-open checks that version.
- **Retain a second full copy of the oversized session beside the bundle.**
  The recorded fingerprint would then be of a copy we also have to store and
  delete. The source that still has that version is the fetch target; if it
  has changed or gone, the open says so.
- **Keep the old gateway key and a restore generation.** The key is not in
  the cut. Inventing a generation peers do not yet enforce would pretend the
  restored process is the old authority. Re-pair does not.
- **Refuse acceptance whenever any lease was open, with no attestation.**
  That is safer and also makes a drill with a live lease unable to finish.
  The attestation is recorded and is not a network fence. No attestation
  leaves the gateway quarantined.
- **Let `nessa-backup-plan` perform the rename.** The plan command's contract
  is that exit 0 is an inspection. A second use case owns the rename.

## Consequences

A person can stage, accept, and open a deferred conversation without a
running gateway having been the restore tool. The desktop and the
`conversation.read` handler do not call this use case yet; a later change
wires them. Remote executors are not contacted. An authenticated inventory
that drops a fence the cut already has is refused; a missing later inventory
does not claim to know revocations made after the cut. A fenced conversation
that is still inside a session database is removed from that restored image.
Accept and fetch share one lock so a fetch cannot publish a session that
acceptance then carries onto the live path without its audit. Accept also
holds the live path's lock across that rename. A quarantine is promotable
only after its sealed audit is stored. The fetch checks the session with
the restore layout, and a fetch that cannot be audited removes, moves, or
empties that file. Emptying truncates the file directly, because the
storage opener refuses a copy it did not create. A canonical session image that cannot be read is not
sealed while a conversation fence applies.
