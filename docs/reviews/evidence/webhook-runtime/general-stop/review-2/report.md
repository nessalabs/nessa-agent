# Fresh independent round TWO — general credential stop

Disposition: changes required for one remaining Nit N1. Blocker: none. Major: none. Minor: none. Nit: N1 below. This is not a clean round under the repository stop rule and is not merge or runtime-composition qualification.

Frozen checkout `/workspace/nessa-webhook-supervisor`; exact base `c7fe656af15346a21c293eb9a5bb5d5cf27c54b0`, exact head `ef06f84f0f355b631e29a8da91d53f688499967f`, production/test source `d742cf55ef740716a7a8dfebe5eaea427819138c`. Verified crates diff source..head empty and frozen worktree clean. All reviewer builds, mutations, fixtures and outputs live under `/tmp/nessa-general-stop-round2-independent-review`. No frozen checkout or author/prior reviewer/shared Cargo artifact edits. No implementation delegation, external mutations or access to the separate prohibited historical registration report.

## Findings and prior-round dispositions

### N1 — Nit: new type references still bypass module imports

The CODING_STANDARDS Rust-import rule requires importing types at the owning module top and using short names in signatures and expressions. New exact-head sites remain:

- `crates/nessa-server/tests/webhooks/infrastructure/outbox/lifecycle.rs:140`: helper result uses `crate::webhooks::application::RegistrationStop`. The type is already available through the module imports.
- `crates/nessa-server/src/webhooks/infrastructure/outbox/registry.rs:188,273`: qualified `super::stop::SavedStop` expressions.
- `crates/nessa-server/src/webhooks/infrastructure/outbox/retirement.rs:29,73`: same qualified type expressions.
- `crates/nessa-server/tests/webhooks/infrastructure/outbox/stop_rows.rs:13,94,141`: qualified `super::super::stop::SavedStop` expressions.

Trigger is reading/building the new helper/codec paths; expected short imported type names, actual qualified references. This is an organization/style contract failure with no demonstrated behavior defect. Import SavedStop at each owning module and use the already imported RegistrationStop short name. These eight sites are in the complete base..head additions; unrelated inherited helper-local imports are excluded. Added-line inventory found no other new qualified type signatures/expressions or helper-local Auth imports requiring this disposition. Module functions, constants and macro qualification are allowed and were not reported. Prior N1's other Auth/domain short names and helper-local imports were corrected, but its closure remains incomplete.

### M1 — prior Major fixed at the frozen head

`SavedRetirement::row` now includes id in columns0..9, applies the total borrowed byte budget before any owned row.get, and then checks canonical UUID/domain shape. `read`, `by_credential`, and `validate` share this projection and checked result. Validation removes the old separate String id read and repeated retirement read. Credential selection remains indexed with LIMIT2 and rejects a second match; restoration retains MAX_REGISTRATIONS+1 sentinel.

Fresh reviewer probe inserted 1MiB text into each of id, audit, credential, principal, organization and owner before measurement. Actual validator returned typed Invalid for all six, with largest measured Rust allocation196 bytes. Direct read (where its UUID key can select the row) and by_credential (where its credential key can select the row) also refused. Invalid, unhyphenated, and nonhex UUID controls refused. Existing maximum valid padded stop row and nearby canonical retirement decode pass.

Deliberately excluding id from the shared borrowed budget (0..9 ->1..9) changes the same id input to largest allocation1048576 bytes with the same typed Invalid, and fails the reviewer allocation assertion. Restored source returns to196 and passes. This is an independently executed before/after mutation of the reviewed projection; the historical old outer String code was inspected in the preserved round1 report/diff, not rerun as a historical canonical checkout. Measurement covers Rust allocations during validation, excluding setup and SQLite's C/page/storage allocations. Evidence: `reviewer-controls.log`, `id-budget.log`, and final restored suite.

### N2 — prior Nit fixed

`application/registration_stop.rs:118-120` now states composition **must retain** the future and explicitly says it is not a command supervisor. No actual outer supervisor or mount is implied.

## Review scope and agreement

Read the brief, AGENTS, applicable CODING_STANDARDS review dimensions, bounded-round rules, agreement and audit/resource/import/policy/browser/stress gates; reviewed relevant architecture boundaries, Auth and webhook module maps and local `/tmp/nessa-docs/skills/nessa-docs/SKILL.md`. Inspected the entire 37-file base..head production/test/documentation diff, not merely the retirement repair, plus adjacent current-binding, actual Auth journal/snapshot, delivery admission, publication, progress and stored-history owners. Read the preserved general-stop round1 report solely for the stated dispositions.

