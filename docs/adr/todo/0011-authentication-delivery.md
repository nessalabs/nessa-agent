# 0011. Complete authentication delivery through the guarded gateway

- **Date:** 2026-09-06
- **Status:** proposed
- **Builds on:** [0010 — local auth](../done/0010-local-authentication.md)
- **Related:** [0007 — session protocol](0007-nessa-session-protocol-and-authorities.md), [0009 — harnesses and tools](0009-agent-harnesses-and-optional-tools.md)

## Context

The local credential library, owner bootstrap/recovery, token lifecycle, Cedar
checks, SDK/CLI, and mandatory gateway routes are implemented. That does not
complete platform release validation, administrative UI, operational hardening,
or the wider identity design. Panel product access and automatic native credential
loading are now implemented, as is Windows private ACL storage. The [review](../../reviews/local-auth-gateway.md) records current
limits; [design references](../../design/auth/README.md) explain the target.

## Decision

Extend authentication through the existing application ports and guarded gateway.
Do not restore shared-token routes or treat local implementation as completion
of the remaining delivery work. Keep each unfinished area explicit:

| Work remaining | Completion evidence |
| --- | --- |
| Panel recovery and credential administration UX | Product-session migration and native credential loading are implemented; broader in-app onboarding/recovery and administration remain |
| Credential administration UI and listing pagination | Settings can administer credentials with current authorization; bounded listings have protocol and SDK coverage |
| Platform release validation | Windows private ACL creation, reading, rotation, and rejection are implemented; execute the checked-in Windows/Linux lifecycle and ACL CI tests |
| Registry capacity and recovery | Retention/compaction preserves revocation and retry guarantees, and capacity exhaustion does not strand owner recovery |
| Gateway concurrency and invalidation | Per-operation admission and slow-socket isolation tests are implemented; connection quotas, stream checkpoints, and revision notification integration remain |
| Policy operations and performance | Policy update/reload behavior is specified and tested; latency/load and failure benchmarks cover real enforcement |
| Broader identity and tenancy | Explicit authority mapping, membership provisioning, deployment audiences, and provider failure/invalidation contracts are implemented and tested before enabling an additional provider |

The broader identity work remains a proposal. The
[identity and tenancy reference](../../design/auth/identity-tenancy-and-cloud.md)
records alternatives and research; it does not select or install a provider.
MCP packaging stays in ADR 0009; conversation permissions, replay, and agent
execution stay in ADR 0007.

## Consequences and completion

Local users retain the working SDK/CLI flow from ADR 0010 while these changes
are delivered. This record remains in `todo/` until the listed scope is completed
or explicitly revised into narrower decisions. A primer, design example, or
passing local test is not evidence that an unfinished integration exists.
