# Completing linked-device synchronization

Status: active integration plan for [#257](https://github.com/nessalabs/nessa-agent/issues/257),
after [PR #354](https://github.com/nessalabs/nessa-agent/pull/354) merged as
`c2f3b0ec3faf7b6ecd7020d63c640b86685e4126`.

This plan names the relationships between the remaining slices. It does not
declare their proposed interfaces implemented. The
[coding standards](../../CODING_STANDARDS.md),
[architecture](../ARCHITECTURE.md), and
[dependency injection](dependency-injection.md) remain their existing owners.
Feature state tables belong in the corresponding feature design before code.

## Lessons from the read-path work

Most consequential findings involved one request crossing multiple owners:
admission, physical source work, response delivery and cleanup. A response timeout
did not mean the physical read had finished. A test of a queued response did not
prove that an earlier socket-capacity refusal reached that queue. A slow authority
refresh could hide completed source work from response reaping. Individually valid
client and server timeouts also disagreed when used together.

The implemented corrections share the actual socket/global read lease, put decoded
passive refusals under one response deadline owner, publish phase bounds through
the product schema, and poll authority refresh independently. Their exact
orderings and regression evidence belong in
[authorized record reads](authorized-record-reads.md). Those guarantees are
consumed by later transports; they are not independently reimplemented there.

The long assembled branch also overlapped incoming MCP changes in composition,
service and projection. Current work therefore splits producer behavior from
consumer activation, with one editor for shared socket/schema/composition seams.
Each consumer names its actual producer commit. An old-head check or an isolated
adapter test cannot establish the assembled contract.

Verification had separate problems: painted loading geometry differed from layout,
development reloads interfered with browser runs, and rounded performance output
could conceal an over-budget sample. The retained failures and their scope matter.
[Verifier issue #369](https://github.com/nessalabs/nessa-agent/issues/369) and
[desktop drag issue #370](https://github.com/nessalabs/nessa-agent/issues/370) are
separate work. This plan makes no performance or skills-prevention claim.

## Existing contract owners

| Relationship | Owner | Later consumers |
| --- | --- | --- |
| Durable records and semantic application | SDK session storage and shared committed transcript fold; `nessa-sync` validates physical pages and pass relationships | Hints and relay prompt current reads. Neither moves downloaded/applied progress or constructs execution authority. |
| Receiver, owner, credential and epoch | `nessa-auth` committed access decisions and server passive-read admission | Protected transport and each hint delivery ask current owners; a connection, cache, manifest or copied record is not permission. |
| Admission, physical completion and delivery | Product socket, tracked read workers and actual read lease | Native and relay preserve actual retention through cancellation and timeout, then join before storage cleanup. |
| Wire limits and phase deadlines | Product schema and generated publications | Protected framing consumes directional publications; enrollment/handshake remain separate phases. Whole-run budgets and memory claims need their own evidence. |
| Cache, checkpoints and freshness | Private receiver transactions and SDK committed status | Live scheduling keeps finite captured passes and current connection generation. A quiet hint stream does not establish current data. |
| Commands and exact-turn effects | SDK admission/submission coordinator and [ADR 0008](../adr/todo/0008-agent-client-api.md) | Receipt lookup is read-only; outbox retries consume durable results instead of redispatching uncertain work. Current identity differences must be reconciled at this owner before changing the contract. |
| File permission and deletion | Attachment holds, conversation ownership and deletion | Artifact adapters use existing policy. A transcript file link or digest alone cannot grant a content read. |

## Dependency slices

| Work | Producer slice | Consumer activation and finish line |
| --- | --- | --- |
| [#264/#265](https://github.com/nessalabs/nessa-agent/issues/264) pairing and direct connection | Integrate the preserved native implementation with actual merged record/MCP owners; settle typed representation and authenticated framing | Protected reads, wrong-key/replay/revocation cases, physical shutdown, revised finite allocation ceiling and supported platform evidence. Enrollment tests alone do not finish this. |
| [#298](https://github.com/nessalabs/nessa-agent/issues/298) change hints | Bounded notifications at actual record/catalogue commit owners, with owned registration and cleanup | Generated wire hints, then watch-before-recheck receiver catch-up. Lost hints, reconnect, slow delivery and revoke orderings use actual public routes. |
| [#277](https://github.com/nessalabs/nessa-agent/issues/277) shared fold adoption | Already implemented in the merged read path | Remaining replay-to-live equality is proved with #298; no additional live/phone reducer. |
| [#262](https://github.com/nessalabs/nessa-agent/issues/262) scheduling | Host-owned finite policy over injected wake, lifecycle and link ports | Real encoded bytes and applied lag under foreground/background, metered, sleep and reconnect. Secure device acceptance consumes #264/#265 and live catch-up. |
| [#266](https://github.com/nessalabs/nessa-agent/issues/266) relay | Settled direct identity/envelope/framing contract | Forwarding without source authority or an offline feed; direct/relay checkpoint convergence and honest sleeping-source state. |
| [#268](https://github.com/nessalabs/nessa-agent/issues/268) receipts and Stop | Canonical command identity, durable acceptance and provider-free receipt lookup | Exact-turn Stop, duplicate/conflicting requests, lost reply and restart evidence. Product publications follow the actual SDK producer. |
| [#269](https://github.com/nessalabs/nessa-agent/issues/269) outbox | Settled receipt identity/result contract | Persist intent before send, query after uncertainty, preserve unknown outcomes and avoid repeated execution. |
| [#273](https://github.com/nessalabs/nessa-agent/issues/273) artifacts | Reusable artifact ports adapted to actual file/hold identity, revision and permission | Paired verified resumable transfer, deletion/revocation and availability. Produced files need an explicit authority owner. |
| [#292](https://github.com/nessalabs/nessa-agent/issues/292) oversized semantic groups | Explicit semantic commit boundaries and exact generation/prefix retry | Source/reopen must not expose an incomplete decision group as acknowledged. Coordinate record-writer edits with source notifications. |
| [#271/#272](https://github.com/nessalabs/nessa-agent/issues/271) export and restore | Inventory actual writers and a consistent cut; offline export can be an honest first slice | Online quiescence needs more than a command-admission guard. Restore consumes authenticated manifests and deletion inventory, creates new trust and quarantines unknown history. |

## Contracts to settle before dependent activation

### Native shutdown, representation and sizing

The native lane preserves both existing histories and resolves shared files by
owner. Its canonical pairing design owns combined reader/native/product drain,
retirement, storage, MCP and final reconciliation order. Known outcomes are
published before the next await. Failed original retirement leaves storage not
started; a later retry cannot replace that first evidence. MCP call return keeps
its existing unit-return meaning rather than inventing physical confirmation.

The existing synchronous WebSocket example facade is private and is not a secure
native API by declaration. Protected transport must enter its larger frame phase
through real authentication and preserve current operation authorization. The
approved immutable response wrapper needs typed decoding without a naked mutable
value escape.

The provisional native 128 KiB whole-owner target is not accepted: a permitted
131072-byte response alone fills it before TLS/parser/decoded allocations. The
pairing design already allows revised finite composition sizing from actual
maximum-valid and retained-lifetime evidence. Shared wire bounds stay with their
published owner; no consumer quietly lowers them to make a measurement pass.

### Source watches and the finite live driver

There is no implemented payloadless commit-watch port in the merged source.
An event-stream subscription carrying `Arc<Record>` is not assumed to be a free
hint source. The first watch producer must attach to actual durable publication,
coalesce without holding writer acknowledgement on receiver delivery, and retain
its registration through actual cleanup. Cross-process changes require an actual
notifier or explicit head-check recovery.

The watch design owns bounded targets, capacity, duplicate registration,
acknowledgement-before-activation, current delivery authorization and pending plus
in-flight accounting. Hints carry neither records nor cursors. The existing
source/delivery owner is consumed rather than bypassed by a generic event queue.

A live connection has a different lifetime from one finite catch-up operation.
The current whole-callback budget cannot simply wrap an infinite loop. One
physical reader multiplexes replies and validated hints; coalesced hint state is
separate from unexpected-event capacity. Source passes retain their captured
target, and a later pass handles new writes. Scheduler policy and measured
fallback behavior remain host-owned.

### Receipts and consistent export

The current SDK recovers submissions by `execution_id` and compares saved
request, actor and submission mode. ADR 0008 describes a mutation `requestId`
and an accepted `turnId`; old issue wording and namespace assumptions do not
resolve this implementation difference. The command producer settles lookup
identity, immutable operation/target/bytes/origin and accepted-turn mapping
before outbox or native mutation activation. No second receipt journal is added.

ADR 0008 owns the
[request namespace, binding and acceptance contract](../adr/todo/0008-agent-client-api.md#identity-durability-and-failure-behavior).
The producer still needs concrete source interfaces and state/order evidence
before changing source behavior; the other feature designs link to that owner.

Command admission is not the complete writer inventory. Queued runs, provider
callbacks, audit writes and deletion work can survive a command's return.
Permanent storage shutdown is not a reversible export lease. An online export
must join its actual owners; an offline first slice cannot be reported as that
online guarantee. Receiver outboxes belong to their hosts, not implicitly to the
gateway's export.

## Parallel ownership

The first lanes use separate worktrees based on the actual merged handoff:

| Lane | Owned edit scope | Shared integration |
| --- | --- | --- |
| Native connectivity | Existing native/auth lifecycle, representation and direct transport; named merged-source adaptations | One native integrator resolves its existing socket/state/schema/generator/composition overlaps. |
| Committed-change watches | SDK source notification and catalogue publication; server watch application/adapters, tests and feature design | No concurrent socket/schema/native/session edits. Wire activation follows the producer commit and the connectivity handoff. |
| Artifact integration | Attachment/file policy adapters, content fixtures and feature contract | Product schema/socket and secure transport activation are queued under the same integration owner. |

Scheduling and outbox can develop behind settled producer ports. Relay, online
export and restore activate after their producer contracts. Every slice has an
issue and a stated finish line; preparation or an inert primitive does not close
the parent. Final integration checks name the assembled tree and the consuming
behavior, preserving earlier source and environment attribution.
