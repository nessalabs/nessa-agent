# Peer gateways — issue 705, slice H

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

Read, on the native listener, exactly what the owner granted it, as a paired
device does. The owner shares a conversation with the peer's credential
(`conversation.share`, slice G, #704, merged in #720); the grant names the
peer's receiver, and every read the peer makes asks that receiver's grants:

- **Record head and pages, and a records watch** go through
  `AdmitPassiveRead::execute`, which asks `admit_read` for the receiver's
  grant on that conversation; an ungranted one is `wrong_owner`.
- **Catalogue manifest and resolve** are narrowed in SQL to the receiver's
  granted rows (`store::read_grants::granted`). A manifest pass may not reach
  past the receiver's own head, so probing boundaries finds that head and not
  the owner's; an ungranted id resolves as one that does not exist.
- **Catalogue head** is the latest revision among the receiver's granted rows
  and its own grant changes. The owner's work on a conversation the peer was
  not granted does not move it; a grant, a revoke or a change to a granted row
  does, and in steady state it never goes backwards. Its values are still the
  owner's catalogue revisions (see Known limits). This applies to every paired reader,
  devices included: it is the head over granted rows that slice G left for
  when paired readers are more than the owner's own phones.
- **Catalogue watch** is refused to a peer (`forbidden`). Its notices fire on
  every change to the owner's catalogue, granted or not, which would show a
  peer when the owner works on conversations it cannot read. A peer polls its
  head instead. A device, which signs in as the owner, keeps its watch.
- **The socket's `conversation.read`, `.list`, `.observe` and subscriptions**
  need `conversation.write`, which a gateway principal can never hold, so the
  policy refuses them. Behind that, `reader_of` makes a paired reader of a
  peer like a device: its read and view ask the grant, its lists are refused.

Admission matches the receiver binding to the session by credential and
organization. The binding's `owner_id` is the **grantor**: the owner whose
conversations the receiver reads, who paired it. It is never the reader and is
never compared with the session's principal, which for a peer is its own
`gateway` principal. Whatever records a peer's activity names that session
principal: its watch allowance is counted against it, and its session is
logged as it. The grant journal names the owner, who made the decision.

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
    GW->>GW: grant on the peer's receiver, journaled with the owner as initiator
    Peer->>GW: conversation.catalogueHead
    GW-->>Peer: head over its granted rows only
    Peer->>GW: conversation.catalogueManifest / catalogueResolve
    GW-->>Peer: the granted rows only
    Peer->>GW: conversation.recordsHead / recordsPage (a granted conversation)
    GW-->>Peer: records (an ungranted one: wrong_owner)
    Peer->>GW: conversation.watchCatalogue
    GW-->>Peer: forbidden (fires on all of the owner's work)
    Owner->>GW: conversation.unshare
    GW-->>Peer: head moves forward; the row leaves its catalogue
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
| H3 | A paired peer authenticates and reads before anything is shared with it | `ready` as its own principal; its catalogue head is zero, so it has nothing to page; `conversation.list` is `forbidden` | `a_peer_gateway_pairs_as_its_own_principal_and_reads_nothing_ungranted` |
| H4 | A gateway principal with `conversation.write` or `credential.manage`, an admin membership, a bearer credential, a credential naming another principal, or a peer record that lost its class | Refused on open and never written | `a_gateway_principal_can_hold_only_the_read_grant`; `a_gateway_principal_holds_only_what_pairing_issued_it` |
| H5 | The owner revokes a paired peer's credential | Its next connection is refused at `openProduct` | `a_revoked_peer_gateway_is_refused_its_next_connection` |
| H6 | `credential.issue` for a principal of kind `gateway` | Refused `Conflict`, nothing written: no second way to make a peer | `a_gateway_principal_holds_only_what_pairing_issued_it` |
| H7 | A gateway that pairs no peer | Every device enrollment, registry and client file reads and writes as before | the existing device pairing suites |
| H8 | The owner shares one of two conversations with a paired peer, then unshares it | The share applies (an access answer about another credential, or one that contradicts itself, is still refused); the peer's record head on the granted one reaches the source and on the other is `wrong_owner`, as are a records watch and a record page on it; resolving the other answers as an id that does not exist; a manifest boundary one past the peer's head is `invalid_request` though the owner's head is past it; its manifest lists the granted one alone; its catalogue watch is `forbidden`, while a device's is admitted; the owner's work on the other conversation does not move the peer's head; the unshare moves the head forward and takes the row away. On the socket, a binding whose owner is not the session principal reads only what it was granted, and its lists are refused | `a_peer_reads_only_what_it_was_granted`; `a_binding_owned_by_another_principal_reads_only_what_it_was_granted` |
| H9 | A paired reader's catalogue head as the owner works, grants and revokes | An ungranted change and a grant to another receiver leave it; a grant, a change to a granted row and a revoke each move it; a revoke moves it forward, never back to an older granted row | `a_paired_readers_head_moves_only_with_what_it_was_granted` |
| H10 | A registry whose owner membership is disabled or not an admin | Refused on open (`owner_membership`). The pairing initiator's membership is what keeps a grantor live; the owner's is the one the registry pins | `a_registry_whose_owner_is_not_an_active_admin_does_not_open` |

## The dialing side

Part 2b gives a gateway the other end: its owner enrolls it into another
gateway's peer invitation, and it keeps what it learned there. The enrolling
client is `nessa-client-core`'s, which the gateway links without its `cli`
feature ([ADR 483](../../adr/done/483-protocol-and-client-core-crates.md),
amended).

```mermaid
sequenceDiagram
    participant O as B's owner
    participant B as Gateway B (product socket)
    participant R as B's peer records
    participant K as B's gateway key store
    participant A as Gateway A (native listener)
    participant U as B's peer audit
    O->>B: peer.enroll {address, code} (Cedar: credential.manage)
    B->>U: intent {operation, address, owner} (not kept: peer_audit_unavailable, nothing checked or dialed)
    B->>B: take the turn (a poller cycle: stop it and take it; another command: peer_busy, outcome audited)
    B->>A: TCP connect via PeerConnector, bounded by the deadline clock (passed: peer_unreachable)
    B->>A: TLS with B's own native key
    B->>R: enrollment key?
    R->>K: restore B's gateway key
    A-->>B: Hello (class must be peer read, else peer_wrong_invitation)
    B->>A: OPAQUE with the code (wrong code: peer_invitation_refused)
    A-->>B: KE2, authenticating A's key
    B->>R: save pending {A's pin, B's key by its public half, address}
    R->>R: note A's key, then its record as read
    R->>R: refuse another key, B's own key, a kept peer, or one past MAX_PEERS
    B->>A: KE3 (confirm)
    A-->>B: Claimed
    B->>U: outcome {operation, peer, before -> after, answer}, on every path (not kept: peer_audit_unavailable)
    B-->>O: PeerGateway {peerKey, phase: pending, address}
```

- **One key, one principal.** B enrolls with the gateway's own native key,
  injected from the gateway key store, never a key made for the enrollment
  (`ClientPendingStore::enrollment_key`). A therefore names B by the same
  `gateway:<key>` principal however often it is enrolled, which slice I
  needs. The record names that key by its public half; the private half
  stays in the key store alone, and a record made with a key that is no longer
  the gateway's does not read.
- **One file per peer.** `peer-gateways/<peer key hex>.json` beneath the
  namespace, private, published by rename through `nessa-local-storage`. It
  holds the peer's pin, the enrollment it claimed, the credential once
  issued, and the address that last answered, with its own `schemaVersion`
  ([ADR 202](../../adr/todo/202-versioned-local-datasets.md), record scope: a
  record that does not read is listed `unreadable` and fails only that
  peer).
- **What the owner is told.** `peer.enroll` returns the pending record;
  approval happens on A. The credential reaches the record when B next reads
  its pinned status, which the poller does ("Reading a peer", below). `peer.list` shows
  every kept peer; `peer.forget` removes B's record only.
- **It dials what the owner names.** `peer.enroll` connects from this
  gateway's host to whatever IP address and port the owner gives, which is
  why it is owner-only (`credential.manage`) and why every failure that is
  not the peer protocol's own answer is `peer_unreachable`: an open port and
  a closed one answer alike. The client keeps a failed TLS handshake
  (`NativeClientError::Handshake`) apart from a failed PAKE proof, so a
  service that is not a gateway never reads as a wrong code.
- **Every deadline is the injected clock's.** The connect goes through the
  `PeerConnector` port, which sets no timeout of its own; the command reads
  the injected monotonic clock every wake tick and drops the attempt once
  the connect deadline passes, and reads it again when a connection
  completes, so one that completes past the deadline is closed unused.
  Each audit record is bounded the same way, by `AUDIT_DEADLINE`: one not
  acknowledged in time counts as not kept, though the audit may still keep
  it later. Each enrollment's entropy comes from an injected source too, so
  a failing generator is a typed `peer_unavailable`, audited. The handshake and enrollment deadlines read
  the same clock, so a substituted clock drives the whole enrollment.
- **Enrolling and forgetting take turns.** One enrollment or forget runs at a
  time, and another is `peer_busy` while it does, so a forget cannot remove
  the record an enrollment is saving and spend the peer's invitation for
  nothing. A poller cycle takes the same turn, but never makes a command
  busy: the command stops the cycle and takes the turn once it is given
  back, within `PREEMPT` (10 s) on the injected clock ("Reading a peer", below).
- **Every enroll and forget call is audited.** Both commands run only
  through one audited wrapper. It hands an intent to the `PeerAudit` port
  before anything else, even the check that no other command holds the turn
  (the address or the peer named, and the owner who asked), and an outcome
  on every way the call ends (the peer, its record before and after, and the
  answer), correlated by one operation id. The command body answers with
  that evidence on each of its returns, as its type demands, so no early
  return skips the outcome. The enrollment's slot notes the peer's key the
  moment the peer's pin reaches it, and its record as first read, so every
  later refusal (`peer_exists`, `peer_capacity`, `peer_own_gateway`) names
  them; a call that ended before reading the record says `not_read`.
  `DurablePeerAudit` keeps each record as its own private file in
  `peer-gateways-audit/`, synced before it is acknowledged. It is the one
  authority over the records' order: it gives each record its `sequence`
  (from 1 in each process), id and `observedAtMs` the moment it is handed
  over, under the same lock that puts it on a bounded queue
  (`AUDIT_QUEUE`, 64, a row of [`limits.md`](../../limits.md)), and one
  writer thread writes the queue in that order. A full queue refuses the
  record at once rather than wait; a refused record uses up its sequence,
  so a gap in the files is a refusal the log names. Stopping native pairing
  first joins the poller, then closes the peer commands, in one step with
  the count of those running: a command asked from then is refused
  `peer_unavailable` before its intent, one already past its intent no
  longer takes the turn (`peer_unavailable`, its outcome kept, before any
  effect), and those running are waited for, within `PREEMPT` on the
  injected clock, so no effect is left without its outcome (see Known
  limits for one that outlasts it). Only then is the audit closed (a command from then on is refused
  before its intent) and its writer waited for, within one `AUDIT_DEADLINE`;
  records still queued past it are logged by sequence and not written, and
  a writer that panics ends the drain at once, its unwritten records logged
  by sequence. An intent that is
  not kept refuses the command before anything is dialed or removed; an
  outcome that is not kept answers `peer_audit_unavailable` while the effect
  stands, so the owner never sees success without its evidence. A process
  that stops between an effect and its outcome leaves the intent, and the
  peer record says what it came to. Logs carry the same fields; never the
  code or key material.
- **Forgetting is local.** A gateway principal can never hold
  `credential.manage`, so B cannot revoke what A issued it. A's owner revokes
  it on A (`credential.revoke`), which is what stops B reading.

| Row | Situation | Result | Test |
| --- | --- | --- | --- |
| P1 | B enrolls into A's peer invitation | A records B's own native key as the claim; B keeps a pending record pinning A's key | `a_gateway_enrolls_into_a_peer_with_its_own_key_and_keeps_only_a_reference`, `a_composed_gateway_enrolls_into_another_with_its_own_key` |
| P2 | B's record on disk | A's pin and B's key's public half; B's private key in no spelling | same |
| P3 | A's owner approves; B reads its pinned status | The credential replaces pending in the same record; listed `active` | `a_gateway_enrolls_into_a_peer_with_its_own_key_and_keeps_only_a_reference` |
| P4 | `peer.forget` | The record is removed, its cache first; a second forget is `peer_not_found`. A cache removal not confirmed durable leaves the record untouched and is audited `cache: unconfirmed` beside it, `peer_unavailable`; forgetting again finishes it | same |
| P5 | A device invitation's code | `peer_wrong_invitation`, before the PAKE; nothing saved | `a_refused_or_wrong_class_invitation_saves_nothing`, `a_composed_gateway_enrolls_into_another_with_its_own_key` |
| P6 | A wrong code, or a cancelled invitation | `peer_invitation_refused`; nothing saved | `a_refused_or_wrong_class_invitation_saves_nothing` |
| P7 | Nothing at the address; a peer already kept | `peer_unreachable`; `peer_exists`, the kept record unchanged byte for byte, and audited with that peer's key and its state before and after, both as found | same |
| P8 | No native pairing; a member; malformed params | `peer_not_configured`; `forbidden`; `invalid_request`, before any effect | `peer_routes_refuse_before_any_effect` |
| P9 | A pending save with any key but the gateway's own, or for a pin that is the gateway's own key | Refused, nothing written; the second is `peer_own_gateway` | `a_record_takes_only_the_gateways_key_and_an_unreadable_one_can_be_forgotten` |
| P10 | A record of another shape, or a valid one whose `gatewayKey` is not this gateway's | Listed `unreadable`; its credential does not load (`Corrupt`); enrolling into that peer is `peer_exists`, audited `unreadable` before and after, and leaves it as it was; `peer.forget` removes it | same |
| P11 | `peer.forget` while an enrollment runs | `peer_busy`; once the enrollment ends, forget runs | `forgetting_waits_for_a_running_enrollment` |
| P12 | An enroll or forget that succeeds or is refused; its intent, or its outcome, not kept | Intent and outcome kept, naming the operation, peer, address, state before and after, answer, cause and owner; no intent: nothing dialed or removed, `peer_audit_unavailable`; no outcome: the record made or removed stands, `peer_audit_unavailable` | `enrolling_and_forgetting_are_audited_and_answer_only_when_kept`, `the_durable_peer_audit_reports_a_record_it_could_not_keep` |
| P13 | A connect that never answers; one that completes past the deadline | Held while the injected deadline clock stands still; once it passes the connect deadline, `peer_unreachable`, audited, the attempt dropped and nothing saved; a late connection is closed unused, `peer_unreachable` | `a_connect_that_never_answers_ends_when_the_injected_clock_passes_its_deadline`, `a_connect_completed_after_the_deadline_is_refused_and_closed` |
| P14 | Every answer of `peer.enroll` (ok, busy, unreachable, refused code, wrong invitation, own gateway, exists, capacity) and of `peer.forget` (ok, busy, not found) | Exactly one intent and one outcome per call under one operation id, naming the owner, the target, and the peer once known; the table must list every answer | `every_peer_command_answer_keeps_one_intent_and_one_outcome` |
| P15 | The injected entropy for an enrollment fails | `peer_unavailable`, nothing saved, its intent and outcome kept | `an_enrollment_whose_entropy_fails_is_unavailable_and_audited` |
| P16 | The audit never acknowledges an intent, or an outcome | Held while the injected clock stands; once it passes `AUDIT_DEADLINE`, `peer_audit_unavailable`: no intent, nothing dialed; no outcome, the effect stands | `an_audit_that_never_answers_ends_the_call_at_its_deadline` |

## Reading a peer

Part 2b-3 reads what each peer granted. One task per gateway,
`PeerPoller`, started and joined with native pairing, walks the kept peers
one at a time. Each cycle holds the peer commands' turn, but gives it up to
the owner: `peer.enroll` and `peer.forget` stop a cycle that holds it and go
ahead, so a peer, however slow, never keeps the owner waiting.

```mermaid
sequenceDiagram
    participant O as B's owner
    participant P as B's poller
    participant R as B's peer records
    participant U as B's peer audit
    participant A as Gateway A (native listener)
    participant C as B's retained cache of A
    P->>R: list kept peers to schedule (pending or active)
    P->>P: take the turn when free (registered as this cycle's)
    P->>R: read the record again (gone, revoked or unreadable: drop the peer)
    P->>A: pinned status, through PeerConnector under the deadline clock
    A-->>P: Pending / Claimed / Approved: wait an interval
    A-->>P: Terminal or Unclaimed: the client removes the cache, then marks the record revoked
    A-->>P: Active {receiver, epoch}: the client saves the credential into the record
    P->>U: what is held before and after the status, if it changed (approved / ended)
    P->>A: openProduct with B's own key and the issued credential (read budget starts)
    P->>A: catalogueHead (unchanged, and a finished walk noted there: done)
    P->>A: catalogue manifest pass and resolve
    P->>C: what A grants, by its owner's catalogue stream
    P->>A: recordsHead for each cached conversation, while the budget lasts
    A-->>P: wrong_owner: no longer granted
    P->>C: withdraw it (entry and transcript)
    P->>A: record pages for the rest (cache full: heads only, to withdraw)
    C-->>P: ResetRequired or damaged: empty the cache, read once more
    A-->>P: openProduct refused (redacted)
    P->>A: pinned status again; only Terminal ends the enrollment
    O->>P: peer.forget or peer.enroll while the cycle holds the turn
    P->>P: cycle stopped: read sockets shut, its stop flag asked every 50 ms, waits woken, turn given back
    P-->>O: the command runs; the read carries on at the next cycle
    P->>U: each change as it is made (status, reset, conversations withdrawn): queued, in turn order
    P->>P: give back the turn
    P->>U: wait for those records, all within one AUDIT_DEADLINE
```

- **Status first, every time.** No read happens without a fresh Active
  status, and nothing is removed without a Terminal one: a refused
  `openProduct` is redacted, so a revoked credential and a full session pool
  look alike until the status is asked, as a device does.
- **What is read is decided under the turn.** The listing only schedules
  peers. Once a cycle holds the turn it reads the peer's record again, and a
  peer forgotten, ended or unreadable since is dropped, its sync with it, so
  a forget is never followed by a stale `sync` coming back.
- **The core is the device's.** `nessa-client-core::retained` is the one
  public entry the gateway calls: open a cache, read into it, count what it
  holds. It composes the same engine, cache and gateway session the device
  example uses; the crate's `cli` feature now gates only that example's
  entrypoint, not the reading core ([ADR 483](../../adr/done/483-protocol-and-client-core-crates.md),
  amended).
- **Synced means walked.** Whether a cache is up to date is a fact about the
  cache, so the cache keeps it: a walk of every cached conversation that
  finishes notes the catalogue head and progress generation it walked at
  (`retained_walks`). A read skips the walk only when A's head is unchanged
  and that note is there at the same head and generation. A walk cut short
  by the budget, a failure, a stop or a restart notes nothing, so the next
  read walks again and withdraws what it had not reached; a note that
  cannot be written fails the read as the cache's own failure (`quota`, or
  `failed`), backed off like any other, rather than walked again at once.
  One read withdraws at most `WITHDRAWN_PER_READ` (1024) conversations, so
  one cycle's audit records fill at most about half the audit's queue; a
  read that reaches it ends incomplete and the next cycle, soon, withdraws
  the rest. Adding the
  table moved the cache's schema version to 2: a cache of the older version
  is refused (`OutdatedSchema`) and, being derived, emptied and read again.
- **Whose catalogue.** A device's session principal owns the catalogue it
  reads, so it checks the answered scope names exactly that owner. A peer
  signs in as itself and reads an owner it cannot name, the one A bound its
  receiver to, so it checks what it can: the catalogue schema, an owner
  stream's shape (lowercase hex digest), its receiver and its epoch
  (`check_granted_catalogue_scope`). The cache keeps the stream it first
  read, one per receiver; an answer naming another is `ResetRequired`.
- **An unshare withdraws.** A's catalogue answers only rows B is granted, so
  an unshared conversation is absent from the pass, not deleted in it. Each
  read therefore asks every cached conversation's record head, and A's
  `wrong_owner` removes that conversation's entry and transcript together. A
  conversation A deletes arrives as a deleted row and leaves the cache too.
- **A cache that cannot continue is emptied.** One that cannot continue
  against what A now serves (another scope or incarnation, or a head behind
  what it completed: `ResetRequired`), or that is damaged or of a shape this
  build does not read (`Corrupt`, `OutdatedSchema`), is deleted and read
  again from nothing. It is derived from A, so nothing is lost. A full cache
  is not emptied: it lists `quota` and backs off. A full cache, or one a
  write fails on, still withdraws: from then on the read asks each remaining
  conversation only whether it is still granted, so what the peer stopped
  sharing always leaves and frees what it held.
- **Every read has a budget.** `READ_BUDGET` (60 s) on the peer commands'
  injected clock, the one their deadlines and `PREEMPT` use: no physical
  read or write waits past it, and no conversation starts after it. A read
  that reaches it having saved something ends incomplete (`syncing`), and
  the next cycle carries on soon. One that saved nothing made no headway:
  it is `unreachable`, backed off, and its last finished read is not
  refreshed, so a peer that trickles is not read again every few seconds.
- **The owner comes first.** A cycle registers itself with the turn it holds,
  and gives the turn back and clears itself under one lock, so an owner
  command never finds the turn taken with no cycle to stop. An owner command
  that finds a cycle stops it: the read's sockets are shut and its waits
  woken, and the command takes the turn once given back, within `PREEMPT`
  (10 s) on the injected clock. Shutting a socket does not wake a receive
  blocked in another thread on every system (Windows does not), so a read
  never waits on its socket longer than 50 ms at a time and asks its stop
  between. What a stop cannot cut short is an operating-system connect, at
  most the 5 s handshake budget; `PREEMPT` is at least twice that, checked
  when the gateway is built, to leave as long again for local writes. The
  cycle only hands its audit records over under the turn, which queues them
  at once, and waits for them after it gives the turn back, so a slow audit
  never counts against `PREEMPT`. Only another owner command, or a stopped
  cycle that does not give the turn back within `PREEMPT`, answers
  `peer_busy`. The stopped read carries on at the next cycle.
- **Revocation ends reading.** A Terminal or Unclaimed status removes the
  peer's cache and then marks the record `revoked`, both under the records'
  lock in the client's own call, so a stopped poller cannot leave a revoked
  peer's cache behind; a failure between them leaves the record as it was,
  for the next status to end again. The peer is not read again;
  `peer.forget` removes the record.
- **Every change the poller makes is audited.** It changes what is held of a
  peer through three steps only: the status (credential saved, enrollment
  ended), the cache reset, and the read (conversations withdrawn). Each
  step reads the record and whether the cache is there before and after,
  and keeps both with the cause (`peer_approved`, `peer_ended` with the
  status's cause or outcome, `cache_reset_required`, `cache_damaged`,
  `peer_withdrew` with the conversations) and the system as initiator. A
  status step is named by the transition the record's store began (the
  credential's save, or the ending), not by what the record holds after,
  so a write that then fails is still named by what it was; whether it
  succeeded is the store's answer too (`SlotOutcome`), not whether the
  status read then did. The conversations one read withdrew are one
  record, or one per 64 (`WITHDRAWN_PER_RECORD`); they are kept outside the
  read's worker, so a worker that panics still reports what it withdrew.
  Each record goes to `PeerCommands::poller_changed` as the step ends,
  while the cycle still holds the turn, and the audit queues it at once; so
  the records are in the order the turn was held, and a change the cycle
  made comes before the owner command that stopped it. The cycle waits for
  their answers once it has given back the turn, all within one
  `AUDIT_DEADLINE`. An owner command's own record waits behind those
  ahead of it in the queue, within its own `AUDIT_DEADLINE`: a stalled
  store answers it `peer_audit_unavailable`, never holds it longer. A
  record not kept is logged as an error and the change stands: cleanup is
  never held back or undone for its evidence.

```mermaid
sequenceDiagram
    participant P as B's poller cycle (holds the turn)
    participant Q as DurablePeerAudit (sequence, queue)
    participant W as its writer thread
    participant O as peer.forget
    P->>Q: record(PollerChanged): sequence 7, queued at once
    O->>Q: record(ForgetRequested): sequence 8; waits for its answer
    O->>P: stop the cycle
    P->>P: give back the turn, then wait for sequence 7's answer
    O->>O: take the turn, remove the record and its cache
    O->>Q: record(ForgetFinished): sequence 9
    W->>W: writes 7, 8, 9 in that order, answering each
```
- **The evidence is after the fact.** Each record describes a change
  already made: a withdrawal, for one, has already removed the conversation
  from the cache, and its before and after both show what the read left. A
  process that stops between a change and its record, a withdrawal included,
  loses that record; the peer's record and cache on disk say what the change
  came to, as an owner command's outcome does.
- **Cadence.** Each peer is read every `POLL_INTERVAL` (30 s), spread 80 to
  120 percent, and after a failure after a backoff that doubles; every wait,
  jitter included, is held to `POLL_BACKOFF_CAP` (15 min). A read stopped
  at a bound, or by an owner command, continues sooner. Every wait is on the
  injected monotonic clock and the jitter is drawn from injected entropy.
  `POLL_INTERVAL`, `POLL_BACKOFF_CAP`, `READ_BUDGET` and `PREEMPT` are rows of
  [`limits.md`](../../limits.md).
- **`peer.list` shows the reading.** Beside each pending or active peer:
  `sync.state` (`waiting`, `synced`, `syncing`, `unreachable`, `failed`,
  `quota`), the wall time the last read finished and how many conversations
  the cache holds. It is what this process saw since it started; nothing
  persists it.
- **Shutdown leaves nothing running.** Stopping native pairing stops the
  poller first: its wait ends, the cycle holding the turn is stopped (a
  status read dropped, a read's sockets shut), and `join` returns once its
  blocking worker has. Then the peer audit is closed and its writer
  waited for, within one `AUDIT_DEADLINE` on the injected clock.

| Row | Situation | Expected | Test |
| --- | --- | --- | --- |
| R1 | B enrolls into A and A's owner approves | B's poller reads Active, saves the credential and reads; `peer.list` shows `active`, `synced`, no conversations; audited `peer_approved`, pending to active | `a_peer_reads_only_what_it_is_granted_and_stops_at_revocation` |
| R2 | A shares X and not Y with B | B's cache holds X only | same |
| R3 | A shares Y and unshares X | The next read takes both: B's cache holds Y only; X's withdrawal audited | same |
| R4 | A unshares Y and shares nothing else; then shares it again | B's cache empties, Y's withdrawal audited; Y comes back; each finished walk is noted in the cache | same |
| R5 | A shares Z, then deletes it | Z leaves B's cache | same |
| R6 | B's cache records more of A's catalogue than A serves | `ResetRequired`: B empties the cache, audits it, and reads again from nothing; `synced`, holding Y | same |
| R7 | B's cache names another owner stream than the one A answers | The scope pin refuses it: `ResetRequired`, emptied and read again | same |
| R8 | B's cache has a shape this build does not read: an older schema version, as every cache made before walks were noted | `OutdatedSchema`, so `cache_damaged`: emptied, audited, read again | same; `an_older_cache_file_is_refused_as_outdated_schema` |
| R9 | A read runs out of its budget having saved nothing | A failure: `unreachable`, backed off; its last finished read not refreshed; the conversations it held kept. One that saved something ends incomplete, `syncing` | same; `a_read_its_budget_cut_short_counts_only_if_it_saved_something` |
| R10 | The poller stops while a read is held | The read's socket is shut, its stop asked every slice, and `join` returns at once, well before the read's own handshake deadline, on every system | same; `a_stop_ends_a_blocked_read_without_shutting_its_socket`, `a_sliced_wait_lasts_as_long_as_the_caller_set` |
| R11 | A's owner revokes B's credential | B's read is refused, its status is Terminal; the cache is removed and the record lists `revoked`, audited `peer_ended (terminal: credential_revoked)`; no connection reaches A after | same |
| R12 | B's owner forgets A, with a cache left behind | The record and the cache beside it are removed, the cache first, so a failure leaves a record that can be forgotten again | same |
| R13 | B enrolls again and A's owner denies it | The record lists `revoked`, audited `peer_ended (terminal: denied)` | same |
| R14 | B's owner forgets A while a read of A is held | The read is stopped and forget succeeds at once, not `peer_busy`; the record and the cache are gone | same |
| R15 | Rows R1–R14 with an audit that refuses every poller change | Every change still lands; each was offered to the audit once, and refused | `every_poller_change_lands_when_its_audit_is_refused` |
| R16 | A device reads its owner's catalogue; a peer reads a granted one | The device refuses another owner's stream; the peer accepts any owner stream for its receiver and epoch and refuses another schema, receiver or epoch, or a stream that is not an owner's (another prefix, uppercase hex, a conversation id) | `a_catalogue_scope_is_checked_as_its_reader_can`, `a_granted_catalogue_scope_is_checked_for_all_a_non_owner_can_check`, `every_owner_stream_has_the_shape_a_non_owner_reader_checks` |
| R18 | A poller change, then a forget that stops its cycle; a store that stalls on the poller's record; a record handed over and never awaited | Records land in turn order: the cycle's change, then the forget's intent and outcome, sequences 1, 2, 3 ... with observation times in order. With the store stalled the forget's intent waits behind the poller's record and is answered `peer_audit_unavailable` once B's clock passes `AUDIT_DEADLINE`, nothing removed; once the store answers every record lands in sequence and the next forget goes ahead. A record never awaited is still written | `records_land_in_turn_order`, `a_stalled_audit_never_holds_an_owner_command`, `an_unpolled_poller_record_is_still_written` |
| R19 | A cached conversation over quota, then one the peer stopped granting; then a stop or a lost connection | The second is withdrawn; the read ends `quota`, still `quota` when the walk is then stopped or loses its connection; a storage failure is held back the same way, and only a damaged cache or a lost connection stops the withdrawals | `a_full_cache_still_withdraws_what_the_gateway_no_longer_grants`, `a_full_catalogue_still_withdraws_and_quota_outranks_a_storage_failure`, `a_withdrawal_that_cannot_be_written_does_not_stop_the_others`, `a_full_cache_is_listed_quota_and_a_read_that_saved_nothing_unreachable`, `a_held_quota_outlasts_a_stop_or_a_lost_connection` |
| R20 | A status step whose write fails after it began, or lands before the status fails; a cycle giving back the turn as an owner command asks; a read whose worker panics after a withdrawal | Named by the transition begun (`peer_ended` after a failed ending, `peer_approved` after a failed save), its outcome the store's (refused when the write failed, succeeded when it landed, whatever the status said); the owner command finds the turn free or a cycle to stop, never `peer_busy`; the withdrawal is still reported and audited | `a_slot_names_the_transition_a_status_began_whatever_storage_did`, `a_status_change_is_named_by_the_transition_its_store_began`, `a_status_change_outcome_is_what_its_store_did`, `a_cycle_gives_back_the_turn_as_it_clears_itself`, `a_read_that_panics_still_reports_what_it_withdrew` |
| R21 | A walk cut short after its catalogue pass (A unshared Y, the pass reached A's new head, Y not yet asked), then a restart, at A's unchanged head | No walk is noted, so the next read walks every cached conversation and withdraws Y, audited; `synced` only after that, holding nothing; a note that cannot be written fails the read as the cache's failure, backed off | `a_peer_reads_only_what_it_is_granted_and_stops_at_revocation` (the cache put back as the cut walk left it), `a_cache_without_the_walk_marker_is_walked_at_an_unchanged_head`, `a_marker_that_cannot_be_written_fails_the_read`, `a_walk_marker_is_kept_per_catalogue_and_goes_with_a_purge` |
| R22 | The audit queue is full (the store fell behind) | Answered at once, no wait: `peer.enroll` and `peer.forget` are `peer_audit_unavailable` with nothing dialed or removed; a poller change lands and its record is logged as not kept, never written; each refusal uses up its sequence, so the next kept record follows a gap | `a_full_queue_refuses_the_owner_and_drops_the_poller_record` |
| R23 | Native pairing stops with records queued and the store stalled; or its writer panics | The audit is closed and drained within one `AUDIT_DEADLINE` on the injected clock: records before the stall are written, the rest are logged and not, and the writer ends once the stalled write returns; a writer that panicked ends the drain at once, reported as not drained, its unwritten records logged | `close_drains_within_one_deadline`, `a_writer_that_panics_does_not_hold_the_drain` |
| R24 | Native pairing stops while an enroll and a forget have handed over their intents; while a command holds the turn; or a command is asked after | The peer commands are closed before the audit. The first two are waited for, find the turn closed, and answer `peer_unavailable` with their outcomes kept, having read, dialed and removed nothing. A command holding the turn is waited for, within `PREEMPT`, and its outcome kept before the audit closes (past `PREEMPT`, see Known limits). One asked after is refused `peer_unavailable` before its intent, leaving no record | `a_command_admitted_before_shutdown_ends_with_its_outcome_and_no_effect`, `close_waits_for_a_command_holding_the_turn_and_keeps_its_outcome`, `a_command_asked_after_close_is_refused_before_its_intent` |
| R25 | One read finds more conversations withdrawn than `WITHDRAWN_PER_READ` | It withdraws that many and ends incomplete; the next cycle withdraws the rest; one cycle's records stay within about half the audit's queue | `a_read_withdraws_at_most_its_cap_and_ends_incomplete`; the bound on a cycle's records is a compile-time assertion beside `WITHDRAWN_PER_READ` |
| R17 | The wait after each cycle | The interval when settled or pending; an eighth of it when incomplete or stopped; doubling from the interval on each failure in a row; spread 80 to 120 percent; never past the cap; a failing entropy source draws the middle | `failures_double_the_wait_up_to_the_cap`, `jitter_spreads_a_wait_over_80_to_120_percent_and_the_cap_still_holds` and the other tests in `tests/peer_gateways/poller.rs` |

## What this part does not do

H as a whole is larger than one change. Part 1 (#725) is the granting side;
part 2a puts a peer's reads behind its grants; part 2b-1 lets the gateway link
the client and gives the head an access path by receiver
(`read_grant_changes_by_receiver`); part 2b-2 enrolls and part 2b-3 reads
(above). The rest:

- **The peer table's last addresses and local discovery.** The issue lists
  them; the ADR places local discovery in slice I, and the map leaves "whether
  to announce at all, and what it reveals" unresolved. This side's peer table
  is the registry's rows: the principal (named by the key), the pinned key
  (the pairing's claim) and the grants (the credential's). Last addresses
  belong to the side that dials, whose record keeps the one that last
  answered (above).
- **A Share or Linked gateways control on the desktop.** The method and the
  generated client take `enrollee`; the UI is a separate change with its
  browser verification.

## Known limits

- **An enrollment that loses its last reply can leave a pending record.** B
  saves its record before it confirms, as a device does, so a connection
  that fails after that save answers `peer_unreachable` or
  `peer_unavailable` while A may have recorded the claim. The record lists
  `pending`; the poller's next pinned status settles it, and
  `peer.forget` removes it. Enrolling into the same peer again is
  `peer_exists` until then.
- **Shutdown waits for a running command only so long.** Closing the peer
  commands waits within `PREEMPT` for every command already past its
  intent. Any command still holding the turn past that, an enrollment in
  its exchange (its own deadlines are longer) or a forget whose storage
  stalls, goes on until it ends or the process does, and its effect may
  land after the audit has closed, its outcome then not kept; that is
  logged with how many were still running. Its intent is kept, and the
  peer record on disk says what it came to.

- **The grantor's liveness is not a read check.** The grantor of a pairing
  is its initiator: an active admin holding `credential.manage` when it
  paired. No runtime operation disables a membership or takes that role away,
  and the registry refuses a state where its owner is not an active admin
  (row H10). A device stops with its owner because it signs in as them; a peer
  signs in as itself, so admission does not see the initiator's state. A
  change that lets a membership be disabled or demoted at runtime, or gives
  another admin a way to pair, must add the pairing initiator's membership to
  read admission in the same change.
- **A paired reader's head values are the owner's catalogue revisions.** Its
  head moves only with what it was granted, but each value is the owner-wide
  revision of that change, so the gap between two values a peer sees counts
  the owner's other changes in between. Per-reader revision numbering is not
  built.
- **A device paired before this change sees its head go back once.** Its
  saved catalogue progress came from the owner's head, which was at or past
  its granted head. Its next pass finds the head below what it completed, and
  the sync engine answers `ResetRequired`; the client does not reset on that
  by itself, so the device's catalogue cache needs an explicit reset once.
  Changing the scope or incarnation such a reader sees would hit the same
  refusal (a scope mismatch is also `ResetRequired`), so it is not done.
- **A peer read's TCP connect waits in real time.** The status read goes
  through `PeerConnector` under the deadline clock; the read itself uses the
  device's gateway session, whose connect is an operating-system connect.
  Its timeout is computed from the injected clock (the handshake deadline or
  the read budget, whichever is sooner), but the wait itself is the
  operating system's, and a stop reaches the sockets a read has open, not
  one still connecting. A connect cannot be cut into slices the way a
  receive is (each try would start over, and a slow link would never
  connect), so a stop or an owner command can wait for it, at most the 5 s
  handshake budget, which `PREEMPT` covers twice over.
- **A budget-bound read can starve its last conversations.** Each read asks
  the cached conversations in the same order and stops starting new ones at
  its budget, so under constant change in the first ones, the last ones are
  read only once those settle. Rotating where a read starts is not built.
- **A full cache is not tested end to end.** `quota` is reported from the
  cache's own refusal; filling a 256 MiB cache in a test is not done. The
  walk that still withdraws past it is tested against scripted answers
  (row R19).
- **A budget hit after partial progress is not tested end to end.** Which
  way a budget-bound read ends is tested against each case (row R9); an end
  to end read cut by its budget after saving some pages needs a peer that
  stalls mid-connection, which the relay does not do.
- **An audit sequence counts within one process.** It starts at 1 each
  time the gateway starts; across restarts `observedAtMs` orders the runs.
  A gap within a run is a refused record, named in the log.
  A record still queued when shutdown's drain runs out is not written
  (logged by sequence), and one the writer had begun may still land.
- **A cache removal not confirmed durable is not tested.** It needs a
  directory sync that fails, which the private storage owner does not let a
  test inject.
- **A peer has no catalogue watch.** It polls its catalogue head. A watch
  that wakes only on changes to its own granted rows needs per-receiver
  notices, which nothing builds yet.

- **The owner's Settings › Linked devices lists a peer's credential** among
  the devices, since both are pairing-bound credentials. Telling them apart
  in the list is part of the desktop control above.
- **Linked devices also shows a claimed peer invitation as an ordinary device
  awaiting approval**, with no class shown. This is a labelling gap, not a
  security one: the class is fixed when the invitation is created and bound
  into the exchange, so approving it yields only the read-only gateway
  credential, never a device credential that names the owner.
