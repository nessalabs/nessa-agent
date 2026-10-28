# Independent review: original issuance and secret-free registration correction, round 1

Reviewed frozen candidate `ee0d8eff159d96eeb70161f1c3ad774ae16da558` on `codex/896-webhook-composition`, correction base `97d5c0991310cf7e10729e3f1cb3c2371be06460`, implementation `a3f5578e1f626cefe031fa04d6d61c6df4571a10`, final source/test checkpoint `946bbc8647b681fbd34440a116ecb3fb8efe5a6f`. Checkout was clean at entry and exit. `git diff --name-only 946bbc864..ee0d8eff -- crates` was empty; candidate changes after source checkpoint do not change crate source/tests.

Result: one Minor finding; no Blocker, Major, or Nit findings. This round is not clean. The demonstrated failure is recovery classification, with admission still refused and retained state preserved. This review is not #896 completion or merge permission.

## Findings

### Blocker

None.

### Major

None.

### Minor

**M1 — accepted ambiguous AdminIssue histories can return Unavailable before the explicit recovery boundary.**

Locations: `crates/nessa-auth/src/adapters/local/registry.rs:627` (receipt replay calls `transitions_of_command`, then rejects an empty result at 629); `registry.rs:1520` (selection requires original principal initiator and original command correlation); `crates/nessa-server/src/webhooks/application/registration.rs:159` (propagates auth failure before the origin inspection at 262–264).

Reachable trigger at the supported restoration boundary: reserve an intent and retain its matching auth receipt/metadata; restore its historical original cause as AdminIssue, with either (a) absent original correlation or (b) another existing principal as original actor. Both are accepted by the current registry restoration contract, which deliberately restricts the receipt/actor/correlation relationship only for the newly distinguished WebhookIssue. The restored access reader successfully supplies the original AdminIssue in both cases. Registration repeats the receipt, but command-based transition selection finds nothing, returns `LocalStoreError::Corrupt`, and maps it to `Auth(Unavailable)` before the new typed recovery branch can run.

Expected: ambiguous accepted AdminIssue origin reaches the explicit `OriginRecoveryRequired` result. Actual: `Err(Auth(Unavailable))`. A permanently ambiguous retained history is presented as auth unavailability. Initial and retry safety remain intact: no registration is installed, one original private intent remains, and auth bytes are unchanged. Severity is Minor because this is an actionable recovery/error-contract gap rather than an authority or secret disclosure bypass.

Reproduction: appended independent test `independent_ambiguous_admin_origin_without_command_has_typed_recovery` in `crates/nessa-server/tests/webhooks/infrastructure/outbox/registration_origin.rs` under this report directory. It fails with left `Err(Auth(Unavailable))`, right `Err(OriginRecoveryRequired)`. See `ambiguous-test.txt` and separately repeated `ambiguous-repeated.txt` (each exit 101). `independent_recovery_gap_preserves_state_for_both_accepted_historical_shapes` passes and establishes the actual outcome and unchanged state independently for absent correlation and different existing actor; see `gap-state-controls.txt`.

Positive counterpart: existing `ambiguous_historical_admin_origin_is_preserved_and_requires_recovery` passes when the historical AdminIssue still matches the original principal/correlation. The independent historical Product test also passes with principal `hook` and correlation `legacy-original-command`, preserving the original record and ordinary session entry. The fix must retain acceptance/preservation of valid historical actors and correlations, including missing correlations. It must not synthesize or rewrite them or make all AdminIssue histories obey the new WebhookIssue receipt promise. The new recovery result belongs to registration; general Product replay recovery is outside this finding.

This probe constructs accepted legacy restoration shapes deliberately. It does not claim these actor/correlation substitutions are produced by the current issuing command, nor that an arbitrary public origin constructor creates authority. The authority in the reproduction is the actual reopened local auth adapter.

### Nit

None.

## Independent execution and evidence

All scripts, probe source, binaries and logs are under `/tmp/nessa-origin-independent-review`. No implementation changes, checkout changes, primary target writes, public listener, network recipient, provider call, live credential, account grant, CI action, or publication occurred. No tool action was denied or flagged in this review. No historical blocked reviewer report was retrieved, reconstructed, or delivered.

