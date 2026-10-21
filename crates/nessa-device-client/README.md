# Native device client

This foreign adapter links `nessa-client-core` without its CLI feature. UniFFI
exports generate Swift, C headers and the modulemap from the compiled library.
The `bindgen` feature is a host tooling dependency, excluded from normal native
runtime builds.

Current development surface:

- `saved`: one off-UI historical cache selector and immutable transcript/scope
  objects. The existing core owns SQLite reads, physical progress and passive
  projection. No second checkpoint or fold is implemented here.
- `serialization`: one capped JSON-copy writer for saved/product views, consuming
  each owning protocol publication; copy refusal retains original native facts.
- `infrastructure`: wall and monotonic clock adapters supplied by the foreign
  composition entry point. Tests substitute clocks through `with_clocks`.
- `session`: one private scope, injected clocks/socket/entropy inputs, and one
  physical admission through pinned status, current binding, read and refusal
  recheck. `request_close` closes admission/wakes sockets; `close` drains and
  releases private state. Saved selection uses issued receiver and actual scope.
- `native_product`: immutable actual Ready, native-ID tickets, full response/stage
  completions and delivered event batches over core's sole protected product pump.
  Completion/batch objects retain core count/byte permits until final handle release.
  Relative event waits capture one deadline on the native clock. Product waits run
  independently of retained admission and of submission/close dispatch.
- `native_session`: generated immutable session/receipt/refresh/read/catalogue
  objects. Failure objects retain publication, cleanup and reconciliation payloads.
  `close_observed` returns physical release separately from its original typed
  failure, derived after actual cleanup under the existing driver lock.
- `native_owner_phone`: separate initial-original preparation and restoration over
  core's canonical durable producer. Immutable public original/key/signature/digest
  getters expose no seed or bearer, and never establish enrollment or Ready.
  Complete discovery conversion uses the protocol mapper and bounded unique JSON.
- `native_owner_recovery`: immutable actual pinned original observations and
  independent local adoption/purge failures. Recovery alone performs no adoption;
  fresh status explicitly settles through Auth's combined original/enrollment owner.
  Reader/product selection reloads and correlates the actual issued record's key,
  pin, complete public lineage, credential and receiver before using that same record.
  Missing original history refuses OwnerPhone records rather than substituting the
  current DeviceRead capability. Tests live in `tests/session/owner_recovery.rs`.
- `tests/product`: actual saved issued key/pin/CredentialId and pinned TLS fixtures;
  parent installation, replacement, poison teardown and private-lock physical drain;
  escaped foreign facts, retained permits and native event timeout mapping.
- `tests/saved`: actual cache recreation, typed invalid selection, direct-call
  admission and checked protocol-bounded serialization.

The explicit saved selector grants no connection, enrollment or command
permission. The phone publication lease (`PhoneLease`) is what a later saved
read or held-file fetch must present. Saved catalogue and transcript reads on
the session go through that stamp; there is no unstamped session entry point. Session composition mints that lease's
instance and its unexported identity from the injected entropy; a source that refuses opens no session.
A published lease id is not itself a stamp.
A stamp from an earlier receiver, or from an earlier session's lease, reads
nothing and does not dial. A successful purge releases that receiver even when
the status carries no observation. A later bind refuses that receiver while the purge receipt remains, including when the credential itself is still stored. A cache path whose metadata cannot be read is a cache failure and does not bind. Catalogue presence reads one stored identity;
absence is not a tombstone, from one snapshot of the fence and the row.
Held-file freshness is either a local digest match, with no range asked, or a
gateway confirmation of the part on disk. Elapsed time is the session clock,
including the status read when the file is not already local. A fault also
reports the bytes the part had already resumed. A malformed digest, media type,
or size is an invalid argument on the native call and does not dial. Busy-range
retries wait on the pause the session was composed with. A stop during that wait
returns within one stop slice and does not ask the next range. A spent fetch budget
ends that wait the same way and is unreachable. A gateway refusal, including a
handshake that refuses before a fetch exists, asks status again even when stop
arrives after that refusal settled. The initial status and that recheck are
returned with the fetch, including a terminal status that selected no reader.
A local fault already produced stays that
fault. A stop during the product handshake is a session stop. A stop during the
part sync does not link the output. A budget that expires after the last range,
or during that sync, is unreachable and does not link. A budget that expires
after that sync check and before the link starts does not link either. A budget that expires
while the hard link holds the gate leaves the output published. Once the hard link exists, a failure to remove
the part or to sync the directory reports that the output exists. The native session and compound
refresh API have run against paired records through generated Swift on macOS.
`swift/NativeSessionWorker.swift` supplies pre-queue admission, cancellation latches
and a shared physical close task around generated calls. The release Apple package
builds and links for device/simulator; its session runs in the simulator and its
macOS static package passes the paired/offline/revocation fixture. Saved
publication and held-file fetch go through the phone lease. Enrollment retry returns its original receipt only when the
whole retry succeeds. An outer status failure cannot establish absence of purge;
recover durable receipts using the previously captured receiver separately.
Caller composition must keep the eventual enrolled session's immutable private
scope and fence historical snapshots on terminal status; a retained snapshot does
not become fresh or revoke itself after it escapes.

