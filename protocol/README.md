# Protocol — S1 (connect + health)

Wire contracts for the **current** spike only: WebSocket handshake and `server.health`.

| Path | Purpose |
| --- | --- |
| [manifest.json](manifest.json) | **SSOT for method/event names**, roles, params/result refs |
| [schemas/v1/](schemas/v1/) | **SSOT for payload shapes** (JSON Schema) |
| [defaults/](defaults/) | Default documents the server serves (e.g. `shortcuts.v1.json`) |
| [fixtures/v1/](fixtures/v1/) | Golden wire examples (CI-validated) |

```bash
pnpm protocol:generate   # → TS + Rust catalogs and payload types
pnpm protocol:check      # manifest + fixtures + generated parity
```

**To add a method:** edit `manifest.json` + schemas → `pnpm protocol:generate` → wire the handler. Do not invent string literals in client/server code.

Generated (do not edit — each file header names its SSOT):

| Output | From |
| --- | --- |
| `packages/nessa-client/src/generated/protocol.ts` | schemas (payload types) |
| `packages/nessa-client/src/generated/catalog.ts` | manifest (`Method.ServerHealth`, …) |
| `crates/nessa-server/src/protocol/generated_catalog.rs` | manifest (`method::SERVER_HEALTH`, …) |
| `crates/nessa-server/src/protocol/generated_types.rs` | schemas (`ConnectParams`, `HelloOk`, …) |

Hand-written on the server: `frames.rs` / `decode.rs` / `encode.rs` (envelope helpers + routing). They **import** generated names and types.

## Wire shape

Frames are JSON text on the WebSocket:

| `type` | Direction | Correlates by |
| --- | --- | --- |
| `req` | client → server | `id` |
| `res` | server → client | same `id` |
| `event` | server → client | `seq` (monotonic per socket) |

S1 methods/events (from `manifest.json`):

| Name | Kind | Notes |
| --- | --- | --- |
| `connect` | req/res | First RPC; returns `HelloOk` |
| `server.health` | req/res | Requires a successful `connect` |
| `connect.challenge` | event | Sent immediately on socket open; nonce echoed in `connect` |

## Error codes (S1)

Returned on `res` frames with `ok: false` and `error: { code, message }`:

| Code | When |
| --- | --- |
| `already_connected` | Second `connect` on the same socket |
| `protocol_mismatch` | Client min/max does not include protocol v1 |
| `invalid_params` | Empty required metadata strings |
| `invalid_challenge` | Auth nonce ≠ this socket's challenge |
| `unauthorized` | Auth token mismatch |
| `not_connected` | `server.health` before `connect` |
| `unknown_method` | Unrecognized `method` string |
| `invalid_request` | Malformed / undecodable frame |
| `internal_error` | Encode failure or missing challenge state |

Repo context: [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md), [docs/codebase-structure.md](../docs/codebase-structure.md).
