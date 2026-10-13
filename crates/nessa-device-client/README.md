# Native device client

This foreign adapter links `nessa-client-core` without its CLI feature. UniFFI
exports generate Swift, C headers and the modulemap from the compiled library.
The `bindgen` feature is a host tooling dependency, excluded from normal native
runtime builds.

Current development surface:

- `saved`: one off-UI historical cache selector and immutable transcript/scope
  objects. The existing core owns SQLite reads, physical progress and passive
  projection. No second checkpoint or fold is implemented here.
- `infrastructure`: wall and monotonic clock adapters supplied by the foreign
  composition entry point. Tests substitute clocks through `with_clocks`.
- `session`: one private scope, injected clocks/socket/entropy inputs, and one
  physical admission through pinned status, current binding, read and refusal
  recheck. `request_close` closes admission/wakes sockets; `close` drains and
  releases private state. Saved selection uses issued receiver and actual scope.
- `native_session`: generated immutable session/receipt/refresh/read/catalogue
  objects. Failure objects retain publication, cleanup and reconciliation payloads.
- `tests/saved`: actual cache recreation, typed invalid selection, direct-call
  admission and checked protocol-bounded serialization.

The explicit saved selector grants no connection, enrollment or command
permission. It is not the phone repository. The native session and compound
refresh API have run against paired records through generated Swift on macOS.
`swift/NativeSessionWorker.swift` supplies pre-queue admission, cancellation latches
and a shared physical close task around generated calls. The release Apple package
builds and links for device/simulator; its session runs in the simulator and its
macOS static package passes the paired/offline/revocation fixture. Nessa's actual
retained repository, remote artifacts and durable uncertain deletion remain unfinished
under issue 782. Enrollment retry returns its original receipt only when the
whole retry succeeds. An outer status failure cannot establish absence of purge;
recover durable receipts using the previously captured receiver separately.
Caller composition must keep the eventual enrolled session's immutable private
scope and fence historical snapshots on terminal status; a retained snapshot does
not become fresh or revoke itself after it escapes.

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
