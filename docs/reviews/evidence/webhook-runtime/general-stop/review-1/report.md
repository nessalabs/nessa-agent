# Independent round ONE: general credential-stop correction

**Disposition: changes required.** Blocker: none. Major: M1. Minor: none. Nit: N1, N2. This is not a clean review and does not qualify runtime composition, publication or merge.

Reviewed frozen checkout `/workspace/nessa-webhook-supervisor`, base `c7fe656af15346a21c293eb9a5bb5d5cf27c54b0`, head `0aa1a479ad6a81ad088fb5b368b9c013d3812ab7`, source checkpoint `c2bbb2705fe03ec77efc727460893cbadd264639`. The head adds documentation only over c2; source/test diff c2..head is empty. The checkout remained clean and unchanged throughout. This is a fresh review of the new correction, not retrieval or reconstruction of any historical blocked report.

## Findings

### M1 — Major: retirement restoration clones an unbounded identity before its budget/UUID check

Location: `crates/nessa-server/src/webhooks/infrastructure/outbox/retirement.rs:130–131`, reached by the retirement validation owner during outbox restoration. `SELECT id FROM retirements LIMIT ?1` bounds row count but `let id: String = row.get(0)` allocates arbitrary text before `uuid(&id)` rejects it. The shared bounded `SavedRetirement::row` projection does not include this outer identity. The schema has no identity text-length CHECK. Thus the new borrowed-row checks do not cover every restored identity.

Trigger: corrupt persistent `retirements.id` containing 1 MiB of text. Expected: typed `OutboxError::Invalid` before an owned identity allocation. Actual: the same typed refusal, but only after a 1,048,576-byte Rust allocation. Larger SQLite values can scale that allocation; startup can suffer memory exhaustion before the promised fail-closed resource boundary. This requires malformed local persisted state, not a demonstrated remotely reachable input. SQLite's own storage/page allocations are outside this specific Rust-allocation measurement.

Independent probe `review_outer_retirement_identity_allocation_probe` uses actual schema and retirement validator, inserts the malformed row before enabling a private System-allocation recorder, then tracks only validation. Output at the reviewed source: `OUTER_RETIREMENT_ID_MAX_ALLOCATION=1048576, input=1048576`. The desired assertion `maximum < 4096` fails. A diagnostic change ONLY in my private copy reuses `codec::check_row_bytes(row, 0..1, 36)` before the existing String/UUID owner. It returns the same typed Invalid and prints maximum allocation **0**; the desired assertion passes. This is a diagnostic alternative, not an implementation fix or an approval of the reviewed head.

Scope: the outer String line is inherited unchanged from c7, rather than introduced in c7..0aa. It remains in the same unpublished retirement/restoration stack and directly violates the complete #896 bounded-restoration/resources requirement and the correction's shared borrowed-row claim. Parent explicitly accepted this scope classification. Severity reflects unbounded allocation on an explicitly supported malformed-restoration boundary, not unauthorized-send authority.

Small owner-level alternatives:

- **A:** borrowed UUID-size guard before the existing outer read, retaining the current UUID parser and later projection. Smallest change, but keeps two row decoding paths and a repeated read.
- **B:** include registration id in the shared bounded SavedRetirement projection; validate/read/by_credential consume that checked representation. This removes the separate unbudgeted identity path and can avoid the extra read without adding history or parser semantics. Parent selected B for the subsequent implementation; it is not present at this reviewed head.

Evidence: `/tmp/nessa-general-stop-review-outer-id-allocation.txt` (demonstration), `/tmp/nessa-general-stop-review-outer-id-desired-before.txt` (desired contract fails), `/tmp/nessa-general-stop-review-outer-id-desired-after.txt` and `outer-id-after-tests.txt` (diagnostic passes). Preserve a regression requiring early typed refusal and valid adjacent restoration, not the demonstration assertion that expects the bad allocation.

### N1 — Nit: new test type references/imports violate the repository's Rust import rule

The Rust import rule requires types imported at the owning module top and used by short names, including test imports. New additions use qualified Auth/stop types or helper-local imports. Exact new sites, excluding inherited unrelated style:

- `tests/webhooks/domain/registration_lifecycle.rs`: 23, 24, 101, 102, 117, 118, 123, 124, 139, 140.
- `tests/webhooks/infrastructure/outbox.rs`: 288, 291, 543, 546, 598, 601, 919, 922, 1212, 1216, 1217, 1265, 1266, 1271, 1278, 1649, 1652, 1693, 1696, 1763, 1766, 1971, 1974, 2107, 2110, 2160, 2163.
- `tests/webhooks/infrastructure/outbox/issuance_context.rs`: 97, 100.
- `tests/webhooks/infrastructure/outbox/lifecycle.rs`: 139, 141, 323, 326, 523, 524. The new helper-local AccessAdmission use is at 323.
- `tests/webhooks/infrastructure/outbox/lifecycle_context.rs`: 262, 265.
- `tests/webhooks/infrastructure/outbox/registration.rs`: 497, 500.
- `tests/webhooks/infrastructure/outbox/registration_origin.rs`: 239, 242.
- `tests/webhooks/infrastructure/outbox/source_reset.rs`: 294, 297.
- `tests/webhooks/infrastructure/registration_install.rs`: 90, 91, 94–97 (new helper-local Auth imports).