Production ownership inspected: Auth revocation and journal identity; webhook lifecycle domain; registration-stop application; outbox audit, codec, intents, journal, publication, registry, restoration/progress, retirement, schema, stop and store. Tests inspected include Auth revocation, webhook lifecycle, installation fixture, outbox lifecycle/context/origin/issuance/registration/reset/row/stop tests. Documentation inspected includes codebase map, implementation design, state page and general-stop evidence/round1 report. No unrelated SDK/ACP/MCP/UI implementation review is claimed.

| Related facts checked together | Owner, enforcement and exercised evidence |
| --- | --- |
| Retained shape vs current-bound credential | RevocationRecord reuses journal/domain decoder; CredentialRevocation::restore/matches binds id, issuance, expiry, revocation and revision; AccessSnapshot additionally binds origin ordering. Auth integration8 and binding mutation distinguish these. Shape decoding grants neither origin proof nor authority. |
| Original actor/cause/journal id vs local observation | RegistrationStop retains actual Auth record, derives immutable projection, SavedStop roundtrip equality rejects inconsistent projection. Actual explicit/supersession tests preserve correlation, sequence, revision, replacement and original command/effective times. LocalOperator remains distinct from principal named local_operator. |
| Command19s vs effective20s vs observation40123ms | Actual surface supersession owner produces this valid ordering in the executed suite. No reconstructed Credential or clamping conflates these clocks. |
| Credential/issuance ownership vs pending publication | Registry run_checked validates supplied fact before finish can publish owed audit. Existing tests hide the audit directory and substitute credential or shape-valid sequence/revision/correlation; typed Invalid occurs without recreating/publishing the directory. The original fact with later observation retries unchanged. |
| Live/retired/audit identity and first observation | Shared SavedStop; registry lifecycle mapping, retirement canonical ids and restoration contextual equality validate exact original record, target and retirement audit. Ack-loss/cold replay tests and reviewer rollback test preserve first observation. |
| Prior expiry vs later independent revoke | Domain retains actual first expiry reader; lifecycle tests traverse observation ack loss, stop, restart and later actual Auth revocation. Original Auth event remains independently retained; registration cleanup does not fabricate it. |
| Held access vs stop acknowledgement | Actual AccessSnapshot remains owned through sink await. Actual Auth mutation stays pending until sink notification. Early-drop mutation fails this test. Missing revocation facts and mismatched snapshots refuse before sink, without expiry fallback; not-yet-valid credential gives NoStopObserved. |
| Capacity, row budget and query work | Existing count/body/audit limits, joined offsets0/11 and maximum stop controls; private suite runs indexed query FullscanStep0/VmStep<32 with equal missing-query work at1 and MAX_REGISTRATIONS, plus ambiguity refusal. Reviewer allocation probe and id exclusion mutation close M1. |
| Retained progress vs erased history | Existing W24 progress owner still combines registrations, registration_intents, cursors, deliveries, audit, retirements, source_reset_facts and source_reset_members. Fully trimmed history supplies no permanent acknowledgement proof. No independent history owner/table or stronger post-trim guarantee added. |

Authority/domain/admission/failure/observations/audit/representation/resource/public-surface dimensions were applied where relevant. The correction adds immutable evidence and an inner observation use case, not a policy bypass or mutable sending authority. Auth owns actual current facts, domain owns stop transitions, one codec owns saved stop mapping, and existing physical/publication owner serializes correlation and audit effects. Failure and acknowledgement remain separate from remote delivery. Gate15 order tables cover explicit revoke, supersession, expiry-first, ack-loss and contradictory supplied records; gate16 scope remains narrow. Gateway browser/stress requirements remain applicable and unqualified below.

## Independent execution and falsification

Used copied actual Auth crate and copied webhook owner/application/domain/test source. A private structural rustc shell exposes actual modules and substitutes only the unrelated application Target declaration with its identical field shape. The real registration application, stop application, Auth implementation and persistence owners are compiled; this is not canonical server composition or the seven-package Cargo graph. Immutable dependency rlibs are read from `/tmp/nessa-retention-dependency-snapshot`; all new outputs are private. Assembly/build scripts and exact argument JSON are retained here. Author helper was read for assembly, not executed. Snapshot feature unification/platform behavior is not established by this shell.

Executed results:

- Actual-owner baseline:140 passed (`baseline140.log`).
- Three new independent reviewer controls:3 passed (`reviewer-controls.log`).
- Final restored actual-owner suite including new tests: 143 passed (`restored143.log`).
- Actual Auth revocation integration:8 passed, passed again after binding restoration (`auth-integration.log`).
- Actual Auth unit suite: 214 passed (`auth-unit.log`).
- Independent nessa-docs catalog validation:135 documents, zero issues (`docs-validation.json`). No independent diagram render or visual/interaction check; author's three inspected diagram renders remain separately attributed supporting evidence.

