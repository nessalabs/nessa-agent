# Peer gateways — issue 705, slice H (first part)

Status: this branch. Part of [#252](https://github.com/nessalabs/nessa-agent/issues/252)
(slice H of [ADR 252](../../adr/todo/252-runtime-roles-and-execution-leases.md)).
The map's rules are in
[Identity and trust](../runtime-architecture.md#identity-and-trust),
[The mesh](../runtime-architecture.md#the-mesh) and
[Security](../runtime-architecture.md#security). This builds on
[device pairing](device-pairing.md), which it reuses whole.

## The problem

Device pairing lets a phone enroll with a one-use code. What it issues is a
credential that **acts as the owner**: the credential's principal is the
owner's, and it holds `conversation.read`. That is right for the owner's own
phone and wrong for another gateway, which is another party: a friend's Nessa,
or a second machine whose reads the owner wants to see and limit on their own.

The map asks for a peer gateway to be "a key-bound credential from pairing"
under a principal kind of its own, "never both authorities over one
conversation", with "the same enrollment and listener" and no second pairing
flow.

## What changes

The owner chooses **who an invitation enrolls** when creating it:

```text
pairing.create {}                     -> a device (unchanged)
pairing.create {"enrollee":"device"}  -> a device
pairing.create {"enrollee":"gateway"} -> a peer gateway
```

That choice is the consent's **class**, and it is the only difference between
the two enrollments:

| | Device (`gateway-conversation-read`) | Peer gateway (`peer-gateway-conversation-read`) |
|---|---|---|
| Enrollment | One-use code, OPAQUE over TLS with raw public keys, owner approves the exact key | The same, on the same listener |
| Grant asked for | `conversation.read` on this gateway | The same |
| Credential names | The owner | `gateway:<key in hex>`, a principal of kind `gateway`, member of the owner's organization |
| Credential proof | Bound to the key pairing pinned | The same |
| Revocation | `credential.revoke` or cancelling the Active pairing | The same |

The class is public and bound into the PAKE transcript beside the invitation,
attempt, consent and generation, so it cannot be changed in transit: a client
that relabels it cannot complete the exchange. A client also checks the class
on the first reply and refuses an invitation made for the other kind of party
before any attempt is charged (`NativeClientError::OtherEnrollee`): a phone
handed a peer's code does not become a gateway principal.

### One owner for each fact

- **Which principal a credential names** is `ConsentClass::credential_principal`
  in the auth domain: the owner for a device, `peer_principal(key)` for a peer.
  Publication mints it, and the registry's replay check (`credential_binding_matches`)
  asks the same function, so a stored credential that names anyone else is
  refused on open.
- **What a principal kind can hold** is `PrincipalKind::may_hold`. A gateway
  may hold `conversation.read` and nothing else. In particular it can never
  hold `conversation.write` (admitting commands into a conversation and
  answering its approvals) or `credential.manage` (holding credentials and
  pairing, which also admits every grant-management method). Those are the
  conversation authority's, listed once in `CONVERSATION_AUTHORITY_ACTIONS`,
  and the gateway that owns a conversation keeps them. Slice I will widen the
  peer's allowlist to the environment-grant actions; the authority's actions
  stay excluded, and `may_hold` refuses them before it consults the
  allowlist, so a widening cannot admit one by mistake. Whatever a peer is
  later allowed to run for a conversation, it therefore cannot also be that
  conversation's authority.
- **That the registry agrees** is `validate_principal_kind`, run by
  `validate_registry` on open and before every write. A gateway principal's
  credential must be pairing-bound, its membership a member's, and its grants
  ones the kind may hold. A pairing-bound credential names a gateway
  principal exactly when its pairing's class is a peer's, and a principal's
  id starts with `gateway:` exactly when its kind is a gateway.
  `credential.issue` refuses a gateway principal, and any principal id with
  that prefix whatever kind it claims, up front: pairing is the only way one
  is made.
- **Which class an enrollment is** is stored with it (`class: "peer"` in the
  registry's pairing record; absent means a device, so every existing registry
  reads unchanged) and in the client's private record (a separate pair of
  magics, so a device's file is byte-for-byte what it was).

### What a peer can do once paired

Authenticate a protected session on the native listener, and nothing else
yet. Every read of the owner's conversations is refused as not its own
(`wrong_owner`): the receiver binding pairing made names the owner whose
conversations it may read, and the session's principal is the peer, not that
owner. The socket's owner methods are `forbidden` to it, as to a device.

That holds even for a conversation shared with it. Per-conversation grants
(slice G, #704, merged in #720) let the owner `conversation.share` with the
peer's credential, because the peer's receiver binding names the owner as its
grantor, and the grant is written. The peer still reads nothing: every read
admission refuses a session whose principal is not the binding's owner before
it consults a grant. Reading what it is granted is the next part (below), so
pairing a peer, or sharing with one, discloses nothing yet.

```mermaid
sequenceDiagram
    participant Owner as Owner surface
    participant GW as Gateway (pairing runtime, Auth registry)
    participant Peer as Peer gateway (enrolling client)
    Owner->>GW: pairing.create {enrollee: "gateway"}
    GW->>GW: ConsentIntent(class = PeerRead), commit invitation
    GW-->>Owner: code (shown once), status.class = peer-gateway-conversation-read
    Peer->>GW: Hello (same native listener)
    GW-->>Peer: PublicIntent (class = PeerRead)
    Peer->>Peer: class is the one it enrolls as, else OtherEnrollee
    Peer->>GW: Begin / Confirm (OPAQUE; class bound in the transcript)
    GW->>GW: claim the peer's key
    Owner->>GW: pairing.approve (exact key)
    GW->>GW: stage, pair receiver, publish
    GW->>GW: credential names gateway:<key hex> (kind gateway, member), grant conversation.read, bound to the key
    Peer->>GW: status (pinned)
    GW-->>Peer: Active {credential, receiver, epoch}
    Peer->>GW: openProduct, session.authenticate
    GW-->>Peer: ready, principalId = gateway:<key hex>
    Owner->>GW: conversation.share {credentialId: the peer's}
    GW-->>Owner: applied (the binding names the owner as grantor)
    Peer->>GW: conversation.catalogueHead (or any read)
    GW-->>Peer: wrong_owner (session principal is not the binding's owner)
    Owner->>GW: credential.revoke (or pairing.cancel)
    Peer->>GW: openProduct
    GW-->>Peer: Refused
```

## State and order table

Each row has at least one test.

| Row | State and input | Result | Test |
|---|---|---|---|
| H1 | The owner creates a peer invitation; a peer claims it; the owner approves its key | Same enrollment and listener; the credential names `gateway:<key hex>`, kind `gateway`, an active member, holding only `conversation.read`, bound to that key; the record and the client's file keep the class | `owner_route_creates_a_peer_gateway_invitation`; `a_peer_enrollment_issues_a_gateway_principal_bound_to_its_key`; `an_enrollment_record_keeps_its_class`; `the_class_decides_which_principal_a_credential_names` |
| H2 | A client given an invitation for the other kind of party, or a client that relabels the class | Refused `OtherEnrollee` at the first reply, no attempt charged, nothing saved, the invitation still open; a relabelled class does not complete the PAKE; the wire carries the class as sent | `a_client_refuses_an_invitation_for_another_kind_of_party`; `a_relabelled_enrollment_class_does_not_complete_the_pake`; `the_enrollment_class_is_carried_and_never_changes_in_transit` |
| H3 | A paired peer authenticates and reads | `ready` as its own principal; a catalogue read is `wrong_owner`; `conversation.list` is `forbidden` | `a_peer_gateway_pairs_as_its_own_principal_and_reads_nothing_ungranted` |
| H4 | A gateway principal with `conversation.write` or `credential.manage`, an admin membership, a bearer credential, a credential naming another principal, or a peer record that lost its class | Refused on open and never written | `a_gateway_principal_can_hold_only_the_read_grant`; `a_gateway_principal_holds_only_what_pairing_issued_it` |
| H5 | The owner revokes a paired peer's credential | Its next connection is refused at `openProduct` | `a_revoked_peer_gateway_is_refused_its_next_connection` |
| H6 | `credential.issue` for a principal of kind `gateway` | Refused `Conflict`, nothing written: no second way to make a peer | `a_gateway_principal_holds_only_what_pairing_issued_it` |
| H7 | A gateway that pairs no peer | Every device enrollment, registry and client file reads and writes as before | the existing device pairing suites |
| H8 | The owner shares a conversation with a paired peer's credential | The share applies; on the native channel the peer's catalogue and record heads and both watches are `wrong_owner`, while a device granted the same conversation passes admission; on the socket its read, view and list subscriptions and lists are `conversation_not_found` and `conversation.shares` is `forbidden` | `a_peer_granted_a_conversation_still_reads_nothing_of_it`; `a_peer_granted_a_conversation_reads_nothing_on_the_socket` |

## What this part does not do

H as a whole is larger than one change. This part is the granting side. The
rest, proposed as the next part of #705 and the slices it names:

- **The peer side.** A gateway that enrolls into another one, keeps the
  resulting credential and pin, and reads as a surface. The enrolling client
  lives in `nessa-client-core`, which `nessa-server` may link only as a
  development dependency (`scripts/architecture/rust-dependency-graphs.mjs`);
  the next part decides whether the enrollment client moves to a crate both
  may use (`nessa-protocol` or `nessa-auth`) or the server gets its own.
- **Reading what it is granted.** Slice G (#704, merged in #720) names a
  grantee by its receiver binding. The binding's `owner_id` is the **grantor**: the owner
  whose conversations the receiver reads, who paired it. It is never the
  reader and must never be read as one; for a peer the reader is the
  session's `gateway` principal. Today admission refuses a session whose
  principal is not the binding's owner, which is what keeps a peer from
  reading anything. It is made in two places, each before any grant is
  consulted: `AdmitPassiveRead::binding` (catalogue head, manifest and
  resolve, record head and pages, catalogue and record watches) and
  `reader_of` (the socket's `conversation.read`, view subscriptions and
  lists). That check may be replaced by comparing the binding's
  credential with the session's **only in the same change that puts every
  peer read path behind G's grant filter**: the catalogue page, resolve and
  head, record head and pages, and catalogue and record watches. Relaxing it
  first would hand a peer the owner's whole catalogue. Whatever records a
  peer's read (audit, journal, logs) names the session's principal, never the
  binding's owner. Until then a share to a peer is accepted and listed by
  `conversation.shares` like a device's, and sits unused.
- **The peer table's last addresses and local discovery.** The issue lists
  them; the ADR places local discovery in slice I, and the map leaves "whether
  to announce at all, and what it reveals" unresolved. This side's peer table
  is the registry's rows: the principal (named by the key), the pinned key
  (the pairing's claim) and the grants (the credential's). Last addresses
  belong to the side that dials.
- **A Share or Linked gateways control on the desktop.** The method and the
  generated client take `enrollee`; the UI is a separate change with its
  browser verification.

## Known limits

- **The owner's Settings › Linked devices lists a peer's credential** among
  the devices, since both are pairing-bound credentials. Telling them apart
  in the list is part of the desktop control above.
- **Linked devices also shows a claimed peer invitation as an ordinary device
  awaiting approval**, with no class shown. This is a labelling gap, not a
  security one: the class is fixed when the invitation is created and bound
  into the exchange, so approving it yields only the read-only gateway
  credential, never a device credential that names the owner.
