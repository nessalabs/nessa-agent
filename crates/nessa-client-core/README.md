# Device client core

Native device enrollment and retained gateway reads, independent of the gateway
runtime. The standalone `read_only_sync` example composes this crate. A gateway
links it as a peer's client without the `cli` feature (on by default), which
holds the example's command line and the retained-sync composition behind it;
gateway process tests enable `cli` to drive that entry point.

```text
example -> composition -> pairing + read_only_sync -> nessa-protocol
nessa-server ------------> pairing (no cli)
nessa-server tests -------> public client entry point (cli)
```

Arrows are construction and calls. The client imports no gateway implementation;
`PACKAGE_DENYLISTS` in `scripts/architecture/rust-dependency-graphs.mjs` rejects
any resolved client dependency path to `nessa-server`. The portable dependency
gate also rejects desktop frameworks. Domain/application imports are checked by
`node scripts/check-architecture.mjs`; application code consumes pure protocol
outcome values through `product_contract`, not transport DTOs through `product`.

The public API is `pairing::{NativeEnrollmentClient, NativeClientError,
NativeRetryOutcome}` and, with `cli`, `composition::{execute, run_read_only_example}`.
Gateway enrollment tests use the former; gateway online tests/child fixtures use
`execute`; the example uses `run_read_only_example`. The crate root publishes an
immutable diagnostic `CommandError` retaining the
original typed cause privately. Debug/Display remain unchanged; JSON owns
machine-readable outcomes. `NativeRetryOutcome` exposes borrowed receipt
accessors. Retained-sync modules, SQLite construction/mutation,
cache policy/progress and reset ports/requests remain internal.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/pairing/client.rs` | Native enrollment, strict pinned status and retry, pending-state publication and one retained physical worker. Shared framing and enrollment socket policy come from `nessa-protocol::pairing`. |
| `src/read_only_sync/domain/` | Immutable attributed cache reset requests. |
| `src/read_only_sync/application/` | Cache policy and typed evidence, finite records/catalogue drivers, saved-read/reset/purge ports, protected connection policy and bounded watch loop. |
| `src/read_only_sync/infrastructure/cache/` | One private SQLite owner for catalogue values, physical records, SDK checkpoints, progress and purge/reset receipts. |
| `src/read_only_sync/infrastructure/gateway/` | Protected pinned session, codecs, sources, and operation deadline/cancellation adapter. Its post-I/O/first-cause policy differs from enrollment's phase/wake socket. |
| `src/read_only_sync/entrypoint/` | Argument contract and JSON output over supplied handles. |
| `src/composition/` | Private profile, device/status admission, protected session and cache construction, independent wall/monotonic adapters, immutable command diagnostic facade, and executable input/output. |
| `examples/read_only_sync.rs` | Standalone executable using `composition::run_read_only_example`. |
| `tests/read_only_sync/` | Matching domain, application, gateway and private-cache boundary evidence, included by the library with `#[path]`. |
| `tests/composition/` | Private profile, code-input and command diagnostic evidence. Real authenticated online process tests remain in `nessa-server/tests/composition/read_only_online.rs`. |

The deterministic five-mode saved-output race is
`tests/read_only_sync/infrastructure/saved_output.rs`. It uses the existing
private cache scheduling hook and production output mapping; the public
composition API carries no test hook. Protocol schemas, storage representations,
phase budgets and wire versions are unchanged by the crate extraction.

Build the example with `cargo build -p nessa-client-core --example read_only_sync`.
See [the example design](../../docs/design/read-only-sync-example.md) for its
commands and separate captured-check/durable-progress contract.
