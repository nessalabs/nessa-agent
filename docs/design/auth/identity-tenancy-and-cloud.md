# Identity, tenancy, and cloud authorization

- **State:** reference for proposed broader identity integration; local foundations implemented in [ADR 0010](../../adr/done/0010-local-authentication.md)
- **Work tracking:** [ADR 0011](../../adr/todo/0011-authentication-delivery.md)
- **Research date:** 2026-09-04
- **Implementation slice:** [Scoped authentication](scoped-authentication.md)
- **Parent:** [ADR 0007](../../adr/todo/0007-nessa-session-protocol-and-authorities.md)

Nessa should support local and hosted individual use, with hosted deployment the
expected path for most team/enterprise users. Deployment location and customer
mode are independent choices. This document records that target without expanding
the first local auth implementation into a cloud platform.

## Direction and implementation boundaries

Nessa defines and implements its own identity model, tenancy boundaries, product
policies, and enforcement. These are required for local-only use as well as future
hosted use. Embedded Cedar can evaluate Nessa-authored policies; it does not own
the domain or require an external service.

For hosted B2B, we may use Clerk or another managed provider for login,
credential lifecycle, federation, and membership provisioning while we build the
experience and operational capacity to run those capabilities ourselves. That is
an adapter choice, not a switch away from Nessa's policies. Local-only users keep
the local implementation and need no signup. Clerk is a researched candidate,
not a dependency required for implementing the local foundation.

An eventual internally operated hosted provider should replace the managed
adapter only when it passes the same contracts and we can maintain it: security
review, load/stress and failure testing, recovery drills, credential rotation,
revocation under concurrency, monitoring, and an operational support owner. Local
correctness alone does not demonstrate hosted readiness. There is no fixed date
or requirement to replace a managed provider before those conditions are met.

## Local implementation commitments

1. Use stable Nessa principal, organization/ownership, and resource IDs independent
   of device names, emails, and provider IDs. Provision the local personal
   ownership container without online signup.
2. Implement Nessa's product policies and tenant/resource checks behind mandatory
   gateway middleware. Credential verification uses an application-owned port
   supplied by local trust/bootstrap today.
3. Own the application DTOs and domain types. Adapters verify and translate
   external evidence into Nessa DTOs; application use cases resolve identity
   bindings and invoke domain behavior with domain values. Domain objects import
   neither application DTOs nor provider/JWT/webhook types.
4. Keep local and hosted authority explicit. Connecting an account must never
   silently upload, merge, or transfer ownership of existing local work.
5. Separate identity acquisition from conversation state and UI lifecycle. Local
   use must not initialize a hosted SDK or require a network call.

Implement these boundaries as local features are built. Org administration UI,
cloud synchronization, enterprise federation, and provider migration machinery
arrive with their actual consumers. Keep the code modular inside the existing
server; no additional authentication service deployment is needed for local use.
The [scoped authentication plan](scoped-authentication.md) details the
proposed auth slice; this design does not mark that slice implemented.

## What existing products establish

These are representative documented patterns, not evidence of a market-wide
majority or knowledge of these products' internal implementations.

