---
id: gateway-surface-call
title: Call a gateway method from the CLI or MCP
kind: operation
status: implemented
sourceRevision: "9a3253951826e2ae0e5666f34298a8c9093ca9d7"
summary: The CLI and MCP send one catalog method through the signed-in gateway
  session, and Cedar decides.
parent: gateway
sources:
  - protocol/product/manifest.json
  - protocol/product/surface-exclusions.json
  - protocol/product/surface-hints.json
  - protocol/product/surface-catalog.generated.json
  - crates/nessa-gateway/src/cli/application/call.rs
  - crates/nessa-gateway/src/cli/application/surface.rs
  - crates/nessa-gateway/src/cli/infrastructure/mcp.rs
  - crates/nessa-gateway/src/cli/infrastructure/gateway.rs
  - crates/nessa-client-core/src/local_product.rs
  - crates/nessa-gateway/src/product/read_access.rs
  - crates/nessa-gateway/src/product/socket.rs
  - crates/nessa-gateway/tests/cli/surface.rs
  - crates/nessa-gateway/tests/cli/gateway.rs
model:
  spec: tla/services/gateway/SurfaceCall.tla
  config: tla/services/gateway/SurfaceCall.cfg
  steps:
    Init: Begin with a signed-in command socket.
    RejectLocally: Refuse a method or parameters the catalog rejects.
    Reauthenticate: Open a fresh signed-in socket after retirement.
    Dispatch: Send one call on the current socket.
    Decide: Evaluate the gateway admission decision.
    Reply: Settle the call with a gateway reply.
    LoseExchange: Retire the socket and report the uncertain exchange.
    NextCall: Begin a distinct subsequent call.
diagramLinks: {}
diagramDetails:
  - diagram: 0
    node: Checking
    section: The catalog accepts the call
  - diagram: 0
    node: Sent
    section: Cedar decides on the gateway
  - diagram: 0
    edge:
      from: Sent
      to: Refused
      label: Cedar denies
    section: A missing grant is the gateway's code
modelCheck:
  sourceRevision: "9a3253951826e2ae0e5666f34298a8c9093ca9d7"
  checker: tlc-1.7.4
  date: 2026-11-25T06:39:50.500Z
  config: tla/services/gateway/SurfaceCall.cfg
  modelDigest: 1d4754b8b7ec5e0db14cd21802931e281f95d6d484ef9bf3dbcf5c66b9dee080
  states: 100
  held:
    - CatalogBeforeSend
    - CedarBeforePayload
    - ReplyMustMatch
    - LossRetiresSocket
    - LostVaultIsUnconfirmed
  broke: []
  unchecked: []
---
# Call a gateway method from the CLI or MCP

`nessa call` and `nessa mcp` send a method the app can already send through the
same gateway handler. The gateway checks that the session is current and asks
Cedar for methods that require a grant.

The method list is generated from `protocol/product/manifest.json`. A method in
`protocol/product/surface-exclusions.json` is left out, and each exclusion has a
written reason: the handshake, upload byte pieces, and subscriptions that stream
events after their acknowledgement. `pnpm protocol:check` fails if a new manifest
method is in neither place.

## One call

`nessa call conversation.list --params '{}'` reads `auth/surfaces/nessa-cli.token`
and opens `/session`. `nessa mcp` reads `auth/surfaces/nessa-mcp.token` and serves
one MCP tool per exposed method. Both go through `invoke`, which checks the
params against the method's schema and then sends the method on that socket.

`server --provision-local` creates the MCP credential when the file is absent.
Without an explicit `--grants`, that credential holds `server.read`,
`conversation.read`, and `conversation.write`. It does not hold
`credential.manage`. The credential's principal is who Cedar sees. The client
name on the handshake does not grant anything.

```mermaid
stateDiagram-v2
    [*] --> Checking
    Checking --> Refused: method excluded or params invalid
    Checking --> Sent: catalog accepts the method
    Sent --> Answered: Cedar allows
    Sent --> Refused: Cedar denies
    Sent --> Lost: the exchange cannot be confirmed
    Answered --> [*]
    Refused --> [*]
    Lost --> [*]
```

## The catalog accepts the call

Checking happens in this process, before a socket write, when the method is
unknown, excluded, or the params do not match the schema in
`protocol/product/v1.json`. Those stops use `unknown_method` or
`invalid_request`. Nothing is sent, so the handler does not run.

A method the catalog accepts is sent as it was given. This step does not look
at the caller's grants.

## Cedar decides on the gateway

The signed-in session hits the same admission as the app. For methods with a
grant, the manifest names it and Cedar permits it only when the credential
holds that grant on this gateway and the membership is active. The gateway
answers `auth.session` based on the current session. An admitted call continues
into the method's existing handler. The handler's JSON is the successful result.

## A missing grant is the gateway's code

A deny comes back as the gateway's `forbidden` (or another code the handler
itself returns). `nessa call` prints `{"ok":false,"code":"..."}` and exits
nonzero. An MCP tool call returns that same JSON as its text and marks the tool
result an error. The process keeps serving. The refusal text from the gateway
is not copied out: the code is the contract.

Each tool's input schema is the method's parameter object, with `type` set to
`object` on that schema. A call waits as long as the longest handler can still
be working, including an agent install's archive transfer. If the answer does
not arrive in that time, or the reply cannot be tied to the request, the
connection is closed. The next call signs in again, so it cannot read the late
reply. Both surfaces report `vault_change_not_confirmed` if a
`vault.changePassphrase` exchange ends without a usable status or a recognized
refusal, because the change may already have taken effect.

What this page does not cover: subscriptions stay excluded because one reply
would drop their events, and upload or resource bytes still travel on their HTTP
routes. In-app Claude and Codex harnesses are not yet launched with `nessa mcp`.

## Model coverage

The model follows up to two grant-checked calls made in order on at most two
sockets. It starts after sign-in. Catalog validation controls dispatch, the
gateway's admission decision controls payload delivery, and an independent
reply ID check controls which response can settle the call. The model groups a
mismatched or unusable response and a timeout into the same loss action. That
action retires the socket. A subsequent call can open a fresh one. For a
passphrase change, loss reports the uncertain outcome.

Catalog and Cedar decisions are modeled as inputs. The model omits actual
schemas, session-only admission, credential establishment and failed
reauthentication, handler-specific results, frame and deadline bounds, and server
work after a transport loss.
It checks safety within those bounds. Source review and counterexamples are
recorded in the [gateway model evidence](../../../reviews/gateway-coverage-models.md).
