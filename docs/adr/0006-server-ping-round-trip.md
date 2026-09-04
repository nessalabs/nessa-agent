# 0006. Prove the session link with `server.ping` before chat RPCs

- **Date:** 2026-09-04
- **Status:** accepted

## Context

The panel already completes S1 connect (`HelloOk`) and `server.health`, then
marks the session ready. That proves auth and liveness, but it does not prove
that a **client-initiated, correlation-id’d RPC** round-trips a payload the
client chose — the shape every later product call (including chat) will use.

We want that proof **now**, without inventing conversation turns. ADR 0002
keeps `sendDraft` / composer a no-op until real chat RPCs exist; bolting an
echo onto the composer would teach the wrong boundary.

WebSocket Ping/Pong frames already exist at the transport layer and are not
how product RPCs work in this stack (the server ignores them for app logic).
The link we care about is JSON `req` / `res` on the control plane — and on the
client that path is **`NessaClient`**, the same facade that already owns
`connect` and `server.health`.

A link probe is a **development** tool. It must not exist as a live method on
`prod` / `alpha` / `ci`: not “started then refused,” but **never wired**
outside `dev`.

## Decision

Add a control-plane method **`server.ping`**, **dev-only on the server**,
**always invoked through `NessaClient` on the client**:

| | |
| --- | --- |
| Params | `{ "nonce": string }` (non-empty) |
| Result | `{ "ok": true, "nonce": string }` — **must echo** the request nonce |
| When offered (server) | Only when the server process stage is `dev` |
| Client API | `NessaClient.server.ping(nonce)` — same `ServerApi` namespace as `health` |
| Role | `surface` (when offered) |

**Server:** register / dispatch `server.ping` **only if** stage is `dev` at
startup. On every other stage the method is **not installed** — unknown-method
like any method that was never composed. Do not install a handler that listens
and then returns “wrong stage.”

**Client (`@nessa/client`):** wire `ping` on `ServerApi` / `NessaClient.server`
exactly like `health` (`WireSession.request` + generated method name +
`assertPingResult`). Surfaces must not open a second socket or hand-roll frames
for this probe.

**Session (panel):** only `connectDevSession` (already `stage: "dev"`) calls
`client.server.ping(...)` after health on the `NessaClient` it just connected.
Non-dev session openers must not call it. If ping fails or the echoed nonce
mismatches, close the socket and fail the session the same way a health
failure does.

Schemas live under `protocol/` so the SDK and the `dev` server share one typed
contract; **runtime composition** gates whether the server listens.

**Not** a keepalive loop in v1 — one probe at session establishment is enough.

## Alternatives considered

- **Treat connect + health as sufficient.** Health does not exercise
  client-chosen payload echo or the same mental model as later RPCs. Rejected
  for the “are we actually linked?” proof in dev.
- **Install the handler on every stage and reject non-dev at call time.** Still
  listens; still a live code path on prod. Rejected — **compose only on `dev`**.
- **Bypass `NessaClient` (raw WebSocket / ad-hoc JSON from the panel).** Splits
  the client surface; fights Architecture (“chat adapters use
  `getSessionClient()`”). Rejected — **all sends go through `NessaClient`**.
- **Wire the composer as a temporary echo.** Fights ADR 0002. Rejected.
- **Rely on WebSocket Ping frames.** Not the typed protocol catalog. Rejected.
- **Continuous ping interval from day one.** Out of scope until needed.
  Rejected for this record.

## Consequences

Easier: local `just start` / `pnpm app` proves the full pipe through the real
SDK; later RPCs copy `NessaClient.server.*`; non-dev servers never register
ping.

Harder: server dispatch is stage-aware at composition; `ServerApi` grows a
method that only succeeds against a `dev` server; tests cover SDK +
dev-session probe + “absent on other stages.”

Watch for: panel code calling the wire without `NessaClient`; a catch-all that
still matches `server.ping` on prod; requiring ping outside `connectDevSession`;
using ping as a stand-in for turns; inventing method string literals outside
the generated catalog.
