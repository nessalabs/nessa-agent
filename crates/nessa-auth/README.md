# nessa-auth

A Rust **library** for Nessa-owned identity/access contracts. This first slice
contains domain models, application DTOs, verification/state/policy ports,
session identity assembly, current-state authorization, and an embedded Cedar
adapter and a durable local credential backend. It has no executable or hosted
identity-provider SDK. The server now integrates these contracts on `/session`;
see [local setup and guarantees](../../docs/adr/done/0010-local-authentication.md).
Publishing remains disabled while the initial API is reviewed.

## Dependencies point inward

```text
External input → adapter verification / translation → application DTO
                                                       │
                                                validated mapping
                                                       │
                                                   domain values

Composition → verifier + access reader + clock → AuthenticateSession
                                                    │
                                          AuthenticatedSession
```

- `domain/`: private-field IDs and models, memberships, exact grants, credential
  lifetime/revocation invariants, and AuthContext. No serde or runtime imports.
- `application/dto.rs`: serializable boundary data. Parsing is not authentication.
- `application/mapping.rs`: `TryFrom` validation into domain objects. Mapping an
  admin membership does not authorize storing it; that belongs in a trusted use case.
- `application/ports.rs`: object-safe asynchronous verification/state ports,
  an absolute-time clock, and a policy evaluator contract. No runtime is required.
- `application/session.rs`: verify evidence, load one committed access snapshot,
  check IDs/audience/membership/time, then construct a context with bounded expiry.
- `application/authorization.rs`: re-read current state and check lifetime/linkage
  before evaluating an action. Older-than-login snapshots fail unavailable.
- `adapters/cedar/`: parse and validate the policy bundle once, project trusted
  request data, and evaluate with the real embedded Cedar engine.

Product applications own their action vocabulary and authoritative resource
ownership. Resource IDs must be unique across resource kinds within an organization
(e.g. application-qualified IDs); the crate treats them as exact opaque values.
A credential audience is checked separately from resource ownership. There are no
wildcard grants or implicit admin bypasses. Domain contracts remain generic; the
checked-in Cedar profile explicitly defines the initial Nessa action vocabulary.

`CredentialEvidence` is bounded and Debug-redacted. It is not serialized, cloned,
or displayed by the crate. Its explicit byte accessor is for trusted adapters;
it is not a memory-zeroization guarantee. DTOs contain metadata, never secrets.
External identity DTOs are untrusted data, not a mechanism for minting AuthContext.

## Authentication is not ongoing authorization

A trusted `CredentialVerifier` verifies the actual proof and returns its Nessa
binding ID plus expiry; a future hosted adapter must resolve a verified issuer
and subject rather than trust caller-provided identity mappings. An `AccessReader`
returns credential and membership from one coherent committed revision. Missing
records and provider/store failures must return errors, never an allow-all value.
The library is a boundary between trusted modules, not a sandbox for malicious
adapter implementations supplied by callers.

`AuthenticateSession` checks snapshot identity linkage, active membership,
audience, issued time, expiry, and revocation. Result expiry is the earlier of
proof and credential expiry. It performs no identity auto-creation or account
linking. An authenticated context carries stable selectors, not permanent roles.

The consuming gateway must reauthorize operations against current state, enforce
session expiry, subscribe to invalidations, and define admission/revocation races.
The returned revision enables that integration but is not itself a subscription
or guarantee that no revocation occurred after the read. `AuthorizeAction` invokes
`PolicyEvaluator` with fresh state; composition supplies `CedarPolicyEvaluator`.
Never expose product handlers based solely on successful authentication.

The auth `Clock::unix_seconds` measures absolute expiry time. The existing server
`Clock::elapsed_ms` measures uptime. They are separate context-owned ports with
different semantics; do not use uptime for expiration.

## Follow-on work and parallel ownership

Independent changes can target these modules after agreeing on their ports:

| Work | Owns | Depends on |
| --- | --- | --- |
| Local verifier/store (implemented) | `adapters/local/` | Evidence verification, durable lifecycle and coherent snapshots |
| Credential lifecycle (implemented) | `application/credential_admin.rs` and `adapters/local/` | Durable issuance, idempotent retries, revocation, and owner recovery |
| Policy evolution | Existing `adapters/cedar/` | Nessa-authored policies and authoritative resource inputs |
| Gateway integration (implemented) | `nessa-server/src/product/` | Verified session, current-state policy checks and bounded socket lifetime |
| Hosted identity | Future managed adapter | Verified binding mapping and authority/freshness rules |

One change should own a shared port/schema edit; coordinate that contract before
parallel adapters build against it. Keep DTO mappings in the application layer
and provider SDKs inside adapters. Local use must require no hosted signup.

Validation: `cargo test -p nessa-auth`, `cargo clippy -p nessa-auth --all-targets -- -D warnings`,
and `cargo fmt -p nessa-auth -- --check`.

## Cedar authorization flow

Construct `CedarPolicyEvaluator::new()` once at application startup. It validates
its checked-in schema and policies before accepting requests. Inject it into
`AuthorizeAction` alongside the same access reader and absolute clock used for
login. A failed constructor must stop policy composition, never fall back to an
allow-all evaluator.

1. `AuthenticateSession` verifies evidence and returns a bounded session.
2. Resolve the requested resource and its organization from trusted application state.
3. `AuthorizeAction` reloads current credential/membership state and checks session
   expiry, credential validity, identity linkage, and membership status.
4. Cedar evaluates the action, organization, membership role, and exact credential
   grant. Dispatch only for `Ok(Decision::Allow)`; Deny and errors both block it.

The initial profile permits `server.read` and `conversation.write` for active members/admins and
`credential.manage` for active admins. Both require an exact action/resource grant
and matching identity/organization. Unknown actions are denied. An admin's narrow
credential does not inherit broader permissions. Cedar evaluation diagnostics
containing errors are treated as failures even if another policy could permit.

`tests/cedar_flow.rs` exercises authentication through actual Cedar decisions using
injected test-only credential and state adapters. It covers current-state changes
rather than caching a decision from login. The `/session` gateway also has a real SDK lifecycle test in
`scripts/smoke-auth.mjs`.

No external authorization service is called. Policies are parsed once; each check
projects only its credential/membership/resource inputs. Performance at production
scale has not been benchmarked, and the gateway still owns request size bounds,
resource lookup consistency, and the admission/invalidation boundary.

Cedar 4.12.0 is pinned, with optional language extensions disabled. This crate
requires Rust 1.89 or newer; other workspace packages retain their existing minimum.
Run `pnpm auth:test`, `pnpm auth:clippy`, and `pnpm auth:fmt:check`; these checks are
also included in `pnpm check`.
