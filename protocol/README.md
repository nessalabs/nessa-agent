# Gateway protocol

The gateway serves authenticated WebSocket sessions at `/session`. It sends
`session.challenge`, accepts `session.authenticate`, and returns verified session
metadata. Every product command checks current credentials, membership, and policy.
HTTP `/health` reports process liveness only.

| Source | Purpose |
| --- | --- |
| [product/manifest.json](product/manifest.json) | Authenticated method and event catalog |
| [product/v1.json](product/v1.json) | Session, credential, and termination payloads |
| [manifest.json](manifest.json) | Shared health and echo method schemas |
| [schemas/v1/](schemas/v1/) | Shared payloads, frames, and shortcut documents |
| [defaults/](defaults/) | Bundled shortcut defaults |
| [fixtures/v1/](fixtures/v1/) | Validated shared wire examples |

```sh
pnpm protocol:generate
pnpm protocol:check
```

Edit source schemas and manifests together with callers, fixtures, and handlers.
Generated TypeScript and Rust files name their source in their headers. No protocol
or schema version bump is needed merely to change this repository's current contract.

| Name | Purpose |
| --- | --- |
| `session.challenge` | Per-socket nonce and accepted protocol range |
| `session.authenticate` | Credential proof and nonce; returns verified session |
| `auth.session` | Current authenticated metadata |
| `server.health` | Authorized health read (`server.read`) |
| `conversation.echo` | Temporary text round trip (`conversation.write`) |
| `credential.issue`, `credential.list`, `credential.revoke` | Credential administration (`credential.manage`) |

Frames use `req`, `res`, and `event`. A transport `id` correlates a response with
its request. Mutations separately carry a stable `requestId` for explicit retries.
Credential and session `expiresAt` may be null; issuance defaults to no expiry.
Authentication challenges advertise a Unix-second deadline rounded up from
millisecond wall time. A single monotonic timeout covers challenge delivery and
authentication; expiry closes with retryable `handshake_timeout` (4006). Typed close reasons distinguish
terminal authority failures from retryable transport or dependency failures.

Credential lifecycle RPC errors distinguish `credential_conflict`,
`credential_capacity`, and `credential_not_found` from
`credential_store_unavailable`. The first three reject the command; they do not
signal a transient connection failure. Empty issuance grants are invalid.

See the [authentication decision](../docs/adr/done/0010-local-authentication.md),
[local setup guide](../docs/guides/local-auth.md), and
[SDK guide](../packages/nessa-client/README.md).
