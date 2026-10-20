# Device client core

Native device enrollment and retained gateway reads, independent of the gateway
runtime. The standalone `read_only_sync` example composes this crate. A gateway
links it as a peer's client without the `cli` feature (on by default), which
holds the example's command line and the retained-sync composition behind it;
gateway process tests enable `cli` to drive that entry point.

```text
example -> composition -> pairing + read_only_sync -> nessa-protocol
native library ---------> pairing + retained ------> shared drivers/cache/fold
product child ----------> shared protected channel -> nessa-protocol
nessa-server ------------> pairing + retained (no cli)
nessa-server tests -------> public client entry point (cli)
```

Arrows are construction and calls. The client imports no gateway implementation;
`PACKAGE_DENYLISTS` in `scripts/architecture/rust-dependency-graphs.mjs` rejects
any resolved client dependency path to `nessa-server`. The portable dependency
gate also rejects desktop frameworks. Domain/application imports are checked by
`node scripts/check-architecture.mjs`; application code consumes pure protocol
outcome values through `product_contract`, not transport DTOs through `product`.

The independent `product::{ProductSession, ProductTicket, ProductSettlement}` API
owns one bounded physical protected pump. `prepare` captures original private
identity inputs with no I/O, permitting parent registration before `start`.
Opening reuses the retained session's canonical pinned OpenProduct handshake,
with Panel metadata and actual Ready methods. Its injected connector supplies a
separate interrupt; `request_close` fences/wakes without the sole reader's lock,
and `close` joins it. Request permits cover queued/in-flight/settled tickets until
physical settlement and final ticket/settlement-observer release. Completions keep
correlated gateway success/error separately from not-dispatched or uncertain
physical failures. A failed injected wake requests stop; ticket observation then
waits on private synchronization for the physical owner's original settlement,
without inventing dispatch evidence or declaring drain. Decrypted partial product frames have a total deadline; idle
TLS control bytes do not. This transport does not grant OwnerPhone permission or
change retained-cache authority.