Product opening installs a strong provisional child before dialing. Parent close
signals and physically joins that child before releasing its private-state owner;
raw parent Drop applies the same drain, including teardown after a poisoned admitted
operation. New admission remains refused after damage. An escaped child retains its
own core resources and native clock, without retaining its parent. Actual Ready and
completed facts remain historical after closure; requests stay fenced. The protected
transport uses the saved issued public CredentialId with the private key, not an
owner bearer proxy. Existing DeviceRead records gain no write permission from this
adapter. OwnerPhone original preparation now has separate initial/restore calls;
restoration never creates a replacement key for missing or Reserved history. The
same private owner retains admitted work through its physical publication, including
late facts after closure. The native consumer now performs read-only original
recovery and explicit local adoption over pinned compact TLS, with ordinary
completed observations retained beside later typed failures. An outer worker fault
leaves completion/effects unconfirmed; it does not prove that publication was absent.
Application authorization and Send wiring remain separate #811 slices. The changed
ABI still needs generated Apple declaration/package verification before adoption.
Swift product-wait integration also remains separately reviewed consuming work. Do not place a blocking product waiter on its
submission/retained/close queue.

A physical close observation can retain Product or Damaged failure while confirming
actual release. An unknown prior unwind with a poisoned empty driver slot remains
unconfirmed; slot absence alone cannot establish destruction. Escaped observations
retain value facts without retaining the parent. Signal close before waiting for
this blocking receipt off UI, and require its confirmed release before replacing
scope when the normal throwing close reported a historical fault.

Build/generate on a host:

```
cargo build -p nessa-device-client
cargo run -p nessa-device-client --features bindgen --bin nessa-device-bindgen -- \
  generate --library target/debug/libnessa_device_client.dylib \
  --language swift --out-dir target/native-bindings
```

Standalone `swiftc` must receive the generated modulemap explicitly with
`-Xcc -fmodule-map-file=...`; framework packaging carries that file in its headers.
Use a dedicated worker for blocking constructors and cache methods. Never infer a
receiver or origin from the endpoint address, and never use display revisions as
physical progress. Query revisions are caller-owned presentation identity only.
For raw generated calls, call `request_close` before queueing close, await `close`
on the worker, and only then release/replace its handle. Dropping an unclosed raw
handle can block while the session drains. The Swift worker performs that ordering
and keeps abandoned handle deallocation on its serial queue; await successful close
before replacing a scope. `check-worker.swift` verifies these dispatch obligations
with actual private-state locks and held worker calls, not a physical phone microphone
or network cancellation experiment.

`scripts/build-native-device.sh <fresh-output>` builds a local Swift package with
generated sources and a release XCFramework for arm64 iOS, arm64 simulator and the
current macOS host. Run from a frozen checkout with Apple Rust targets/Xcode installed.
This build tool does not configure enrollment or install the package into Nessa.
