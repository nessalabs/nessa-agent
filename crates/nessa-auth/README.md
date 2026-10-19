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
  `Credential` produces a `CredentialTransition` (before, after, cause,
  initiator, time) for every issuance, explicit revocation, and automatic
  supersession; nothing assembles that evidence after the fact.
- `application/dto.rs`: serializable boundary data. Parsing is not authentication.
- `application/mapping.rs`: `TryFrom` validation into domain objects. Mapping an
  admin membership does not authorize storing it; that belongs in a trusted use case.
- `application/ports.rs`: object-safe asynchronous verification/state ports,
  an absolute-time clock, and a policy evaluator contract. No runtime is required.
- `application/credential_registry.rs`: typed, secret-free refusal evidence and
  the audit port used when registry bytes cannot be trusted.
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

## Lifecycle evidence is committed with the state

The local registry file keeps an append-only `transitions` list next to the
credentials and receipts. A mutation appends its transitions in the same atomic
write as the state change, so a failed write is a failed commit and there is no
"committed but unaudited" state to reconcile. `validate_registry` applies one
rule to that list before every write and on every reopen: each record satisfies
the domain, chains from the previous state of its credential, and ends at the
lifecycle the registry actually stores. A file edited by hand fails the same
way a bad live write would.

Causes are exactly the commands that exist: bootstrap, admin issue, surface
provisioning, exact device pairing, owner recovery, explicit revocation, and supersession by
provisioning or by owner recovery. Expiry is not a cause; nothing happens at
expiry and the instant is already in the issuance record. Automatic
revocations carry the initiator of the command that triggered them, and their
cause says they were automatic. The registry file is schema 2; earlier files
are not read, because the project is pre-alpha and carries no registry
compatibility.

Issue, bootstrap, and revoke results carry their committed transitions, and the
`CredentialTransitionReader` port lists them per organization. See
[credential transition audit](../../docs/design/auth/credential-transition-audit.md)
for the decisions.

Opening an invalid registry authority file is a separate audited event because
no registry state can be trusted enough to append evidence to it. The local
adapter reports the exact path, the registry-or-lock role, and a bounded
structural fault without including rejected values. The application derives a
role-specific preserved transition and records it with cause and the known
initiator in a private sibling audit directory. Audit failure remains visible
beside the original refusal. Neither path edits or deletes the registry.
The local refusal sink addresses every directory and record beneath the verified
auth root and refuses symbolic-link ancestry. Unix syncs each parent after
creating its child; Windows revalidates the tree, flushes the record file, and
uses a write-through move because it has no directory-fsync equivalent.

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

## Pairing producer contracts

`domain/pairing/` owns immutable exact consent, invitation state and device
binding values. `application/pairing/` owns entropy, clock, receiver staging,
private state and durable consent ports. `adapters/pairing/` owns the pinned
OPAQUE profile, actual TLS possession proof and shared private-storage consumer;
`adapters/local/registry/pairing/` commits pairing and credential evidence with
its coherent registry. Cryptographic transport carries raw messages; a server
consumer owns its application framing, listener and action admission.

Private acknowledgement compares bytes and named identity against the original
published file. Restart uses a writable existing-file handle, reflushes and
reconciles under the original directory/lock. No acknowledgement manufactures a
missing canonical file. Public adapter tests cover exact intent, same-save retry
and restart. The synthetic `private_state/recovery_tests.rs` cases retain real
published handles to test live identity/byte refusal and writable restart reflush;
decomposed failure observations also exercise consumer dispatch with those real
published handles. They do not inject a failed native acknowledgement through a
public save. Shared storage tests own the native post-rename failure fact. The
C324 table in the design records these boundaries.
Supported platform consumer gates are required independently of shared storage tests.

Target and current producer disposition: [device pairing](../../docs/design/auth/device-pairing.md).

The separate [owner-phone decision](../../docs/adr/todo/811-owner-phone-authorization.md)
adds a complete canonical consent set and a signed original request. The domain
`value_objects/owner_phone.rs` retains exact public source/request/key/pin/endpoint
correlation. `adapters/pairing/owner_phone.rs` alone verifies its device signature;
`VerifiedOwnerPhoneDeviceProof` is distinct from PAKE and TLS possession.
`application/pairing/owner_phone.rs` declares original receipt and proof-bound
read-only recovery ports. The local registry's `pairing/owner_phone.rs` commits
that original and the signed owner approval event in one registry entry;
`owner_phone_original.rs` validates the retained full-input fingerprint. Fresh
canonical admission evaluates management and every delegated capability together.
Later stage/publication checks retain the exact original issuer; another valid
owner credential cannot substitute. This Auth slice does not implement endpoint
or product/native phone transport, and does not change legacy per-chat policies.

`value_objects/owner_phone_context.rs` holds validated original source/pin/numeric
destination before a permanent device key exists. The native original port in
`application/pairing/owner_phone_private.rs` reserves that exact context before
entropy; `adapters/pairing/private_state/owner_phone.rs` uses FilePairingState's
existing directory, operation guard and publisher for Reserved/Prepared records.
Prepared stores the original seed and its crypto-derived public key, validates
their correspondence on cold load, and requires actual durability acknowledgement.
Legacy enrollment and original preparation exclude mixed records under that same
owner. Actual private fixtures live in `tests/adapters/private_state/owner_phone.rs`.
Coherent replacement of seed and public key is not authenticated evidence.

Native listener and protected product activation remain subsequent consumers;
this producer alone establishes no protected connection or resource acceptance.

`application/pairing/owner_phone_purpose.rs` publishes the trusted current
delegated identity projection. The registry adapter joins a freshly checked
session and the actual signed Active device record from the same revision;
historical bootstrap retirement does not withdraw the delegated credential.
`owner_phone_purpose_tests.rs` covers current revocation, membership loss, legacy
classes and a real revoke between snapshot and join. Receiver holding, current
epochs and Cedar method/target decisions remain the server's responsibility.
`NativeTransport::request_key_update` delegates TLS control emission to rustls;
the counted real transport regression checks flushed bytes before application
writes and preserved pinned identity, without implementing another TLS parser.

## Follow-on work and parallel ownership

Independent changes can target these modules after agreeing on their ports:

| Work | Owns | Depends on |
| --- | --- | --- |
| Local verifier/store (implemented) | `adapters/local/` | Evidence verification, durable lifecycle and coherent snapshots |
| Credential lifecycle (implemented) | `application/credential_admin.rs` and `adapters/local/` | Durable issuance, idempotent retries, revocation, owner recovery, and committed transition evidence |
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

The profile permits `server.read`, `conversation.read`, and `conversation.write` for active members/admins and
`credential.manage` for active admins. Each requires an exact action/resource grant
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