Paths above are relative to `crates/nessa-server/`. Group appropriate imports at module scope; preserve test-only compilation and behavior. No unrelated import cleanup is requested.

### N2 — Nit: inner use-case documentation describes future composition as present

`crates/nessa-server/src/webhooks/application/registration_stop.rs:118–120` says “Composition retains this inner future across caller loss”. The actual command/startup supervisor is explicitly unimplemented. State a caller obligation (“Composition **must retain**…”), preserving the explicit statement that this inner helper is not a command supervisor. No new supervisor is required to close this wording finding.

## Scope, agreement and architecture checks

Read the full review brief, AGENTS.md, CODING_STANDARDS.md review dimensions/agreement/bounded process and relevant gates, module structure and architecture documents, source module maps, and local nessa-docs instructions. Inspected the complete 33-file c7..0aa production/test/doc diff and adjacent existing owners. Production scope: Auth application revocation decoder; registration lifecycle domain; application registration-stop observation; outbox stop codec, audit/publication, registry, retirement, restoration, journal/store/intents and schema. Test scope includes Auth revocation, webhook domain, outbox lifecycle/origin/reset/retirement/row tests and real Auth registration fixture. Documentation scope includes implementation design, state, module map and evidence README.

Relationships checked together:

| Related facts | Authority/enforcement and evidence |
| --- | --- |
| Shape-only retained record versus current credential binding | `RevocationRecord` is immutable shape data; `CredentialRevocation::restore/matches` checks current credential. Actual Auth integration suite and current-binding mutation distinguish them. No shape object is treated as authority. |
| Original journal identity, actor/cause and command/effective time versus local observation | Actual Auth issuance/revoke owner supplies the original fact; RegistrationStop/SavedStop preserve it separately from first local observation. Shape-valid substitutions and earlier/later clocks were exercised before publication and through cold restore. |
| Explicit, supersession and offline actor attribution | Closed cause/initiator mappings, shared saved stop projection and original fact comparison; canonical fixtures plus principal namespace/duplicate-field probe. `LocalOperator` is not synthesized as an authenticated principal. |
| Live/retired/audit correlation and acknowledgement | Shared stop codec, registry preflight/run_checked, retirement correlation and existing restoration owner. Failed publication and ack-loss retries cannot replace original facts; same record with later local time succeeds while preserving the first observation. |
| Credential ownership and ambiguity | Existing active_principal uniqueness plus indexed retirement credential lookup with LIMIT 2, ambiguity rejection and query-plan/row-limit tests. No new credential ownership table inferred or proposed. |
| Held current credential snapshot versus sink acknowledgement | `ObserveRegistrationStop` retains actual AccessSnapshot through sink await; actual Auth revoke is blocked until acknowledgement. Moving the drop earlier fails the existing real-Auth hold test. |
| Expiry and clock rollback | First observation and actual issued/expiry seconds are distinct; fractional issuance/backward local clock valid control passes. Current-observation helper's NoStopObserved is not sender admission. It is not the startup reconciler. |
| Retained progress versus erased proof | Existing W24 eight-table summary (registrations, registration_intents, cursors, deliveries, audit, retirements, source_reset_facts and source_reset_members) remains owner. Fully trimmed prior expiry does not imply permanent proof; rollback/replay/cold reopen positive control demonstrates the stated post-trim limit, not a fabricated stronger history contract. Future mount must reconcile retained original facts first. |
| Restoration resource count and payload | Borrowed budgets checked for new shared rows and joins, maximum valid cases and indexed lookup tests pass; outer retirement id remains the concrete M1 gap. |

Authority/domain/admission/failure/audit/representation/resource/public-surface dimensions were inspected where applicable. No new mutable sender authority, grant-policy owner, permission table, bearer persistence or grant/Cedar bypass was found in this correction. Domain lifecycle remains the owner of stop attribution; Auth remains the current authority and original-fact owner. The inner observer's future still requires an outer retained supervisor. Invalid supplied facts are checked before owed audit publication; successful acknowledgement remains distinct from remote delivery effects.

Gate15 was checked against the existing state/design order and tested identity/prepublication/ack-loss sequence; M1 is a representation-budget defect, not a new lifecycle semantic rule. Gate16 remains a narrow reuse of Auth and existing outbox owners rather than a new history/permission system. Gate17 and the gateway scripted browser requirement remain unqualified as described below. No UI, SDK dispatch/provider, MCP or ACP implementation changes were reviewed. Product/config/protocol/share/source admission/sender composition and recipient E2E remain explicitly incomplete. Those absences are not concealed as completion of #896 and are not waived by this narrow review.

## Independently executed checks