Reviewer controls:

1. `review2_retirement_allocation_and_projection_controls`: actual schema/validator with oversized identity fields, read/credential selection controls, malformed UUID variants and allocation measurement above.
2. `review2_nested_codec_and_actor_boundaries`: duplicate equal keys for top-level cause/initiator/atMs/revocation and nested sequence/revision/at/after/credentialId are built directly as text, avoiding map collapse; all reject Invalid. Canonical stop accepts. Shape-valid principal named local_operator accepts only principal:local_operator projection and refuses offline local_operator substitution.
3. `review2_fractional_issuance_rollback_and_first_observation`: actual installed issuance observed10999ms/Auth issued10s, actual revoke101s, stop observation9999ms; retirement ack-loss then cold reopen and replay at201111ms preserves original9999ms and Auth101s.

Initial namespace fixture used principalId instead of actual DTO key id and failed fixture decoding; corrected to id and passed. `fixture-error.log` preserves that reviewer mistake; it is not a production defect.

Production restoration independently compared all119 Auth/webhook production files byte-for-byte to frozen head, with zero mismatches (`production-restoration.txt`). Reviewer test additions alone remain private. Final verification used Linux rustc1.99.1 and serial test execution.

Seven independent load-bearing rule mutations were applied one at a time, asserted changed, compiled, failed the relevant test and restored with fresh writes:

| Mutation | Failing evidence |
| --- | --- |
| Exclude retirement id from borrowed budget | `id-budget.log`:1048576 allocation, allocation assertion fails |
| Remove saved actor projection equality | `actor-projection.log`: namespace/codec refusal control fails |
| Replace local observation with Auth seconds*1000 | `local-clock.log`: fractional/rollback first observation control fails |
| Remove original record mismatch refusal | `original-record.log`: substituted journal fact reaches wrong behavior |
| Omit run_checked prepublication check | `prepublication.log`: different credential refuses too late/incorrectly |
| Drop actual held snapshot before sink await | `held-snapshot.log`: Auth revoke becomes ready before sink acknowledgement |
| Remove CredentialRevocation current binding | `current-binding-mutation.log`: Auth integration refusal assertions fail; restored8 pass |

A narrower initial probe deleting only matches_credential survived (`credential-prepublication.log`), because live matches_registration independently includes credential equality and still rejects the test's substituted credential. This is not proof the full prepublication rule is unused; deleting the actual run_checked invocation is caught as above. No claim that every syntactic guard has an independent regression is made. The first mutation driver stopped at this surviving probe and was then corrected to target the actual rule; all source was restored in finally blocks.

## Qualification limits and outstanding work

Author-reported canonical Linux server webhook170, seven-package all-target Clippy -Dwarnings, fmt/architecture and prior full Auth282 are supporting evidence, not relabeled independent canonical runs. Auth source is unchanged from that prior full run. First author canonical compile SIGKILL before tests and cgroup OOM1 without pre-baseline remain distinct from successful serial/MALLOC_ARENA_MAX2 retry. No assertion/timeout weakening is claimed. No independent full Cargo/package/MSRV/platform matrix was run here.

Windows native peer enrollment remains unresolved: expected peer_unreachable, actual peer_audit_unavailable. Cancelled matrix jobs are not additional assertion failures; seven new revocation Windows tests passing does not fix the native failure. Windows/macOS, exact-head CI, canonical scripted Chromium/WebKit, default and raised stress/chaos remain parent-owned merge gates. Browser download DomainForbidden, foundation job120322813252 retry denial, and manual workflow lookup/dispatch denials remain binding; this reviewer did not retry, seek alternate triggers/downloads, dispatch CLI/no-op workflows or use gh. No review action was denied or pending; one unconfigured rustc --version read could not create default rustup home and was not escalated.

Runtime mounting, credential command supervisor, startup reconciliation, config/product/protocol/share mount, authorized source reader, supervised sender and recipient/capability E2E are incomplete. Current credential/grant/Cedar holds remain required at future dispatch. No live credentials, registrations, recipients, production listener, network/security change or transmission used. No exactly-once external effect claim.

Parent should fix the eight N1 short-name references and obtain the next fresh exact-head review under the bounded process. This round has no remaining behavioral finding, but its Nit prevents a clean-round declaration. Parent owns edits, publication, CI and merges.
