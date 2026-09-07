# Local authentication API readiness

Measured 2026-09-07 for [ADR 0007](../adr/done/0007-authentication-delivery.md).
The existing gateway and NessaClient cover the required capabilities. No runtime
API gap was demonstrated; this change adds evidence and operating guidance.

## Coverage and caller behavior

| Capability | Evidence | Caller behavior |
| --- | --- | --- |
| Authentication | `scripts/smoke-auth.mjs`: real Rust gateway and NessaClient, invalid credentials, pre-auth requests, expiry and revocation | Supply a credential through connection setup; client metadata grants no authority |
| Identity/restrictions | Smoke test compares `auth.session()` with the initial owner snapshot and scoped credential grants; Rust `session_ready_reports_current_restrictions_and_registered_methods` | Snapshot only; registered methods are not a permission list |
| Issue and list | Smoke test checks optional expiry, metadata-only listing, exact issue retry, changed-input conflict, empty grants and rejected privilege escalation | Persist the request ID and exact input; retain the first secret securely |
| Denied administration | Smoke test checks reader list, issue and revoke against real Cedar | Reads throw `NessaRpcError` with `forbidden`; mutations wrap it in `NessaMutationError.cause` |
| Revoke | Smoke test checks shared-token invalidation, unrelated sessions, self-revoke acknowledgement, restart persistence and exact revoke retry | Reuse the original request ID on explicit retry; admitted work may finish |
| Lost issuance response | Smoke test proxy drops the successful response after durable commit; checks one issue frame, then restarts gateway and explicitly retries | `NessaMutationError.requestId` survives; retry returns the original ID with `secretUnavailable`, never another secret or credential |
| Capacity | `scripts/measure-auth.mjs` fills through real issuance to 1,000, rejects 1,001, retries a retained revoked credential's issue receipt, checks unrelated health | `credential_capacity` is a command rejection; deliberate offline capacity adjustment, no blind retry |
| Adapter substitution/isolation | Rust `administration_reports_typed_rejections_from_a_substitute_adapter`, `configured_limits_apply_to_mutations_reopen_and_independent_stores`, auth port tests; client composition tests | No new dependency seam or concrete backend choice introduced |

Initial authentication uses `NessaRpcError` with `unauthorized` for invalid, expired
or revoked credentials; it deliberately does not distinguish those causes to an
unauthenticated caller. Established sessions expose
`NessaConnectionClosedError.closeReason` and `retryable`; expired/revoked access
is terminal, requiring new authorized credentials. Permission denial does not
mean the transport is broken. `credential_conflict`, `credential_not_found`, and
`credential_capacity` are distinct command rejections; `credential_store_unavailable`
is a storage/provider failure. Existing substitute-adapter tests exercise that
failure without weakening authorization or sabotaging a user's filesystem.

Temporary transport failures and deadlines are handled by bounded connection
recovery. A failed mutation may have committed even if the connection is retryable.
Inspect `NessaMutationError.cause`, retain `requestId` plus the exact command, and
retry explicitly after restoring authorized access. Never infer non-execution from
a timeout. If the issuance secret was lost, resolve its receipt, revoke the returned
credential, then deliberately issue a replacement using a new request ID. Trusted
local recovery remains an offline OS-authorized command, never a gateway bypass.

## Measured operating range

Reproduce with `pnpm auth:measure`. The script creates an isolated temporary
`ci/instances/auth-bounds` namespace, initializes through the executable, uses the
real gateway, Cedar, NessaClient and durable storage for every issued record, then
closes clients/server and removes only that temporary namespace. No direct registry
seeding or policy bypass is used. Build cache follows the worktree's `target` link.

Reference host: Apple M5, 10 logical CPUs, Darwin 25.6.0, Node 26.8.1; debug Rust
build on local storage. Default limits: 1,000 credentials, 4 MiB registry, 2,000
receipts **per collection**. Each issued integration has its own principal and
membership, one `server.read` grant and no expiry. Final registry includes 64
revoked credentials and their receipts. Full observations and sample counts are
in [the JSON evidence](evidence/auth-api-bounds-macos.json).