All compilation/source changes were confined to `/tmp/nessa-general-stop-independent-review`. Copied actual Auth and webhook source/tests from frozen0aa. Used private rustc outputs and immutable dependency snapshot `/tmp/nessa-retention-dependency-snapshot`; did not mutate the frozen checkout, `/tmp/nessa-stop-fact-frozen`, shared Cargo target, or another worker's source. Structural private driver replaces only the unrelated Target fixture shell as disclosed by the author assembly; Auth source and relevant owner/application/domain source are real copies. This is not a canonical seven-package Cargo invocation.

- Actual Auth unit suite: **214 passed** (`auth-unit-tests.txt`).
- Actual Auth revocation integration suite: **8 passed**, and passed again after current-binding mutation restoration (`auth-revocation-tests.txt`, `auth-restored-integration-run.txt`).
- Private actual-owner/domain baseline: **139 passed** (`/tmp/nessa-general-stop-review-baseline.txt`).
- Four additional reviewer tests combined with baseline: **143 passed** (`/tmp/nessa-general-stop-review-final-suite.txt`).
- Additional allocation demonstration and desired before/after control: described under M1. The diagnostic guard plus all reviewer/owner tests passed **144/144** (`/tmp/nessa-general-stop-review-diagnostic-full.txt`). Afterward all copied Auth/webhook production Rust files were verified byte-for-byte against frozen source; the desired failing allocation test remains only in private reviewer fixtures. Final restored-source control passed **143/143**, excluding only that deliberately failing allocation test (`/tmp/nessa-general-stop-review-restored-final.txt`).
- Independent nessa-docs validation: **135 documents, zero issues** (`docs-validation.txt`). Did not independently render browser diagrams; author reports three inspected renders with system Chromium151, which does not establish canonical browser qualification.

Additional reviewer inputs:

1. `review_closed_stop_codec_and_actor_namespaces`: duplicate top-level cause/initiator/atMs/revocation keys and nested sequence/revision/at/initiator/cause keys, including equal duplicates, reject. A shape-only principal named local_operator cannot substitute the offline namespace.
2. `review_fractional_issuance_and_backward_local_clock_restore`: local issuance10999ms/Auth issued10s, actual revoke101s, local observation9999ms; live stop and cold restoration preserve distinct clocks correctly.
3. `review_every_original_auth_fact_family_is_checked_before_owed_publication`: mutate actual canonical retained original actor, cause, command/effective time, credential and issuance/expiry facts after retirement ack-loss; all shape-valid substitutions return Invalid before hidden audit-directory publication. Original fact with later local clock succeeds unchanged.
4. `review_clock_rollback_retired_registration_replay`: fully acknowledged/trimmed expiry followed by clock rollback and registration replay/cold reopen remains valid under the retained-proof limitation. Not reported as new current-observer defect.

An initial reviewer fixture used DTO key `command` rather than actual `supersession`; it failed during fixture construction. That was corrected, not counted as an implementation failure. The corrected test passed. Both logs remain (`/tmp/nessa-general-stop-review-original-fields.txt`, `...-corrected.txt`).

## Independent falsification / mutation controls

Six isolated rule mutations were compiled and caught by meaningful tests; all restored with new source writes and positive checks. Five detailed results are in `mutation-results.json` with per-mutation compile/test logs:

- Replace first local observation with Auth time: backward/fractional-clock test fails.
- Remove saved actor projection equality: namespace/codec test fails.
- Remove original-record mismatch refusal: actual explicit-fact/first-observation test fails.
- Remove prepublication check: different actual credential cannot publish test fails.
- Release held snapshot before sink await: actual Auth revoke-hold test fails.
- Remove current-Credential binding in `CredentialRevocation::restore`: actual Auth revocation integration suite exits101; original source/rlib restored and all8 pass. `mutation-current-binding-run.txt` preserves subprocess test failure; its detailed integration assertion log was overwritten by the restored positive run, so no stronger individual-assertion evidence is claimed.

The additional private M1 diagnostic is separately labeled and restored after checking; it does not close M1 in frozen0aa. No implementation changes were made by this reviewer.

## Qualification limits and remaining gates

Author-reported canonical webhook169, full Auth282, seven-package all-target Clippy -Dwarnings, fmt, architecture, six author mutations and docs/renders are supporting evidence; the independent executions above are specifically identified rather than relabeled as canonical builds. This review did not independently rerun shared Cargo checks, canonical browser suites, stress/chaos, platform feature matrices, MSRV, Windows/macOS or exact-head CI. These remain gates for parent coordination. The native Windows c5 expected peer_unreachable/actual peer_audit_unavailable failure remains unqualified; cancelled matrix jobs are not additional assertion failures.

Gateway behavior still requires the repository's canonical scripted Chromium/WebKit and applicable default/raised stress/chaos evidence, plus actual exact-head CI; an unmounted slice is not an automatic waiver. Parent owns denied foundation job retry120322813252 and manual workflow/browser holds. No denied action was retried, bypassed or delegated here; no reviewer tool action was denied or left uncertain. No live credentials, recipients, registration, network/security changes or production transmissions were used. No exactly-once remote effects are claimed.

Parent should close M1/N1/N2 together, update docs/evidence and obtain a fresh exact-head independent round TWO under the bounded process before beginning composition. No merge approval is given.