Toolchain: `/tmp/nessa-cargo/bin/rustc`, Rust 1.99.1, `CARGO_HOME=/tmp/nessa-cargo`, `RUSTUP_HOME=/tmp/nessa-rustup`. Package metadata explicitly declares Auth MSRV 1.89; this review did not execute MSRV qualification. Shared compiled third-party and Nessa dependency artifacts were read from `/workspace/nessa-agent/target/debug/deps`; Auth itself was independently compiled from current actual source into this directory, then linked into the private registration harness. This avoids relying on the supplied auth rlib as evidence of the reviewed Auth implementation. `compile.py`, `auth-args.json`, `server-args.json` preserve the commands and exact dependency paths. The harness driver covers actual copied current webhook application/domain/outbox modules, not a production product route. Its small Target declaration stands in for an unchanged consuming type; composition is inspected separately.

| Check | Independent result | Evidence |
| --- | --- | --- |
| Current Auth actual-source library compilation | Pass | `auth-compile.txt`, `auth-args.json` |
| Current Auth actual-source unit tests | 214 passed, 0 failed | `auth-unit-compile.txt`, `auth-unit.txt`, `auth-unit.py` |
| Current public origin integration tests | 5 passed | `origin-compile.txt`, `origin-tests.txt`, `run.py` |
| Current supplied registration tests | 30 passed | `registration-baseline.txt` |
| Supplied registration plus first 3 independent probes | 33 passed | `registration-expanded.txt` |
| Final supplied registration plus all 4 positive independent probes | 34 passed; one intentionally failing recovery expectation excluded and executed separately | `final-registration.txt` |
| Recovery classification expected contract | Fails, repeated | `ambiguous-test.txt`, `ambiguous-repeated.txt` |
| Recovery actual outcome and state preservation, 2 shapes | Pass | `gap-state-controls.txt` |
| Remove held-origin actor comparison privately | Regression fails, exit 101 | `actor-mutated-hunk.txt`, `actor-mutated-compile.txt`, `actor-mutated-test.txt` |
| Restore actor comparison | 33 passed | `actor-restored-compile.txt`, `actor-restored-tests.txt` |
| Privately permit webhook_issue in session.enter Cedar policy | Real bearer/resumed refusal regression fails, exit 101 | `cedar-mutated-policy.cedar`, `cedar-mutated-auth-compile.txt`, `cedar-mutated-server-compile.txt`, `cedar-mutated-test.txt` |
| Restore Cedar policy | 33 passed | `cedar-restored-auth-compile.txt`, `cedar-restored-server-compile.txt`, `cedar-restored-tests.txt` |
| Both mutated private source files byte-identical to checkout after controls | Pass | `control-restoration.json` |
| Base-to-candidate git diff whitespace check; final frozen identity and cleanliness | Pass | `checkpoint.txt` |

Initial independent adjacent probe compilation had a partial-move error in the temporary test's final assertion. It was corrected to inspect the remaining store field directly; the failure log remains in `adjacent-compile.txt` only if not superseded by the successful rerun (the successful final compilation is separately preserved). It was not a production failure. Scripts retain reproduction commands; `adjacent.rs` is the initial probe draft, while the copied `registration_origin.rs` is the final complete probe source.

## Agreement across fields and layers

Read AGENTS.md, CODING_STANDARDS.md including required brief, review loop, agreement, dimensions, all severity buckets/evidence closure, architecture, codebase structure, typed dependency injection, and the actual correction diff. The review brief identifies the exact checkout/base/head and frozen state. Ownership remains in the existing domain/application/adapters and composition boundaries; no second authorization owner was introduced.

