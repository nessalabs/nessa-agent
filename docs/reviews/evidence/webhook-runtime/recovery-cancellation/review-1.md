# Cancellation and Mac TLS fixture independent review, round 1

Reviewed frozen candidate `710572454a9c5e95a076993d8f7be511ca1c9ced`, source checkpoint `566cf419781203209666df344a65b3f213876560`, against published base `6bece0d25649aca981d414cc7be2492b4257eb70`. Scoped commits: `30a8e8b22`, `05f416db9`, `566cf4197`, documentation `710572454`. Candidate includes main `8ee1eb2e8e6f41628a9595b383c3bb89f519d6f2`; unrelated parent-qualified UI/test changes were excluded from source review. Checkout was clean at review start. The author later appended a temporary reviewer-designed race probe as sole source writer; no reviewer source/ref/target changes were made.

Read AGENTS.md, coding standards gates, local review gate, required brief, review dimensions, agreement across layers and evidence/closure; consulted codebase structure, architecture and typed dependency injection. Inspected the complete scoped production diff, cancellation tests, touched test adapters, bounded row tests, state pages, design ordering table, unchanged transport/application observation mapping and relevant stop/drain owners.

## Findings

### M1 — major — ordinary revoke-between-reply-and-hold ordering bypasses canonical cleanup

Location: `crates/nessa-server/src/webhooks/application/registration.rs:273` and subsequent original-origin/stop checks.

Trigger: actual Auth owner commits a valid webhook issue, its caller captures the successful issue outcome, an actual explicit revocation commits before the registration use case acquires its Auth hold, and the hold returns the actual revoked current credential with its valid original issuance and revocation journals. The request, credential identity, configuration and original issue are unchanged.

The complete current credential now differs from the earlier issue-reply metadata solely in revocation lifecycle. The early equality returns `RegistrationFailure::Inconsistent` before consuming the actual held revocation, so no cancellation/stop sink is called and the private reservation remains. Expected for this designed ordering: canonical stopped cleanup under the held actual fact, no destination installed, `Stopped` (or `StoppedCleanup` if the sink fails), and reservation removal when acknowledged. A later authenticated retry may repair it; that does not classify the current ordinary ordering correctly or fulfill its immediate cleanup contract.

Reviewer designed concrete probe `/tmp/nessa-896-review-revoke-after-issue.rs`: a `CredentialAdmin` decorator captures the real issue outcome, invokes the real store's `revoke_sync`, returns the captured outcome, and uses the real store's normal admission hold. Author runs it serially, with fixture clock advanced to20,007ms after constructing the original10s intent. Confirmed by serial author execution: **zero passed, one failed**, actual `Err(Inconsistent)` versus expected `Err(Stopped)`; original failure preserved at `/tmp/nessa-896-review-revoke-after-issue-before.log`. This uses an actual issue reply followed by actual revocation at20s and a current20,007ms observer, not a fabricated contradictory snapshot. A repair must continue rejecting changed identity, grants/configuration, original issuance, correlation and fabricated/missing stop facts; it must not simply drop all credential agreement checks.

### D1 — minor — diagram conflates installed stop acknowledgement with drain

Location: `docs/state/services/gateway/webhook-issuance.md:126`.

`Held --> Retired` on original stop of an installed registration skips the live Stopped state, while `Retired --> final` claims registration/intent deletion. The ordinary stop owner may leave an admitted Sending row responsible for its outcome and retain the stopped registration. Restore Stopped for installed handoff, and allow Stopped to reach Retired only when retained work drains. This needs a diagram correction, not another runtime state owner. Author accepted D1.

No blocker or nit finding. Cancellation timestamp equality was considered and explicitly rejected as an unsupported hypothesis: cancellation event time and first stop observation are separate facts and need not be forced equal absent a contract.

## Independent executed checks

- Existing compiled current-source test binary, `webhooks:: --test-threads=4`: **190 passed, zero failed/ignored**,19.68s. Log `/tmp/nessa-896-cancellation-review-webhooks.log`. This includes all15 cancellation regressions, actual TLS request timeout, pre-send expiry/no-connection test, and original full-boundary retention tests. No compilation or target writes by reviewer.
- Independent in-memory canonical-schema EXPLAIN probes confirm indexed `retirement_credential`, `registration_audit` for the five-row retained-history query with rowid ordering and no temporary B-tree, and covering unique live-credential lookup. `/tmp/nessa-896-cancellation-review-query-plans.json`. These are SQL-plan checks, not synthetic Auth authority or production restore evidence.
- Inspected author's13 mutation-control outcomes and their scopes; they ran before final inactive-member/Mac corrections and are not described as exact-final-tree independent controls. The author also preserved the pre-fix inactive-member failure and post-fix passing regression. Author's initial wrong test filter matched zero and is not counted.

## Agreement and boundaries checked

- Auth owns original issuance/revocation journal identity, current credential and membership. Application correlates reserved issuer/request, selected IDs, issue transition sequence/revision/correlation and current hold; inactive membership does not become install authority. M1 concerns the ordinary evolution between reply and hold, not forged values.
- `RegistrationIntents` owns saved original input and typed installed/cancelled selection; `cancellation::inspect` correlates target/issued time and original stop before `run_checked` publishes owed evidence. The connection lock retains preflight selection through finishing. Installed work delegates to the existing `RegistrationStops`, with the same Auth hold retained through sink acknowledgement/error.
- Uninstalled cancellation atomically appends its audit/retained terminal kind and removes the private intent; destination/key never enter secret-free audit. Lost writes and lost acknowledgement retain replay identity/first observer. No temporary installation/credential is manufactured.
- Shared indexed retirement credential selection rejects duplicate retained owners and live/retired overlap. Cold restoration and owed publication call the same contextual/history owners before file effects or trim. The retained kind refuses cancelled/drained cross-history; duplicate stop/expiry facts, competing initial UUIDs and impossible fifth fragments are covered, with valid distinct-owner and four-fact drained controls. Byte checks include the new kind before allocation; schema comparison refuses old shapes.
- Mac fixture receives a complete HTTP request through genuine loopback TLS before advancing virtual time across the unchanged150ms client deadline. The keeper prevents paused-clock auto-advance while real TLS progresses, with the original3s watchdog retained. The151ms tick accounts for Tokio timer granularity; no production15s deadline changed. Expired preparation remains a separate before-poll no-connection/permit-release case. Timeout mapping remains typed TimedOut and terminal (no automatic retry); incomplete/unproven response mappings remain Unknown.
- New ports use explicit injected dependencies, typed errors preserve stopped primary meaning plus cleanup failure, module maps identify the owning persistence paths. No new Cedar policy or policy bypass; no UI/product conversational entry point was mounted. Design table covers the new inner lifecycle and states that runtime composition remains separate.

## Exclusions and remaining gates

No independent Mac/Windows execution, exact-head CI, browser/docs visual rendering, stress/chaos or complete recipient capability/revoke E2E qualification. Windows45-minute timeout remains unresolved; finite Linux completion does not rule out a Windows-specific I/O stall. Reviewer did not retry/dispatch CI or cross any held operation. Author-reported seven-package Clippy/fmt/architecture results are supporting evidence, not independently rerun compiler checks.

Runtime command supervision, startup reconciliation, shutdown ownership, source/share mounting and supervised sender remain uncomposed. This review does not complete #896, authorize merge, qualify parent UI changes or change any prior access holds. Round1 is not clean while M1/D1 remain open; after fixes and author controls, a fresh full round is required by the repository review loop.
