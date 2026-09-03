# Protocol — S1 (connect + health)

Wire contracts for the **current** spike only: WebSocket handshake and `server.health`.

| Path | Purpose |
| --- | --- |
| [manifest.json](manifest.json) | **SSOT for method/event names**, roles, params/result refs |
| [schemas/v1/](schemas/v1/) | **SSOT for payload shapes** (JSON Schema) |
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

**Full v1** (conversation, channel state, agent, capability, plugin manifests) remains on branch `spike/server-ws-acp-panel`. Add schemas here when each spike lands — see [docs/plan/vision.md](../docs/plan/vision.md).

Docs: [docs/server/protocol.md](../docs/server/protocol.md), [docs/server/client.md](../docs/server/client.md), [docs/server/websocket-primer.md](../docs/server/websocket-primer.md), [docs/coding_standards.md](../docs/coding_standards.md).
