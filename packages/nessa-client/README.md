# Nessa TypeScript SDK

`@nessa/client` connects to the Nessa gateway from a browser or a JavaScript
runtime with a WebSocket implementation. Within this repository the package is
linked directly. The package is currently private and is not published to npm.

## How the pieces work together

`NessaClient` owns one managed connection. `NessaClient.connect(options)` validates
configuration, opens a WebSocket, checks the challenge's protocol range, and
sends credential evidence for the product profile. It returns when authentication
has succeeded. Its `server`, `auth`, and `credentials` API objects share that
connection; each method sends a typed request and validates its response.

`NessaClientConfig` holds immutable timeout and retry settings. The managed session
handles transport replacement after eligible failures. API objects and subscriptions
survive recovery, but in-flight requests fail and are never automatically replayed.
Read `connectionState` and subscribe to state changes to reflect recovery in a UI.

The authentication types describe relationships: a `ProductPrincipal` acts through
a `ProductMembership` in an organization. A credential authenticates that principal
for a gateway audience. Each `ProductGrant` limits an action to a `ProductResource`.
For example, `server.read` is the policy action checked for `server.health`; it is
not itself an RPC method. Grants and session snapshots do not replace the server's
per-command check of current membership, credential validity, and policy.

`ProductCredentialMetadata` contains no secret. The first issuance returns an
`IssuedCredentialResult` with secret evidence; a duplicate request returns an
`ExistingCredentialResult` without that secret. Preserve the operation's request ID
when retrying an uncertain mutation. `NessaMutationError` retains it on failure.

The generated symbol pages below explain the current methods and individual fields.

## Connect

Initialize local access and provision the surface using the repository's
[local-auth guide](../../docs/guides/local-auth.md). Node automatically loads its
assigned private `auth/surfaces/<client.id>.token` when `auth` is omitted. The desktop
panel uses an injected native `CredentialSource`. Other hosts can inject their own
source or pass `auth: { credential }`. Missing storage fails before opening a socket.

```ts
import {
  NessaClient, NessaClientConfig, ConnectionProfile,
  ClientRole, SurfaceKind, ClientPlatform, Stage,
} from "@nessa/client"

async function run() {
  const client = await NessaClient.connect({
    profile: ConnectionProfile.Product,
    stage: Stage.Dev,
    url: "ws://127.0.0.1:7420/session",
    role: ClientRole.Surface,
    surface: { kind: SurfaceKind.Cli, instance: "terminal" },
    client: { id: "terminal", version: "1.0.0", platform: ClientPlatform.Node },
    config: new NessaClientConfig(),
  })
  try {
    console.log(await client.server.health())
  } finally {
    client.close()
  }
}
```

Product authentication is mandatory, including local development. `role`,
`surface`, and `client` describe the client; they do not grant permissions.
Local credentials have no expiry by default. Issuance accepts optional future
`expiresAt` seconds; session and credential metadata use `null` for no expiry.

Use `ClientPlatform.Node` for Node.js, `Browser` for browsers, or `Macos`, `Linux`,
`Windows`, and `Other` for host-specific clients. IDs, instance names, versions,
URLs, credentials, resource names, and extensible policy action names remain
strings. Closed vocabularies have named constants: `ClientRole`, `SurfaceKind`,
`ClientPlatform`, `Stage`, `PrincipalKind`, `MembershipRole`, `MembershipState`,
`Scope`, and the shortcut enums. Their values are also accepted as literal strings.

## Configure retries and lifecycle

```ts
const config = new NessaClientConfig({
  requestTimeoutMs: 30_000,
  retry: { maxAttempts: 3, initialDelayMs: 250, maxDelayMs: 2_000, jitter: true },
  reconnect: {
    enabled: true,
    maxAttempts: 3,
    initialDelayMs: 250,
    maxDelayMs: 2_000,
    jitter: true,
  },
})
```

These are the defaults. Initial `retry.maxAttempts` includes the first connection.
`reconnect.maxAttempts` counts new connection attempts after an established
transport closes. The reconnect budget resets after successful authentication.
Set `retry.maxAttempts: 1` to disable initial retries and
`reconnect.enabled: false` to disable established-session recovery. The config is
validated and frozen; create a new config to change settings for a new client.

Both policies use capped exponential backoff, with jitter between half and all of
each delay ceiling. Valid server `retryAfterMs` hints can extend that delay, up to
60 seconds. Each retry opens a fresh socket, obtains a fresh challenge, and
authenticates again. Reconnection uses the credential loaded or supplied for this `connect` call.
Call `connect` again to load a newly provisioned credential.

`connectionState` is `connected`, `reconnecting`, or `closed`.
`onConnectionStateChange(handler)` reports subsequent transitions and returns an
unsubscribe function. Read `connectionState` for the current snapshot.
`onClose(handler)` runs once on permanent closure, including explicit close,
terminal failure, or retry exhaustion. Late close subscribers receive the final
error immediately. Observer exceptions cannot prevent cleanup.

Network interruption, restart, overload, handshake timeout, and temporary gateway
dependency failures are retryable. Revoked or expired credentials, lost
authorization, authentication rejection, incompatible protocols, normal shutdown,
and unknown close codes stop recovery. `NessaConnectionClosedError` exposes the
numeric `code`, typed `closeReason`, `retryable`, and optional `retryAfterMs`.
`SessionCloseReason` provides discoverable reason constants. A network failure may
prevent the server's reason from arriving; the next authentication then checks
current authorization.