| Registry entries | Listing samples | Metadata JSON bytes | Registry bytes | Listing p50 / p95 / max ms |
| --- | --- | --- | --- | --- |
| 100 | 30 | 40,202 | 86,049 | 1.56 / 1.98 / 2.39 |
| 500 | 30 | 199,002 | 429,249 | 5.73 / 6.49 / 6.79 |
| 1,000 | 30 | 397,886 | 868,297 | 10.47 / 10.97 / 11.06 |

JSON size is the SDK metadata result serialized to UTF-8, excluding the wire
envelope and WebSocket framing. Timing includes round trip and SDK parsing.

| Load at 900–964 entries | Samples | p50 / p95 / max ms |
| --- | --- | --- |
| Health, 1 connection | 20 | 0.18 / 0.27 / 0.46 |
| Health, 16 connections | 320 | 0.53 / 0.89 / 1.24 |
| Health, 64 connections | 1,280 | 1.82 / 2.97 / 4.72 |
| Connect additional 48 sessions concurrently | 48 | 5.59 / 5.91 / 5.94 |
| Issue, 8 concurrent mutation callers | 64 | 231.59 / 251.46 / 253.90 |
| Revoke, same callers | 64 | 231.88 / 247.60 / 255.55 |
| Health, 56 readers throughout mutations | 18,555 | 0.94 / 1.95 / 3.85 |

Each writer performs eight issue/revoke pairs sequentially. Readers wait 10 ms
between requests and remain active until all writers finish. The mixed workload
completed in 3,766 ms (128 mutations, about 34/s); writes retain real serialization,
private-file replacement and synchronization. Sampled server RSS grew from about
15 MiB at 100 entries to 37 MiB maximum. RSS is sampled with `ps` every 250 ms and
at phase boundaries; it is not an exact allocator peak or CPU measurement.

Support the default registry size with these ordinary metadata shapes, up to 64
simultaneous authenticated local connections and eight outstanding credential
mutations across connections. This is a measured operating range, not an enforced
connection cap or universal latency SLA. Keep the existing 4 MiB byte guard and
receipt limits; more/larger grants can reach the byte guard before 1,000 records.
Higher configured sizes, connection counts, long-duration churn, remote network
latency and hostile connection floods are unmeasured. Measure on the deployment
hardware before expanding this range. Windows/Linux functional support does not
imply these Mac latency or memory figures apply there.

No pagination or new quota is warranted within this range: the largest measured
listing is under 0.4 MB and 12 ms, and normal reads continue during durable writes.
Existing per-connection write timeouts, handshake deadlines and admission-time
authorization remain unchanged. This probe is a reproducible measurement command,
not a timing threshold in CI; noisy shared runners must not turn observations into
flaky correctness tests.

## Retention decision

Retain revoked and expired credential metadata and all issue/revoke receipts for
the lifetime of the registry. Do not compact automatically or delete records to
free capacity. Credentials and receipts are bounded independently, and revoked
records still count. Exact old requests resolve existing receipts even at capacity;
changed inputs conflict. Removing receipts would let an old issue request create
another credential; deleting revocation history or restoring stale snapshots can
undermine access invalidation. No safe retention horizon is implied by this ADR.

At capacity, new mutations fail closed without publication. Read/list and existing
valid credentials continue to work. Revoke receipts can also fill (including new
request IDs for repeated revocations); reuse the original ID for a retry. Monitor
metadata count and the private registry file size operationally, allow headroom for
rotation/recovery, and deliberately raise the configured limit offline if needed.
Owner recovery creates a retained record too and may need more headroom. Never
reset the registry or prune receipts to unblock it. A future cleanup design must
preserve permanent non-replay and invalidation before any retention change.

## Validation and limitations

On this Mac: 80 Rust tests across `nessa-auth`, `nessa-local-storage`, and
`nessa-server`; 89 NessaClient tests; real gateway/SDK
smoke including the dropped response; bounds probe; client typecheck; architecture
and protocol checks; strict Clippy for all three Rust crates. No runtime or platform
storage code changed. The existing Windows/Linux/macOS workflow continues to run
the expanded smoke test; this local validation does not claim a new Windows/Linux
run. Historical platform evidence remains linked in [local auth](../guides/local-auth.md).
