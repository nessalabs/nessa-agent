# PR650 binding-winner correction — explicit exception after five rounds

Checkout `/workspace/nessa-agent-owned-publication`, branch `codex/628-owned-publication`.
Whole-PR base `f1682e8a763646c6c3e73dee73c1c47987e2cbae`; previous round5 clean reviewed head `3e65d96e44e27c502a6225ec16e554fe18c4a152`.
Correction head `b78bf641` (resolve exact SHA in manifest.json), clean/frozen. This is the user's narrow approved exception to finish the confirmed binding-winner defect, not a reset of the five-round review history.

Read AGENTS.md/CODING_STANDARDS.md and the prior complete `/tmp/628-required-review-brief.md` and round5 reports. Root owns fresh independent review, push, exact-head CI and merge. No subagent review was spawned by the coder.

The new 3-path correction refines canonical ADR329 row22 before source, adds a cfg(test)-only one-shot boundary after claim(None)/outer bound=false and before scoped absence, and changes scoped None from Incomplete to skipping absence. The existing loop reinspects rather than ending the owned drain. `processed_any` accurately names attempted processing/reinspection rather than implying actual closure. No lifecycle/outcome/controller/DTO or public seam is added. Graph remains lifecycle authority and existing admission scope remains binding/absence authority.

Two deterministic library regressions use actual public open_root and drop its queued unclaimed delivery without receiving its ID. Original owned reconciliation pauses at the exact loser boundary. Public bind_resources then accepts the actual cleanup owner behind Closing; release must finish the ORIGINAL drain without any end_lifetime retry, close that owner once, preserve OwnerDisposed/Runtime, and persist Closed/Released/Acknowledged with no absence claim. Gate-only public participation at the same boundary must retain sealed Closing/Incomplete and no settlement or absence claim.

Clean test-only RED commit `bb195df3` compiled and failed at the actual original-drain result assertion (Err(Incomplete) versus Ok); gate-only control passed. Minimal correction passed both. Deliberate compiled reversion of only the skip result to Err(Incomplete) failed the same runtime assertion with the gate-only control passing. Restored source SHA256 is identical and source was touched before subsequent checks. Exact commands/logs/patches/env are in manifest.json and run-gates.sh; pending checks are not passes.

Self-review dimensions: traced admission/owner identity, domain authority, lock/scope order, original root transaction and delivery ownership, notification/reinspection, provider physical report versus audit and storage, first cause/initiator, failure semantics, restoration/public transfer refusal, layer/module organization, typed failure/API surface, and canonical table/diagram/enforcer agreement. Gate15 ordering updated before implementation; gate16 uses existing state/loop; gate17 inapplicable (no UI/gateway/ACP/MCP behavior change). No source changes during running checks. No unresolved findings in the correction self-review. Existing absence-wins claim and audit failures remain guarded/domain-correlated; gate-only transfer is not manufactured as closure.

Fresh review must inspect whole PR and correction, try adjacent binding/absence/gate orders, and disclose checks and unverified limits. Separate accepted-absence-audit/final-write failure/restart gap stays explicitly deferred to #625/#646/#649 with the prior recorded actual REDs; this correction does not claim recovery persistence repair. Origin-runtime shutdown, panic supervision and blocking adapters remain prior scoped limitations. Platform runtime/coverage and current exact-head CI remain root-owned.

Correction ordering:

```mermaid
sequenceDiagram
    participant Ticket as Unclaimed root ticket
    participant Drain as Existing owned drain
    participant Caller as Cleanup owner caller
    participant Owner as Actual cleanup owner
    participant Scope as Admission scope
    participant Store as Ownership store
    Ticket->>Drain: Drop schedules OwnerDisposed reconciliation
    Drain->>Drain: claim(root)=None; outer bound=false
    Note over Drain: Test-only held boundary
    Caller->>Scope: bind_resources(root, actual_owner)
    Scope-->>Caller: Accepted owner transfer
    Drain->>Scope: Scoped absence decision
    Scope-->>Drain: Bound; skip absence
    Drain->>Drain: Existing loop reinspects and claims actual_owner
    Drain->>Owner: close(OwnerDisposed, Runtime)
    Owner-->>Drain: Released / Acknowledged
    Drain->>Store: Persist graph Closed / Released / Acknowledged
    Note over Ticket,Store: No end_lifetime retry; gate-only handoff supplies no owner and stays Closing/Incomplete
```

Final implementer gates completed exit0 on frozen `b78bf641825d77c8be2e88f9ac76b1f14ed2a8dc`: workspace fmt; SDK static docs; all74 architecture tests plus checker; CI six-package all-target Clippy with -D warnings; isolated SDK full965lib/401application/212domain/65infrastructure/38doctests; CI six-package combined tests and doctests using the actual concurrency2 script; warning-denying SDK rustdoc. `gate-status.txt`/logs/manifest retain exact statuses and source/log SHA256. No remaining local check is pending; independent review/current CI/platform coverage remain root-owned. Tree clean and source digests unchanged.
