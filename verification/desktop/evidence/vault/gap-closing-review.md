# Fresh closing review: retention-gap wire contract

## Result and exact scope

**No findings at any priority in the changed wire-contract repair.** The initial
review's audit-evidence finding is closed by the authoritative encrypted audit
assertions, rejection triggers and recovery checks now present. This is a fresh
closing round; I read the implementation and its neighboring owners rather than
adopting the first review's conclusion.

Reviewed HEAD `3494d2aa873e831a8d551b21aaa2a5def0ab2088` with the seven
uncommitted/new files listed below. Ordered path-NUL-content SHA-256:
`716c76068900baf504ea36e6b506717fac95d14919336039ebf91156fa80fa1a`.
The review applies to that source snapshot, not arbitrary later edits.

## Boundaries and agreement verified

- The production delta is the `WireGap` camelCase mapping. The Worker returns
  `expiredThrough` with HTTP410 and the current stage; the native decoder now
  maps that same receipt into `Failure::Gap`. Wrong stage/kind still refuse, and
  missing/snake_case watermark is not accepted through an alias. No schema/API
  version, authorization rule, dependency or production origin configuration
  changes.
- The bundled Worker's real expiry response is compared with the shared fixture.
  Both the TypeScript and Rust include paths resolve to the same
  `protocol/fixtures/github-relay-gap.json`. The fresh Worker fixture's first
  delivery has sequence1, so its watermark matches this receipt. The native
  test sends it through a real loopback HTTP response and the actual
  `HttpRelaySource::page`, not only a serde helper or injected typed failure.
- `catch_up` owns watermark admission before storage: it rejects a nonadvancing
  watermark and a value above the safe-integer bound. The negative test crosses
  HTTP, application and encrypted storage for stage, kind, missing, snake_case,
  zero and above-bound cases, with unchanged cursor/events and no cloud-gap or
  acceptance audit rows.
- The received gap persists through a fresh store handle and a nonaccepting bind.
  A repeated paused catch-up returns the stored gap before another HTTP request;
  the request counter and unchanged single audit record establish that ordering.
  The original target is user11/repository42, before0/through1, cause
  `cloud_retention_gap`, actor `system:cloud-relay`. It is not mislabeled as an
  explicit human decision.
- A trigger refusing the cloud-gap audit causes a typed Storage outcome with the
  original cursor and no stored gap/evidence. Removing it allows the same HTTP
  receipt to retry successfully. A separate acceptance-audit trigger leaves the
  stored gap and its original evidence intact; recovery produces the owner row
  before0/after1 with `local-owner:github:11`, then the valid trailing page commits
  cursor2 and one event. This checks actual durable sink outcomes and rollback,
  not a returned in-memory evidence object.
- The new loopback test uses direct `RelayStore::bind` for owner acceptance. I
  separately traced the existing Settings path and
  `opt_in_survives_restart_without_resetting_the_cursor_and_disable_fences_old_pages`
  in `tests/github_relay/inbox.rs`: `LiveEvents::status` counts the saved Gap,
  `set(..., accept_gaps=true)` uses `select`, and acknowledgement installs the new
  binding generation. The new test does not claim to drive the UI itself.
- The private unit-test child constructs the existing injected-client adapter for
  loopback HTTP without widening production visibility or adding a test-only
  runtime flag/constructor. The shipped public constructor remains HTTPS-only.
  Its explicit per-client ring configuration avoids global crypto-provider
  mutation. Empty roots do not disable certificate validation in production;
  external TLS itself is outside this HTTP serialization test. The owned server
  task is aborted on fixture drop, and requests have a five-second deadline.
- Files and maps remain with the relay owner; the protocol receipt is shared
  intentionally by producer and consumer. Rust imports use short type names.
  The ADR table precedes this behavior repair and describes the existing gap
  policy rather than creating another state machine. Worker lease fingerprint
  inputs, cloud deployment/stage rollout, vault ownership and UI source are not
  modified by this delta.

## Evidence inspected

I did not run Cargo, providers, browsers or source mutations while the coordinator
owned the frozen build. I independently read these completed logs:

- `/tmp/nessa-1158-gap-red-wire.log`: actual HTTP assertion fails with
  `left: Some(InvalidResponse)` / `right: Some(Gap(1))` without the rename.
- `/tmp/nessa-1158-gap-green-final.log`: all three transport/storage regressions
  pass after restoration, including invalid receipts and audit refusal/retry.
- `/tmp/nessa-1158-gap-github-tests.log`: the eight-package GitHub selection reports
  **70 passed**, including the new tests and existing inbox/Settings tests.
- `/tmp/nessa-1158-gap-clippy.log`: strict all-target eight-package Clippy completed
  successfully. Final process receipt remains coordinator-owned.

The earlier crypto-provider and private-directory fixture failures are excluded
from revert proof. They failed before the wire assertion and do not establish the
production defect. `git diff --check` passed independently in this review.

## Separate unchanged-owner observation

**P3, outside this seven-file repair: audit-cap maintenance in direct SQLite gap
and bind paths needs separate disposition.** In unchanged
`infrastructure/sqlite.rs`, `gap` inserts into `audit` but trims `binding_audit`;
`bind(..., true)` likewise trims only `binding_audit`. A gap appended when payload
audit already holds `AUDIT_ROWS` can leave it over that declared cap until a later
selection or page commit trims it. Repeated direct bind/gap cycles also do not
trim payload audit themselves. The Settings `selection.rs` path does trim both
and is not the omitted path. This is source-derived, not an executed new runtime
probe; no production change or extra test is requested in the wire fix by default.
The coordinator should track/disposition this pre-existing owning-adapter case
separately under scope gate4, rather than interpret this scoped review as proof
of all existing retention bounds.

Coordinator disposition: accepted as a separate follow-up, with runtime probe
pending. The follow-up will identify the direct gap/CLI-bind paths, and the state
page's blanket bounded-audit wording will be qualified before merge. This concerns
local encrypted audit metadata; it is not a demonstrated cloud payload-quota or
confidentiality failure. The issue link is coordinator-owned and must be attached
before the merge handoff. No production expansion of the wire repair is requested.

## Limits

This review closes the local wire repair and the initial audit-evidence finding.
It is not a re-audit of the full stacked feature or a new external penetration
test. It does not establish native TLS, deployment, full-platform CI for the new
source, performance, or broad desktop-suite success. Documentation checkpoint
updates and the final-head hosted run remain coordinator-owned. Previously
recorded baseline UI failures are outside this unchanged UI delta.

## Reviewed files, in digest order

- `crates/nessa-server/src/github_relay/infrastructure/transport.rs`
- `crates/nessa-server/tests/github_relay/transport.rs`
- `protocol/fixtures/github-relay-gap.json`
- `packages/github-webhooks/tests/relay/relay.test.ts`
- `docs/adr/todo/1152-local-vault.md`
- `protocol/README.md`
- `packages/github-webhooks/README.md`

## Coordinator disposition

The separate P3 local audit-cap source observation is tracked in
[#1185](https://github.com/nessalabs/nessa-agent/issues/1185), added to the project
backlog. It remains source-observed with runtime reproduction pending. The relay
state page now qualifies its previous blanket audit-cap wording. The wire repair
stays scoped; no general retention/security approval is inferred.
