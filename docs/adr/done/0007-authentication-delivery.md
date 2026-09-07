# 0007. Local authentication API readiness and operating bounds

## Purpose

Make local authentication capabilities available through authorized APIs and
establish their supported operating limits. Reuse the APIs already implemented;
credential settings and recovery UI can be planned separately later.

- **Date:** 2026-09-06
- **Status:** accepted and completed — existing APIs verified; operating bounds measured
- **Builds on:** [0010 — local authentication](../done/0010-local-authentication.md)

## Current API coverage

Local authentication and credential administration already work. The following
operations are present in the [product manifest](../../../protocol/product/manifest.json)
and exposed by the existing NessaClient. They are not new endpoints to implement.

| Capability | Existing API | Contract |
| --- | --- | --- |
| Authenticate a connection | `session.authenticate` through client connection setup | Verify credentials before allowing product operations |
| Read current identity and restrictions | `auth.session` / `client.auth.session()` | Current snapshot; subsequent operations still require authorization |
| Issue a scoped credential | `credential.issue` / `client.credentials.issue()` | Authorized creation, optional expiry, one-time secret delivery, stable mutation identity |
| List credentials | `credential.list` / `client.credentials.list()` | Authorized metadata only; secrets are never listed |
| Revoke a credential | `credential.revoke` / `client.credentials.revoke()` | Authorized, idempotent mutation with an explicit retry identity |

The client exposes typed RPC/connection errors and retains mutation request IDs
through `NessaMutationError`. Connection recovery is bounded; uncertain mutations
are not automatically replayed. Trusted local setup/recovery remains available
through the existing native/command-line path. An unauthenticated gateway caller
cannot restore its own access.

Registry capacity and connection deadlines are already configurable. Local
credential persistence, platform access controls, revocation, and slow-socket
isolation have existing checks under ADR 0010. This ADR does not repeat that work
or introduce a second authentication path.

## Decision

Limit this ADR to backend/API readiness and measured operating bounds. Future
surfaces consume the existing APIs through NessaClient. UI rendering, settings
screens, recovery navigation, and user-facing copy are outside this decision and
do not block its completion or the Rust SDK work that follows.

### 1. Verify coverage and implement only demonstrated API gaps

Check the real gateway and NessaClient contracts against credential metadata,
issuance, revocation, identity/restriction inspection, and typed failure handling.
Use existing tests and targeted additions where a required behavior lacks evidence.
Document how callers distinguish permission denial, invalid/expired/revoked
access, temporary transport failure, and uncertain mutation outcomes.

If a required capability is missing, add it through the existing application,
authorized gateway, shared protocol, and NessaClient boundaries. If it already
exists, mark it covered. No new endpoint is required merely because the future UI
has not been built. Do not add speculative recovery APIs or weaken the trusted
local recovery boundary.

Preserve one-time secret delivery, metadata-only listings, scoped authorization,
and explicit retries using the original mutation identity. A lost issuance
response must not reveal the secret on retry or silently issue another credential.
Update the current contract, callers, tests, and local development data directly;
no compatibility aliases, fallbacks, or unnecessary version bumps.

### 2. Establish supported registry sizes

Measure credential listing size and response time against realistic configured
registry sizes. If pagination is needed for the supported bounds, implement it
across storage, protocol, and NessaClient with authorization and consistent page
semantics. No settings screen is required to validate that API.

Record a retention decision for revocations and mutation receipts before adding
cleanup. Removing records must not revive access or make an old request create a
new credential. Configurable bounded storage is a valid outcome when measurements
support it; automatic compaction is not required without a demonstrated need.

### 3. Establish gateway load bounds

Measure latency and resource use with concurrent connections and credential
mutations through the real authorization and persistence paths. Document supported
bounds and add quotas only where evidence requires them. Quota rejection must not
bypass policy or disrupt unrelated sessions. Preserve per-connection write
timeouts and admission-time authorization.

## Completion criteria

- Required auth capabilities have a documented API coverage result; demonstrated
  gaps are implemented and verified through the real gateway and NessaClient.
- Authorized/denied operations, typed failures, one-time secret handling, and
  uncertain mutation retries have appropriate existing or added test evidence.
- Supported registry and connection/mutation load bounds are measured and
  documented, with any necessary pagination/limits verified at the API boundary.
- Retention behavior is explicit; required adapter changes retain substitution
  and independent application isolation tests. Platform checks remain passing.

If API coverage is already complete, that part requires no new runtime code.
Only the remaining measurement or demonstrated backend gaps keep this ADR open.
Move it to `done/` when those criteria are met; do not wait for UI implementation.

## Deferred work

Credential settings, recovery screens, messages, navigation, and secret-storage
presentation flows belong to a later UI proposal if requested. This update does
not create or schedule another ADR automatically.

Hosted login, broader tenancy, optional live policy reload, and push-based idle
invalidation remain separate decisions. Conversation/stream permissions are added
with their operations under [ADR 0008](../todo/0008-agent-client-api.md) and
[ADR 0011](../todo/0011-nessa-session-protocol-and-authorities.md). Optional MCP/CLI agent
tools belong to [ADR 0012](../todo/0012-agent-harnesses-and-optional-tools.md).

See [ADR 0010](../done/0010-local-authentication.md), the
[local auth guide](../../guides/local-auth.md), and the
[gateway review](../../reviews/local-auth-gateway.md) for the implemented system
and its existing evidence.

## Completion evidence — 2026-09-07

The [readiness review](../../reviews/auth-api-readiness.md) records API coverage,
caller error/retry behavior, the retention decision, measured results and platform
limitations. Existing APIs fulfill the required scope; no runtime changes, new
endpoint, schema change, pagination or quota were needed. Added real gateway/SDK
evidence covers identity/restriction inspection, typed denied mutations and an
issuance response deliberately lost after commit, followed by restart and explicit
retry without a second secret or credential.

Supported measured range: default 1,000-entry registry with ordinary scoped
metadata, 64 local authenticated connections and eight concurrent mutation callers.
The 1,000-entry list was 397,886 bytes with 10.97 ms p95; mixed-load issue/revoke
p95 was about 250 ms while health p95 remained 1.95 ms. See the review for exact
workload, sample counts, resource measurements and limits on extrapolation.
Retain revocations and receipts for the registry lifetime under existing capacity
bounds; no automatic cleanup. Platform storage and dependency seams are unchanged.
All completion criteria have evidence in the review. UI remains deferred and
other ADRs remain proposed.