The public enrollment/retained API is `pairing::{NativeEnrollmentClient, NativeClientError,
NativeRetryOutcome}`, `retained::{RetainedCache, ReaderAccess, ReaderRole, ReadStop,
ReadReport, ReadFailure, ConversationFeed, FeedEvent, SubscribeError}` and its typed saved catalogue/transcript/progress values,
`outbox::{PhoneOutbox, PhoneIntent, PhoneOutcome, IntentStore, IntentEvent, StoreError, open}`,
and, with `cli`, `composition::{execute, run_read_only_example}`.
Gateway enrollment tests use the former; gateway online tests/child fixtures use
`execute`; the example uses `run_read_only_example`. The crate root publishes an
immutable diagnostic `CommandError` retaining the
original typed cause privately. Debug/Display remain unchanged; JSON owns
machine-readable outcomes. `NativeRetryOutcome` exposes borrowed receipt
accessors. Retained driver/cache mutation, policy and reset ports remain internal;
saved values expose actual progress without claiming current source authority.
`RetainedEnrollmentClient` composes the existing physical enrollment worker with
the purge-before-end policy. Its immutable status result captures that operation's
purge inside physical admission; an outer worker/admission error does not prove
cleanup absent. `RetainedCache::purge_receipt` reads original durable evidence
without a credential or another status RPC. Credential-removal acknowledgement
is distinct from a successful purge, including uncertain unlink and worker faults.
The private store/cache scope remains stable until shutdown drains the owner.
`ReaderRole` chooses the existing owner or granted-peer scope authorizer; it does
not grant permission. `saved_catalogue_scope` discovers a saved target, and
`catalogue_page` selects an explicit receiver/origin/stream in one SQLite snapshot.
Each page carries scope/progress/generation: callers compare them across pages,
including reset/deletion between requests. Owned transcript views do not update
after withdrawal; the presentation revision is distinct from physical progress.
`RetainedTranscriptSnapshot` keeps its progress/view pair private and exposes
borrowed accessors; its compile-fail example checks that the view cannot be mutated.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/product.rs`, `src/product/` | Bounded native product pump, injected socket/interrupt/wake ports, actual ready and original ticket/settlement handles. No bootstrap proxy, retry or second method-schema owner. |
| `src/protected.rs`, `src/protected/deadline_stream.rs` | Shared incremental frame/TLS transport and first typed physical failure; distinct terminal retained deadlines and finite product poll ticks. |
| `src/pairing/owner_phone.rs` | Nonnetwork original preparation/restoration through Auth private-store/crypto owners; separate initial and restore APIs, no replacement key for missing/incomplete originals. No remote approval or issuance. |
| `src/outbox/domain/` | One typed phone send, its canonical bytes, and the outcome a gateway answer becomes. No store and no socket. |
| `src/outbox/application/` | Save the send, ask `conversation.send`, and read `conversation.receipt` before any later send. The store and the gateway are ports. |
| `src/outbox/infrastructure/` | One private SQLite file for the command and its audit row, and the product frames for those two calls. |
| `src/pairing/client.rs` | Native enrollment, strict pinned status and retry, pending-state publication and one retained physical worker. Shared framing and enrollment socket policy come from `nessa-protocol::pairing`. |
| `src/retained/device.rs` | Library composition of that physical worker and the existing purge policy; immutable admission-scoped status/purge results and durable purge receipt lookup. No CLI or second worker. |
| `src/retained.rs`, `src/retained/saved.rs`, `src/retained/subscribe.rs` | Public bounded owner/granted checkpoint read over the existing driver, plus typed saved scope/page/transcript/progress projections delegating to the same SQLite and passive-view owners. `subscribe` follows one conversation with `conversation.subscribe` on a new pinned session to the same gateway and does not write a checkpoint. No CLI or additional receiver. |
| `src/retained/phone.rs` | Publication lease and catalogue presence. A stamp reads only the receiver and generation it was bound under. Presence is one stored row or fence; a missing row is not a tombstone. |
| `src/retained/artifact.rs` | One held-file fetch through the existing range owner. A file already at the output is a local match and is not asked. Gateway freshness is a digest-matched part. Busy retries wait on an injected `Pause`; `ThreadPause` is the operating-system wait. |
| `src/read_only_sync/domain/` | Immutable attributed cache reset requests; a held file named as the view names it, and its ranges. |
| `src/read_only_sync/application/` | Cache policy and typed evidence, finite records/catalogue drivers, saved-read/reset/purge ports, protected connection policy, bounded watch loop, and the held-file fetch (ranges, at least one asked every time, busy retries, size and digest before keeping). |
| `src/read_only_sync/infrastructure/cache/` | One private SQLite owner for catalogue values, physical records, SDK checkpoints, progress and purge/reset receipts. |
| `src/read_only_sync/infrastructure/gateway/` | Protected pinned session, codecs, sources (`artifact.read` ranges included), and operation deadline/cancellation adapter. Its post-I/O/first-cause policy differs from enrollment's phase/wake socket. |
| `src/read_only_sync/infrastructure/held_file.rs` | The output and the `OUT.<key>.part` a fetch assembles in (keyed on conversation, digest, media type and size), held by one fetch at a time, re-read and checked from disk, synced and hard-linked at `OUT` (the link refuses anything there); an existing `OUT` looked at without following links. |
| `src/read_only_sync/entrypoint/` | Argument contract and JSON output over supplied handles. |
| `src/composition/` | Private profile, device/status admission, protected session and cache construction, independent wall/monotonic adapters, `fetch` (an existing output answered before dialling, then one held file into its part), immutable command diagnostic facade, and executable input/output. The held-file retry pause is `retained::ThreadPause`, re-exported here. |
| `examples/read_only_sync.rs` | Standalone executable using `composition::run_read_only_example`. |
| `tests/protected/` | Shared physical deadline/first-cause and cancellation evidence for the protected byte stream. |
| `tests/product/` | Real pinned TLS opening, idle/partial traffic, original request IDs, permit retention, typed deadlines/failures and independent physical drain. |
| `tests/outbox/` | Phone outbox ordering, canonical bytes, and product-frame evidence, included by the library with `#[path]`. `departure.rs` selects initial-read and lease orderings; `lease_wait.rs` tests real SQLite bounded lock refusal. |
| `tests/read_only_sync/` | Matching domain, application, gateway and private-cache boundary evidence, included by the library with `#[path]`. |
| `tests/composition/` | Private profile, code-input, command diagnostic, and composed `fetch` (scripted native peer, real files) evidence. Real authenticated online process tests remain in `nessa-server/tests/composition/read_only_online.rs`. |
| `tests/pairing/owner_phone.rs` | Real private-key publication, exact signed original/digest replay, cold corruption, reserved/missing refusal, before-entropy ordering and concurrent first reservations. |
| `tests/pairing/completion.rs` | Physical admission through effect capture, rejected Busy admission, caller/shutdown cancellation and last-handle-drop ownership. |

The deterministic five-mode saved-output race is
`tests/read_only_sync/infrastructure/saved_output.rs`. It uses the existing
private cache scheduling hook and production output mapping; the public
composition API carries no test hook. Protocol schemas, storage representations,
phase budgets and wire versions are unchanged by the crate extraction.

Build the example with `cargo build -p nessa-client-core --example read_only_sync`.
See [the example design](../../docs/design/read-only-sync-example.md) for its
commands and separate captured-check/durable-progress contract.