API objects and event subscriptions survive recovery. Session getters are only
available while connected. Pending RPCs reject on closure; calls during recovery
reject with `NessaSessionUnavailableError`. **No RPC is automatically replayed.**
A lost response does not prove the operation failed. For credential mutations,
reuse the same `requestId` when explicitly retrying an uncertain operation.
The SDK generates it when omitted, includes it in successful mutation results,
and retains it on `NessaMutationError` along with the original error as `cause`.
Provide an ID saved with the inputs if retries must survive a caller-process crash. An
issued secret is returned once and cannot be recovered by replaying the command.

`close()` cancels recovery, closes an in-progress reconnect socket, rejects pending
requests, and releases subscriptions. It is safe to call repeatedly.

## Available product APIs

- `client.server.health()` reads authorized gateway health.
- `client.conversation.create({ conversationId? })` creates or reopens an agent conversation.
- `client.conversation.send(id, text)` queues input; `steer(id, text)` uses supported steering.
- `client.conversation.read(id)` returns a bounded replacement view of live output, waiting input, tools, and permissions.
- `remove`, `answer`, `cancel`, and `close` control waiting input, permission reviews, and the live context.
- `client.auth.session()` reads the current authenticated session.
- `client.credentials.issue(params)` issues a scoped credential.
- `client.credentials.list()` lists credential metadata without secrets.
- `client.credentials.revoke(credentialId, requestId?)` revokes a credential.

The server authorizes every operation using current state.
`NessaRpcError` carries server RPC rejection details;
`NessaProtocolCompatibilityError` reports incompatible version ranges before any
credential is sent. Transport errors do not imply a mutation was rolled back.

## Agent conversations

```ts
const { conversationId } = await client.conversation.create()
const receipt = await client.conversation.send(conversationId, "Hello")
const view = await client.conversation.read(conversationId)
```

Read serially while the surface is visible to display current streamed output. Each
view replaces the previous one; `revision` is opaque, and `truncated` indicates
omitted history or text. `permissionViewError` means a complete review cannot be
shown safely. Permission controls must use its exact offered IDs and original input.

The client generates separate execution and action IDs once per message. A
conversation ID is a canonical lowercase hyphenated UUID. Message text is limited
to 8 KiB of UTF-8, and execution and action IDs are limited to 256 UTF-8 bytes.
The client enforces these limits before sending a request. A
`NessaConversationMutationError` retains them and exposes `retry()` for the same
immutable creation or message-admission command after recovery. No mutation automatically replays. If retry state
must survive a process restart, save the input and the error's `conversationId`,
`executionId`, and `requestId`, then pass the IDs in the command options. An
intentional new send generates a new execution ID even when its text is identical.

Controls (reorder, remove, answer, cancel, and close) instead expose `NessaConversationControlError` with `uncertain: true` when no trustworthy acknowledgement arrives. They do not offer `retry()`: read the current view, then deliberately choose a new action if needed. Replaying an earlier close could stop newer work from another surface.

Failed permission answers also expose `permissionSelection` as `"pending"`,
`"consumed"`, or `"unknown"`. This typed fact says whether the domain review is
still actionable, was selected before a later audit or delivery failure, or must
be resolved by reading authoritative state. It is independent of the diagnostic
error code and message; `pending` also sets `uncertain` to `false` because the
gateway proved that no selection occurred.

`client.conversation.close(id)` closes the live provider context and retains saved
conversation history. `client.close()` only disconnects this surface; the gateway
continues owning accepted work.

## Export API documentation

From the repository root:

```sh
pnpm client:docs
pnpm client:docs:json
pnpm client:docs:check
```

The first command exports a standalone, searchable HTML reference to
`docs/generated/client-api/index.html`. Copy or zip that entire directory to share
it or host it as static files. The second exports TypeDoc's machine-readable API
model to `docs/generated/client-api.json`. Both commands regenerate protocol declarations first, then generate documentation from the public
entry point and JSDoc, using `typedoc.client.json`; generated output is ignored by
git. No hosted service is required. Regenerate after SDK changes.

Maintain explanations alongside the implementation: JSDoc on SDK classes, methods,
and options; JSON Schema `description` fields in `protocol/product/v1.json` and
`protocol/schemas/v1/*.json` for generated wire types. The product generator carries
those descriptions into TypeScript JSDoc, which TypeDoc renders on symbol and
property pages. Do not edit generated TypeScript or HTML to change documentation.
`pnpm protocol:check` detects drift between schemas and generated declarations.
`pnpm client:docs:check` also verifies that public interface and field descriptions
survive the complete generation pipeline and that the main SDK classes have member
documentation. This check is included in `pnpm check`.
The prose is maintained in source; regeneration extracts it rather than inventing
explanations from signatures.

`pnpm client:typecheck` checks the SDK and its executable example in
`examples/product-client.ts`. `pnpm client:test` runs handshake, lifecycle, cleanup,
and real-socket stress tests. These bounded tests do not replace long-running
network or memory profiling.

On Windows, automatic local loading uses `%USERPROFILE%\.nessa` and built-in Windows
PowerShell for handle-based ACL checks. Files must have a protected DACL limited to
the current user SID and LocalSystem; inherited/shared ACLs, hard links and reparse
points are rejected. PowerShell environments that disable `Add-Type` fail closed.
The native desktop uses the shared Rust storage backend directly. See
[local authentication](../../docs/guides/local-auth.md#windows-storage) for platform
requirements and validation status.

### Reorder waiting messages

`client.conversation.reorder(id, executionIds)` atomically replaces the full waiting order (at most 64 unique IDs). Include every current waiting input, including steering inputs; running work is excluded, and ordinary messages cannot move ahead of steering. The result contains `requestId` and `outcome`: `applied`, `unchanged`, `queue_changed`, or `priority_conflict`. The latter two leave the queue unchanged; refresh the view before choosing another order. An uncertain acknowledgement exposes `NessaConversationControlError` without replay.
