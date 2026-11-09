---
id: gateway-same-person
title: Pair a gateway as the same person
kind: flow
status: implemented
parent: gateway
summary: An immutable invitation choice adds credential-bound attribution after activation, without granting access.
sourceRevision: "a3dd63839d52a616560cc70eca470f6a82dbb86f"
sources:
  - crates/nessa-server/src/product/pairing.rs
  - crates/nessa-server/src/composition/local_auth.rs
  - protocol/product/v1.json
  - crates/nessa-auth/src/domain/pairing/value_objects/person_link.rs
  - crates/nessa-auth/src/domain/pairing/value_objects/values.rs
  - crates/nessa-auth/src/application/pairing/person_links.rs
  - crates/nessa-auth/src/domain/pairing/aggregates/invitation.rs
  - crates/nessa-auth/src/adapters/local/registry/pairing/person_links.rs
  - crates/nessa-auth/src/adapters/local/registry/pairing/enrollment.rs
  - crates/nessa-server/src/device_pairing/application/activation.rs
  - crates/nessa-server/src/conversation/infrastructure/store/person_links.rs
  - crates/nessa-server/src/conversation/infrastructure/store/grant_database.rs
  - crates/nessa-server/tests/device_pairing/infrastructure/person_links.rs
  - crates/nessa-server/tests/conversation/grant_database.rs
  - crates/nessa-server/src/peer_gateways/infrastructure/records.rs
  - crates/nessa-server/tests/device_pairing/mounted/same_person.rs
  - crates/nessa-server/tests/peer_gateways/same_person.rs
  - crates/nessa-server/tests/backup/deletion.rs
diagramLinks: {}
---
# Pair a gateway as the same person

An owner creating a gateway invitation can choose that the gateway acts for the
same person. A true choice requires its authoritative owner principal to be
Human. Admin desktop or CLI surface principals without a published person
mapping receive an ineligible refusal; an admin role is not person provenance.
Ordinary false-choice pairing is unchanged. After pairing, a granted read reports the gateway principal and
that person together. The association grants no access: an unshared conversation
still cannot be read, and comment or drive remain subject to the existing Cedar
policies. A gateway paired without the choice reports no person.

For example, the owner's laptop and desktop can each have a separate read grant
on one conversation. Their shared person does not collapse those grants. Device
and owner-phone consent retain their own classes and cannot select this choice.

## Choice, activation and recovery

`pairing.create` stores `samePerson` on the immutable invitation. The authenticated
exchange binds both true and false, and the client retains the exact choice.
Changing it in transit prevents pairing. Restart before activation restores it.

The credential registry remains the activation owner. It commits Active, the
credential and receiver first. The existing grant owner then commits the person
link and its audit together in
`<namespace>/access-grants/access-grants.sqlite3`. These are two commits.
An error can therefore return an Active receipt with `activationStopped`; the
credential was issued, but attribution remains owed. Reapproving uses the same
credential and receiver. The next grant write and startup use the same
reconciler. Read admission reports the person only after the link commits.

The chart follows the link obligation, not a new enrollment phase. A grant still
has its independent subject, capabilities and epoch throughout this flow.

```mermaid
stateDiagram-v2
    [*] --> ChoiceSaved: create gateway invitation
    ChoiceSaved --> NoLink: activate with false choice
    ChoiceSaved --> LinkOwed: commit Active with true choice
    LinkOwed --> LinkOwed: link or audit write fails
    LinkOwed --> Linked: retry commits link and audit
    LinkOwed --> Removed: revoked before a link commits
    Linked --> RemovalOwed: credential revoke, expiry or enrollment revoke
    RemovalOwed --> Removed: retry commits removal and audit
    RemovalOwed --> RemovalOwed: write fails or replacement activation waits
```

## Removal retains its cause

Credential revocation or enrollment cancellation commits in the credential
registry first. Removal then uses the original owner or expiry initiator and
cause, including live retries and restart. The domain record retains its
original terminal event through history replay: cancellation must match its
Explicit revocation and command time, independent revocation must match its
exact original transition, and a pre-Active cancellation cannot supply an issued
credential. Custom effect sources consume this same correlation validator. Link and removal audit cannot commit
partially. Grant rows and their epochs do not change. Exclusive expiry uses the
fixed credential expiry; if revocation also occurs, the earlier boundary retains
its meaning as time advances.

A later enrollment of the same gateway key waits while an old link remains,
including when the new choice is false. Cleanup names the exact old credential,
so it cannot remove a later association. A committed removal is a durable receipt
that prevents replaying the consumed invitation after restart or clock rollback,
including when expiry was observed before the first link could commit.
Ordinary device and owner-phone activation do not perform an unrelated owed
same-person insertion.

## Grant storage boundary

The grant file lives outside `conversations/`, so opening it does not create
conversation data or change retirement accounting. Startup repair and credential
revocation cleanup remain bound when native linking is disabled and when no
agent is configured. The peer gateway retains the choice in its private record
through activation and restart.

The grant file has its own current schema version. A different version is refused
before conversation metadata is opened, and is never migrated or rewritten.
Read-only backup snapshots apply the same version and shape checks. Legacy grant
rows in conversation metadata remain unused and unchanged. Paired grants use
receiver subjects; unpaired grants use principal subjects. Their export identities
include that kind, so equal text in the two namespaces cannot collide.

## Review scope and limits

This page traces the source at the checkpoint above and the focused regression
fixtures listed in Sources. Execution receipts and deliberate failure probes are
recorded in the pull request and its linked review evidence. The chart has no
formal model; the older device activation model does not cover these two-store
link obligations. No real device, account grant, live credential, physical phone
or deployed gateway was used to verify this flow. Peer subscriptions, desktop
choice controls and unrelated gateway lifecycle models are outside this change.
