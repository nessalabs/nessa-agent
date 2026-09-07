# Local auth and gateway adversarial review

Reviewed the current working tree: credential storage and minting, session
assembly, Cedar evaluation, server composition, WebSocket admission/dispatch,
SDK/CLI delivery, expiry, and revocation. This review includes fixes and regression
tests; it is not a claim that the system has no vulnerabilities.

## Findings and changes

| Finding | Impact | Resolution |
| --- | --- | --- |
| P1: the serving router also mounted the legacy shared-token route | A shared token (including the development default) reached health/echo handlers without local identity or action authorization | Only `/session` serves WebSockets with the mandatory product handshake and request authorization. `/` returns 404; the legacy handler was removed. |
| P2: registry loading used `Path::exists()` | A dangling registry symlink looked like an uninitialized store, allowing initialization to replace it instead of failing | Inspect symlink metadata and distinguish missing paths from invalid files. Bound reads on the opened file as well as checking metadata. |
| P2: private-file setup changed permissions before validating the existing object | Directory symlinks could change target permissions before rejection; hard-linked/shared files could be adopted | Open the directory without following its final symlink before chmod; reject writable-by-others directories, shared-permission files, and non-regular or multiply-linked files. |
| P2: session validity was implemented separately in auth and gateway code | Enforcement could drift as identity or revocation rules evolved | Added application-owned `ReadCurrentSession`, used by both `AuthorizeAction` and gateway state checks. Dispatch checks current identity before routing. |
| P3: origin filtering accepted string prefixes instead of complete origins | Malformed origin strings matched the local-origin allowlist | Accept only exact local scheme/host/optional numeric-port forms, including IPv6 loopback. This is defense in depth; it does not replace token verification. |
| P3: product request validation was inconsistent | Empty/oversized IDs and unexpected health parameters reached dispatch; revoke command IDs lacked the issue-command bound | Bound envelope IDs and revoke command IDs, and require empty health parameters. |

## Gateway contract

1. The gateway sends a versioned challenge and accepts only a matching,
   unexpired authentication exchange. Raw bearer evidence goes to the injected
   verifier; request-provided principal labels do not establish identity.
2. Authentication binds credential, principal, organization, membership, and
   audience. Proof/credential expiry bounds the session lifetime.
3. Every request checks current state. `auth.session` returns only the caller's
   current metadata. Product handlers additionally require authorization:

   | RPC | Required action on the server-resolved gateway |
   | --- | --- |
   | `server.health` | `server.read` |
   | `credential.issue`, `credential.list`, `credential.revoke` | `credential.manage`, with active admin membership |

4. Unknown methods are rejected. New handlers must have an explicit action and
   trusted resource resolution; membership or authentication alone is insufficient.
5. Each protected operation is authorized against one committed snapshot at
   admission. Admitted handlers/responses may finish after revocation; later
   operations read again. Only auth mutations serialize; socket writes have no
   global lock. Sessions expire and idle sessions poll current state.
6. HTTP `/health` intentionally reports only liveness via a status code. It exposes
   neither identity metadata nor product operations.

The shared token environment setting and legacy SDK protocol were removed. The
local registry must be initialized in every stage. The panel uses `/session` and
loads its distinct surface credential through the native host. Owner and ordinary
credentials have no expiry by default, with optional future expiry; grants remain
specific to each credential.

## Verification

Passed on macOS:

- `cargo test -p nessa-local-storage -p nessa-auth -p nessa-server`: 72 tests, including real Cedar,
  injected adapter isolation, secret hashing/redaction, retries, persistence,
  restart, uncertain-write failure, symlinks, and shared-file rejection.
- Gateway tests use a health dependency that panics if invoked to prove that
  revoked, expired, disabled, stale, mismatched, cross-organization, unavailable
  store, and unavailable policy cases never reach the handler.
- `pnpm auth:e2e`: real binary and SDK/CLI bootstrap, issue/retry, changed-request
  rejection, admin escalation rejection, optional expiry and no-expiry access, denial,
  idle revocation/expiry, restart, recovery, and rejection of legacy/pre-auth RPCs.
- `pnpm server:smoke`: authenticated product connection and health request.
- 137 TypeScript tests, typechecks, Rust Clippy, architecture and protocol checks.
- Native surface credential loading test and frontend production build.
- Deterministic socket backpressure and revocation-at-admission regressions prove
  unrelated sessions continue and admitted responses are not discarded.

## Remaining limits

- Local process/OS ownership is the trust boundary. A process running as the same
  OS user can read owner secrets or alter files. Data-root ancestors must be
  trusted; final-component symlink checks are not a complete directory-descriptor
  traversal defense against hostile ancestor replacement.
- Socket writes have a per-connection configurable deadline (five seconds by
  default); a slow reader does not hold a shared authorization mutex. Connection
  quotas and load/latency guarantees are still unimplemented.
- Revocation does not cancel admitted operations or their output. Reads after
  publication observe the new state. Idle invalidation polls every second by
  default (configurable); the revision notification port is not yet consumed.
  Streams will need action/batch checkpoints when introduced.
- Storage defaults to 1,000 credentials and 4 MiB. These limits and receipt capacity
  can be raised in the namespace config file without rebuilding. There is no
  compaction or pagination; configured capacity exhaustion can still block minting
  and owner recovery.
- Windows private ACL support is implemented for the gateway, token commands,
  desktop, and Node. Its native crate/tests cross-compile; Windows and Linux
  runtime results remain pending in the platform CI workflow. See the guide for
  supported storage, Node PowerShell requirements, and durability scope.

See [local auth usage](../guides/local-auth.md) and the
[local decision](../adr/done/0010-local-authentication.md).