| Related facts and owner | Enforcer inspected | Regression / independent evidence |
| --- | --- | --- |
| Original issuance identity/lifecycle/cause/actor, journal sequence/revision and optional command | `application/origin.rs:18` validates representation through domain transition; `matches` at 62 checks current identity/lifetime and committed revision. Domain and DTO mappings are exhaustive for WebhookIssue. | All 5 origin tests; current Auth unit tests. Public construction validates facts, while injected access/admission supplies authority. |
| First actual issuance versus current revoked metadata | Local `registry.rs:1250` reads first journal fact with current coherent credential/membership; `validate_transitions:1562` checks global sequence/revision, domain chain and final metadata before publication/restoration. | Existing original-origin revocation test; independent cold reopen after actual revoke retains exact origin and refuses bearer and resumed entry. |
| Newly issued purpose, grant constraints and command receipt | Actual issuing owner `registry.rs:694` maps WebhookIssue; restore `1832` selects existing webhook grant policy from original cause; `1898` binds precisely one webhook receipt to original principal and correlation. | Existing malformed restoration tests; independent six additional cases: future/zero revision, wrong expiry, empty correlation, LocalOperator webhook actor, duplicate credential receipt. Every case refuses cold open without rewriting auth bytes or installing a registration. |
| Reserved request identity/config versus returned issuance and held authority | `registration.rs:145` resolves retained proposal; 228 validates metadata; current hold and lifetime checks at 236–260; 262–276 compares actual original actor, command, lifecycle and journal identity/revision against the returned DTO; install only at 279. | Existing 19 reply contradictions, actual actor and command substitution, coherent future-origin reply, unavailable/mismatched holds; actor-removal independent control proves held authority is load-bearing. |
| Secret lifetime and initial/replay acknowledgement | `registration.rs:119` acknowledgement has only registration id and metadata; 168–180 consumes outcome and explicitly drops evidence before later awaits. | Existing synthetic secret marker absence in result/database/audit and matching restart replay; source tracing verifies early drop. Ordinary Product issuance still returns one-time evidence from Auth. |
| Bearer and opaque resumed entry versus original issuance | Both application entry paths call single `admit_session` at `session.rs:137,201,221`; Cedar attributes publish cause; `policies.cedar:159` owns session.enter decision; evaluator identity consistency includes origin at 118. | Actual-source Auth tests, all original cause tests, real proof verification then bearer/resumed refusal and ordinary Product success; independent Cedar mutation fails regression. |
| Product/browser/paired composition carries existing policy owner | Production `product/socket.rs`, `browser_session/entrypoint/http.rs`, `composition/paired_environments.rs` inject the existing policy at every changed session entry site. | Source/diff inspection; supplied product regression fixture asserts recorded session.enter followed by denied tested action. No fixture assertion was removed to bypass policy. These full route suites were not independently rerun here. |
| Legitimate historical Product facts versus newly distinguished webhook restrictions | Restore intentionally leaves non-WebhookIssue actor/correlation interpretation intact; no historical rewrite in reader. | Existing LocalOperator/null historical Product preservation and independent different-existing-principal/nonreceipt-correlation Product entry and byte preservation pass. M1 exposes the adjacent registration recovery classification boundary. |
| Revocation/retirement versus session purpose | Original journal fact survives current revoke and private outbox retirement; registration presence is not the policy input. | Existing retirement and stopped registration tests plus independent cold revoked restore. No Product send authority is supplied by registration or session.enter. |

## Limits and excluded completion claims

Independent execution is focused actual-source Auth plus a private webhook/outbox harness. I did not rerun seven-package Clippy, full canonical server/webhook/product suites, architecture scripts, docs renders, browser CI/stress/chaos, wildcard-listener fixtures, recipient capability/revoke E2E, full expiry/outbox/reset coordinator qualification or MSRV. The supplied evidence README/logs were read as supporting reported results, not independently certified gates. The documented unrelated Codex CleanupUnconfirmed failure is not attributed to this correction and no full-suite-green claim is made.

This correction remains unmounted: Product registration protocol/mount, W1 admission/lifetime ownership, sender/source/replay supervision and complete revoke/expiry coordination remain #896 tasks. #1052 general app installation/no-binding principals and inbound #1088 are excluded. Existing expiry/outbox/shared-reset implementation is base behavior; relevant hold/retirement relationships were inspected without claiming a fresh complete lifecycle review. Documentation Chromium is not canonical browser qualification. No remote exactly-once or complete feature readiness claim follows from this review.
