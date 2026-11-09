# Independent registration review, round 1

**Not clean: one major and one minor new finding.** No primary source was edited.
The known immutable-purpose/product-session gap is separately demonstrated below;
it is not claimed as a new independent registration finding.

## Checkpoint and scope

- Checkout: `/workspace/nessa-webhook-runtime`, branch `codex/896-webhook-runtime`.
- Exact frozen candidate: `7041ac656528c88776ebb0931500a8667f52a1df`.
- Registration implementation source: `460a1c85935973f367cfd5fd033bcb81dd181469`;
  initial registration source `c0ef8c35736ffd34645f39e4d1310f93132dcc1e`.
- Intended review base: `501a81dca92d0af7f7d8fa457bd1ec409cdf3b89`.
- Primary checkout was clean at entry. Committed-range `git diff --check` passed.
- Read AGENTS.md, coding standards gates, required review brief, all review
  dimensions, agreement, audit, policy, seams, dependency injection, architecture,
  owning codebase map, ordering design and affected state/evidence pages.
- Reviewed all changed production files across this base/head (auth request serde,
  registration application and export, intent persistence, shared row budget,
  install/registry/transaction integration, restoration, schema and moved schema
  validation), their new tests and unchanged adjacent auth issuance, auth DTO/domain
  transition mapping, access holds, audit publication, original reset references,
  retirement and product ordinary-issue dispatch.

## Priority buckets

### Blocker

Empty within this inner registration slice. Full feature acceptance and security
classification remain unqualified as documented under exclusions and K1.

### Major — R1: returned reservation replaces original caller command/configuration

Location: `crates/nessa-server/src/webhooks/application/registration.rs:101-106`.

`execute` passes ownership of the original proposal to `RegistrationIntents.reserve`,
then uses the returned intent as the sole reference for auth issuance, result
correlation and installation. No application check ties its caller-controlled
request, destination or MAC key to the original command. The built-in adapter's
replay comparison does not enforce that relationship for a substituted adapter.

Concrete counterexample: an isolated `RegistrationIntents` wrapper changes only
its received destination to `https://different-recipient.test/changed` and key to
synthetic `[87;32]`, then delegates reserve and install to the real `SqlOutbox`.
Actual Cedar/local auth issuance, held credential/member and all SQL retained facts
are coherent. `RegisterWebhook.execute` returns `Issued`; `registration` contains
the substituted URL/key rather than the caller's original URL/key. No HTTP request
is made. These are individually valid facts for the wrong original command.

Expected: reject the returned intent before calling auth or installing a destination,
without adopting changed caller facts as authority. Actual: issue and install succeed.
This violates the original-input agreement and adapter-substitution boundary in
CODING_STANDARDS gates 5/13 and Agreement across fields and layers. A future source
and sender would consume the wrong registered recipient, so this must be fixed
before composing them.

Evidence: `registration.rs.probe.diff`, test
`independent_substituted_reservation_changes_destination_before_auth_and_install`;
`probes.txt` and `additional-probes.txt` both reproduce current acceptance.
Those passing demonstration tests are bug evidence, not fixed regression passes.

Structural direction discussed with coordinator: one application-owned
retained-intent command-correlation predicate, shared by application validation
immediately after reserve and built-in SQL replay. Only server-selected
registration/credential IDs and original observation/issuance-time projection may
differ on a retry. Full original caller request/configuration/key/purpose must
remain bound. Exact installation equality is a distinct relationship. I found no
additional independent constraint that makes this correction insufficient for R1;
K1 below concerns a later, different boundary and is not solved by this predicate.

### Minor — R2: transient row-step failure becomes malformed-state Invalid

Location: `crates/nessa-server/src/webhooks/infrastructure/outbox/intents.rs:213`.

The iterator from `query_map` can yield SQLite execution failures as well as typed
column/format failures. `row.map_err(invalid)` treats all of them as malformed
persisted state. Existing `cursor::row_error` already owns the correct distinction
and is used by the adjacent registration query and other reads in this file.

Concrete counterexample: with a fully valid real reserved intent, a test-only SQLite
progress callback interrupts execution during row stepping. `intents::validate`
returns `Invalid` at interruption thresholds 23 through 35, 46 and 47. Removing the
callback makes the exact unchanged rows immediately validate successfully.

Expected: typed `Unavailable` for interruption/storage execution failure; keep
`Invalid` for malformed values. Actual: valid storage is reported malformed.
This violates typed failure/degrade-honestly contracts (gates 1/5/7) and can cause
future orchestration to treat a recoverable storage interruption as corrupt state.
Use the existing row-error owner; no new classification system is necessary.

Evidence: `registration.rs.probe.diff`, test
`independent_transient_step_interruption_is_reported_as_invalid_intent` and
`additional-probes.txt`. The callback exists only in the isolated review test;
no production hook or security setting was changed.

### Nit

Empty.

## Known boundary K1 — purpose and principal kind are absent from returned proof

This was explicitly identified in the review brief and remains an open design
boundary, not an additional new registration finding. It must be resolved before
claiming the complete no-product-session / integration-only feature contract.

A `ChangedIssue` wrapper changes the real issuance request *inside CredentialAdmin*
from Webhook/Integration to Product/Human while preserving all other original
fields. The actual auth owner creates a Human principal and ordinary credential.
The returned metadata, issued transition and current hold match every fact checked
by registration; it installs successfully. Neither metadata nor current hold
carries issuing purpose/principal kind, so original-command correlation at the
reserve boundary alone cannot detect this later substitution.