| Source | Documented pattern | Implication for Nessa |
| --- | --- | --- |
| [GitHub account types](https://docs.github.com/en/enterprise-cloud@latest/get-started/learning-about-github/types-of-github-accounts) | Personal accounts, organizations, and enterprise accounts are distinct; enterprise accounts can manage organizations | Keep actor identity separate from resource ownership and membership |
| [Clerk organization settings](https://clerk.com/docs/guides/organizations/configure) | Personal accounts are optional alongside organizations, with an active organization context | Personal-as-organization is a Nessa simplification, not a universal requirement; org selection must be verified |
| [Slack enterprise organizations](https://docs.slack.dev/enterprise/) | An enterprise organization can encompass multiple workspaces | A future enterprise grouping is possible; do not introduce a second tenant hierarchy before Nessa needs it |
| [AWS tenant isolation](https://docs.aws.amazon.com/whitepapers/latest/saas-architecture-fundamentals/tenant-isolation.html) | Authentication and authorization alone do not establish tenant isolation | Carry organization scope through data access, caches, jobs, streams, and execution |
| [AWS policy-store strategies](https://docs.aws.amazon.com/prescriptive-guidance/latest/saas-multitenant-api-access-authorization/avp-design-considerations.html) | Shared and per-tenant policy stores have different isolation and operational tradeoffs | Explicitly choose policy/data isolation; a Cedar dependency does not make that decision |

## Account-optional local use

Local users can use Nessa without signup or a hosted account. Local installation
identity and the personal ownership container are provisioned locally, not through
Clerk. This does not bypass local gateway authorization. Hosted/team features
require their own verified account and organization access. Connecting an account
never uploads, transfers ownership of, or merges local conversations automatically.

The planned frontend integration loads the hosted provider only after explicit account access. Missing
configuration or provider failure must leave local functionality usable. The exploratory
Clerk UI integration was removed after feasibility checks; it did not implement
product `/session` authentication, local bootstrap, or cloud synchronization.

## Recommended model

Use one Organization as the ownership/tenant boundary in both personal and team
modes. Personal signup/bootstrap creates one automatically, with an active admin
membership. Joining a business creates a separate membership; it never converts
personal resources into company resources. Enterprise-managed onboarding may go
directly into the company organization without creating an unused personal one.

```text
Principal ── Membership(role, state) ── Organization
                                          │
                                  owned product resources

Credential ── Principal + Organization + Audience + restricted grants
Deployment ── hosts one or more organizations
Team (later) ── belongs to one Organization, groups memberships
```

An individual may belong to multiple organizations. An organization may have many
members, teams, workspaces, conversations, and execution workers. Team is a group
inside an organization, not a deployment mode or a new tenant. Treat enterprise
features as organization configuration/entitlements initially; defer an Enterprise
entity spanning organizations until there is a concrete requirement.

Keep `organizationId`, `principalId`, `membershipId`, and deployment/audience IDs
separate and stable. Membership roles are org-local. Resource IDs and cache keys
must be scoped by organization; server-owned resource lookup verifies ownership.
An org admin role does not imply access to every private conversation: future
resource policies must explicitly decide content access versus administration.
Billing entitlement and authorization are distinct checks; buying a feature does
not grant every member permission to use it.

## Smallest useful runtime layout

Start with one gateway/application deployment containing authentication,
organization/credential management, and embedded Cedar policy evaluation as
modules. Cloud workers may be separate for execution isolation, but creating an
auth network service is not required. Cedar remains an embedded library; managed
Verified Permissions is an alternative deployment choice, not a required part of
using Cedar. [Cedar Rust integration](https://github.com/cedar-policy/cedar).

| Concern | Local first slice | Hosted target |
| --- | --- | --- |
| Human authentication | Explicit private OS-owner bootstrap | Established identity provider integration, with enterprise SSO/provisioning later |
| Auth state | One process, private registry, one org | Transactional shared store with tenant-scoped access, multiple orgs |
| Token verification | Local credential provider | Hosted verifier mapped to the same verified context; explicit issuer/audience checks |
| Authorization | Cedar in gateway middleware | Same policies and adapter boundary in each enforcement process |
| Membership/grant freshness | Committed local revision and socket invalidation | Revisioned tenant snapshots, invalidation, bounded freshness and resynchronization |
| Recovery | Offline local personal-admin recovery | Verified account/org recovery, never the local filesystem bootstrap command |

Prefer an established identity provider for login and federation; selection is
separate from choosing Cedar. Map an external identity by verified issuer and
subject, not email alone. Nessa owns its resource model and scoped tool grants.
Whether memberships originate in Nessa or are provisioned from an enterprise IdP,
designate one authority per field; do not create competing membership databases.

A login can enumerate authorized organizations. Selecting one produces an
org-bound product session after current membership verification. Use one org per
NessaClient instance; a multi-org UI may own several explicit instances. Credential
rotation and org switching establish new authenticated contexts. Never accept a
caller-supplied organization ID as sufficient authority.

The current exact `gatewayId` audience represents one local gateway. Before cloud
replication, generalize the verifier's audience to a logical gateway deployment,
shared by authorized replicas, with issuer trust separate from host identity.
Do not bind hosted credentials to one ephemeral pod or accept local credentials
in cloud. Local-to-cloud linking requires verified enrollment and scoped issuance.

## Provider independence through domain boundaries

Clerk is one future hosted adapter candidate. Nessa owns the contracts below;
choosing Clerk does not make its user, organization, role, or token model the Nessa
model. Implement one Identity and Access bounded context within the existing
server. Domain and application code depend on ports they own; infrastructure
adapters depend inward on those ports. This requires no additional deployed
service or generic authentication framework.

```text
auth/
  domain/           Principal, Organization, Membership, CredentialBinding, Grant
  application/      AuthenticateSession, IssueCredential, RevokeCredential
                    ports for verification, credential management, persistence
  infrastructure/   clerk/, local/, cedar/, persistence/
  entrypoint/       RPC and verified provider-event translation

composition/        chooses adapters and configured authorities
```

These are logical module responsibilities; follow repository layout conventions
when implementing them rather than adding empty layers. Conversation/workspace
contexts keep their own ownership data and provide typed resource facts to access
checks. They do not import Clerk SDKs or query Clerk organizations directly.

### Dependency injection and adapter selection

Select the implementation once in the composition root using explicit deployment
configuration, for example `identity_backend = local | clerk | internal`. These
are proposed configuration values, not implemented flags. Tests inject a
controlled implementation directly. Client requests cannot choose the backend.

```text
Deployment configuration
        │
        ▼
Composition root creates configured adapter
        │
        ▼
Application-owned verification / credential-management ports
        │
        ▼
Same middleware → same use cases → same domain policies
```

Use constructor injection (Rust traits behind concrete generics or `Arc<dyn ...>`
where runtime selection is useful). Keep one small adapter factory in composition;
no service locator, global mutable auth singleton, or `if clerk` branches in
handlers. Separate narrow verification and credential-management ports rather
than requiring every provider to implement one large interface. Inject repository,
clock, and policy evaluation dependencies through the same composition boundary.

After a compatible adapter is implemented and configured, switching the backend
should be a configuration change and restart/redeployment. A provider adapter may
need several cohesive files for verification, API calls, and event translation;
“one adapter” means one isolated module, not forcing unrelated responsibilities
into one file. New providers may add their module and one factory registration,
without changing domain objects, application DTOs, ordinary handlers, or NessaClient.

Supported capabilities are explicit. Composition validates required capabilities
at startup; optional operations expose a typed unavailable/unsupported result.
Changing to a provider with fewer capabilities must not silently weaken grants,
remove middleware, or fall back to local trust. Keep local-only and hosted profiles
separate so local mode never requires cloud configuration.

Shared contract tests establish behavior parity before the selector changes.
Switching the selector is operationally simple only after identity bindings,
credential migration, membership authority, and trusted issuers are prepared.
Existing sessions are drained or reauthenticated on cutover; hot-swapping the
identity authority of an active socket is outside the initial design.

### Contracts owned by Nessa

| Boundary | Contract |
| --- | --- |
| Domain identity | Nessa-generated stable principal, organization, membership, and credential-binding IDs |
| External identity mapping | Unique `(provider configuration, verified issuer, external subject)` to Nessa principal; separate verified external-org and credential mappings |
| Authentication port | Verify opaque credential evidence for a configured audience and return normalized proof, expiry, and credential/session reference |
| Credential administration port | Issue/revoke managed credentials where supported, returning normalized results and private adapter references |
| Auth repository | Persist Nessa identities, bindings, grant restrictions, and versioned authorization state |
| Policy boundary | Typed Nessa action/resource/context to decision; Cedar types stay inside its adapter |
| Session boundary | AuthContext and SessionReady expose Nessa IDs and product permissions, never provider claims as the product schema |

An external proof is not yet AuthContext. The application resolves trusted
bindings, organization membership, credential restrictions, and current state;
only then may middleware create the verified Nessa context. Bindings are not
created from arbitrary client IDs or email matches. Provider secrets and raw
claims stay within the adapter; logs contain normalized, redacted failures.

Keep verification separate from credential administration. A provider may verify
human session tokens without allowing Nessa to mint those tokens; interactive
login remains that provider's flow. Model explicit supported capabilities and
return `unsupported_operation` for an unavailable operation, never emulate it
with broader credentials. The local adapter implements offline issue/revoke;
the hosted adapter delegates supported machine-credential lifecycle operations.

### Authority and synchronization

Nessa is authoritative for its internal IDs, resource ownership, product actions,
and additional credential restrictions. The configured identity provider is
authoritative for its login identities and credential validity. For a future managed-provider deployment, the provider can own provisioning and base admin/member membership;
Nessa maintains a versioned projection mapped to its own IDs. That projection is
not an independently editable copy. Local mode uses Nessa's local store as the
membership authority. Record the authority on the organization and permit only
one writer for each field. Future provider migration changes that authority
explicitly.

Provider events enter through an adapter that verifies authenticity and translates
payloads into application updates. Handle duplicates, stale/out-of-order events,
deletions, and missed deliveries with idempotency, authoritative reconciliation,
and the freshness rules below. If source events have no reliable ordering, fetch
current authoritative state rather than treating arrival order as truth. An
unrecognized role must not become admin. Do not forward raw webhook payloads as
Nessa domain events or require the external stream crate for auth synchronization.

Remote issuance and local binding persistence cannot share one transaction. Track
pending/active/revoked binding state and a Nessa operation ID. Use upstream
idempotency where available; otherwise reconcile an ambiguous timeout before
retrying creation. A remotely created credential is unusable in Nessa until its
binding/grants commit. Record and clean up orphaned remote credentials. Local
deny takes effect before remote revoke completes; report remote cleanup failure
separately instead of claiming global provider revocation succeeded.

### Client boundary and replacement test

The hosted UI may use Clerk's login components inside a small authentication
integration module. NessaClient depends on an application-owned credential source
that obtains/refreshes opaque credential evidence; it does not import Clerk or
assume JWT fields. The mandatory server verifier selects only configured trusted
providers and validates issuer/audience. A client provider hint cannot select test
behavior or broaden the issuer allowlist. No Clerk SDK types enter protocol schemas,
MCP/CLI tool contracts, domain entities, or ordinary product components.

Provider UI replacement is expected integration work; do not recreate a complete
universal login UI to hide it. Keep the change contained to login, credential
acquisition, and the server adapter. Product sessions, permissions, and resource
ownership should remain unchanged.

Run a shared adapter contract suite against the local implementation, deterministic
test implementation, and hosted adapter integration environment. Cover invalid
issuer/audience, expiry, restricted credentials, unknown mappings, disabled
memberships, provider outage, and unsupported capabilities. Prove two different
external identities can be deliberately mapped to the same Nessa principal during
a controlled migration without changing conversation ownership or granting access
across organizations. Test that unlinked identities cannot do this. Enforce import
boundaries so provider dependencies cannot spread into domain or SDK code.

A provider swap consists of adding an adapter, importing/linking verified identities
and organizations to existing Nessa IDs, reconciling memberships, changing the
configured authority, and retiring the old issuer. During a bounded migration,
explicitly allow both issuers only for linked identities and preserve restrictions;
keep one membership authority. Users may need to sign in again, machine credentials
may need reissuance, and enterprise SSO may need reconfiguration. Passwords, MFA
secrets, and active sessions are not assumed portable. “Quickly swappable” means
contained code changes and preserved product data, not a zero-work migration.

## Isolation without a network hop for every policy check

Load a tenant-scoped auth snapshot and parsed application policies into each
serving process. Supply only the selected tenant's entities to Cedar. Start with
one application-owned, versioned policy bundle and data-driven memberships/grants;
defer tenant-authored policy text and policy-store-per-tenant operations. This is
an embedded Nessa design choice, not use of a shared managed AVP policy store.

For the first hosted deployment, keep the application modular and use a shared
transactional store. Add per-tenant in-memory snapshots as needed with revisioned
invalidation. A cache is usable only while its freshness contract holds. Missing
updates, reconnection, or an expired lease require authoritative refresh or denial.
Notifications alone do not guarantee immediate revocation across replicas.

Before multi-replica launch, define and test a numeric maximum revocation delay.
For operations requiring immediate global revocation, use an authoritative
transaction/check at admission or route admissions through one tenant authority;
that stronger guarantee has a consistency/latency cost. Do not promise both fully
offline decisions and instant remote revocation. Initial local performance
measurements must not be presented as hosted benchmarks.

Cedar admission checks do not isolate a database query or an agent process.
Enforce organization scope in repositories, list queries, jobs, object storage,
stream subscriptions/replay, and caches. Workers receive scoped task authority
and reauthorize queued work at execution. Provider harnesses running customer
code need an execution isolation design; authorization is not their sandbox.
Cross-organization collaboration is initially denied and needs a future explicit
sharing model. Do not introduce it through generic messaging grants.

## Delivery order and launch gates

1. **Local foundation and auth slice:** implement mandatory `/session` middleware, embedded Cedar, stable org
   and membership IDs, personal bootstrap, restricted credentials, and local
   invalidation. Keep the spike path unchanged. No dependency on the stream crate.
2. **Hosted individuals and teams:** a managed identity adapter when needed, transactional
   tenant data, verified org selection/invites, logical deployment audiences,
   TLS/session protections, tenant-scoped data access and execution. Reuse the SDK
   and policy boundary; replace local provisioning and storage. Require two-tenant
   isolation tests across RPC, lists, streams, jobs, cache, and worker dispatch.
3. **Enterprise:** SSO and provisioning/deprovisioning, team/group mappings,
   audit records, recovery/last-admin rules, org-managed credential policy, and
   measured multi-replica invalidation. Define offline access rules for enrolled
   local workers. Dedicated deployments can be added without changing org IDs.

4. **Optional internally operated hosted identity:** replace the managed adapter
   after contract parity, security and stress testing, recovery validation, and
   operational readiness. Preserve Nessa IDs and product policies, reconcile
   memberships, and plan explicit reauthentication/credential rotation.

## Clerk feasibility findings

The CLI was installed and linked to the Nessa development application, with
ignored local configuration retained. A temporary React integration demonstrated
that account UI could be loaded on demand while local use stayed outside the
provider. Its runtime code and dependency changes were removed after the scope
was clarified. No production deployment was configured.

Sign-in/account-creation controls rendered in the browser. Actual signup, native
OAuth, gateway verification, automatic organization provisioning, and domain
identity mapping were not validated. This establishes limited UI feasibility,
not implemented identity capabilities or a commitment to Clerk.

The first slice does not claim cloud or enterprise readiness. Its acceptance
criteria should demonstrate that these additions do not require changing what a
principal, membership, organization, credential, or authorization decision means.