Evidence: `registration_failures.rs.probe.diff`, test
`independent_substituted_auth_purpose_and_kind_pass_existing_correlation`, with
real auth `principal_kind("hook") == Human`; `probes.txt` and
`additional-probes.txt`. This demonstrates bypass of the intended webhook-only
issuing classification, not conversation.send or management authority: the
credential still has only the original conversation.read grants. It does not
justify inventing a parallel policy/receiver/application-installation model.
An auth-owned immutable use fact consumed by the existing Cedar/product-entry
owner is the coordinator's pending structural direction. No scope1052 changes
were made in this review.

## Independent checks and retained evidence

- Built a private `/tmp/nessa-registration-independent-review` source/test copy.
  `source-match.json` records the complete initial exact-copy hash set;
  `production-source-match.json` confirms every copied production webhook file
  still matches the frozen primary bytes after adversarial test additions.
- Rust1.99.1 through the designated `/tmp` toolchain; exact compiler args retained
  in `args.json`. Dependency artifacts were read from the existing target;
  no shared build artifact was rebuilt, cleaned or invalidated.
- Independently compiled and ran the copied outbox/domain/registration baseline:
  **84 passed, 0 failed**, `baseline-compile.txt` and `baseline.txt`.
- Added two adversarial demonstrations: **2 passed**, `probes-compile.txt` and
  `probes.txt`. Added the transient-interruption demonstration: **3 passed**,
  `additional-compile.txt` and `additional-probes.txt`.
- The isolated harness compiles the actual copied registration, outbox, delivery,
  destination and message source; it defines a field-identical secret-free Target
  DTO and excludes conversation admission/transport composition. It is not the
  full canonical server/browser/E2E suite. Real compiled auth/Cedar/local storage
  owners are used by registration fixtures and the counterexamples.
- Inspected the implementer's exact-source evidence:114 canonical webhook tests,
  212 auth tests, 7-package all-target Clippy `-D warnings`, server all-target check,
  configured architecture, format and 135-page docs validation; 21 fail/restore
  probes and preserved earlier defect/fixture/coverage failures. These are
  implementer runs, distinct from my independent runs above.
- Reviewed effective package metadata: auth and server explicitly require Rust1.89;
  this review did not incorrectly inherit the workspace version or execute an
  unavailable1.89 toolchain. No new code requiring a newer standard API was found.

## Agreement and lifecycle review

- Original caller/config -> returned reservation: **R1 fails**, owner missing at
  the application boundary. Built-in retry equality does not substitute for it.
- Original issue -> current returned credential and issued transition: metadata
  identity/tenant/audience/time/expiry/grants, exact correlation and one typed
  auth-domain transition are checked. The original transition can coexist with
  later current revocation metadata. **K1** names the missing classification fact.
- Current membership/credential -> installation: complete held values are compared,
  inactive/expired/revoked facts refuse, and the hold spans normal install/ack.
  A hold does not imply absolute expiry suspension or supervised cancellation;
  the inner use case explicitly requires an outer physical owner/W1 coordinator.
- Installed row -> retained original intent: mandatory FK, exact target/config/key
  and original observation correlation; malformed type/identity/cross-record tests
  refuse before publication. R2 is the transient execution classification gap.
- Registration -> retired history/audit: transactional private-intent deletion keeps
  existing secret-free original actor/target/stop evidence; lost acknowledgement
  and legitimate retirement/restart remain accepted in baseline tests. Reset fact,
  first-capture membership and reference retention stay with their existing owner.
- Resource accounting: retained intent count64, issuance JSON16KiB, whole-row borrowed
  caps before restored owned allocations, schema row-count/text caps and physical
  worker permits4 were inspected. Product ingress/command queue budgets are still
  uncomposed and are not claimed by these storage constants.
- Auth grant decisions remain in existing Cedar; keys/URLs provide no read or send
  permission. Intent persistence is proposed input, not an admission receipt.
  Endpoint/redirect/SSRF and remote acknowledgement mechanics are unchanged in this
  slice; their previous tests are supporting evidence, not new E2E qualification.
- State pages retain exact source460a1c85 and explicitly mark pending work. Historic
  finding and fixed-source evidence are distinguished; no exactly-once remote
  effects claim is introduced. Original workflow-trigger/app-installation contexts
  remain separate.

## Exclusions and gates still required

No product configuration/mount, W1 owner, automatic pending-command recovery,
canonical revoke/expiry orchestration, committed source reader, sender settlement
owner, capability/revoke/cancel/cleanup recipient E2E or live listener is implemented
or qualified by this review. These are genuine remaining task requirements, not
waived by reviewing this inner slice. Fresh exact-head independent review is needed
after R1/R2 corrections; this round is not clean.

Canonical Chromium/WebKit scripted verification, feature stress/chaos at default
and raised settings, supported-platform CI and exact-head CI remain required. I did
not attempt forbidden browser downloads, workflow lookup/dispatch, the denied
foundation1091 job retry120322813252, CLI/alternate triggers, no-op pushes or any
live recipient/credential/production action. No tool action was denied, pending
approval or uncertain in this review. No primary source, Git branch, public PR,
account credential or external application was modified by this review.
