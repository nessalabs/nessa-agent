# Fresh independent round THREE: general credential stop

Disposition: **clean local review round** for the bounded correction at the exact frozen head below. **Blocker: none. Major: none. Minor: none. Nit: none.** No open substantive finding requires the structural handoff rule. This completes an empty full third ordinary review round; it is not runtime mounting, merge approval, platform qualification or completion of issue #896.

Frozen checkout `/workspace/nessa-webhook-supervisor`; exact base `c7fe656af15346a21c293eb9a5bb5d5cf27c54b0`; exact head `ab5caa5bb28dd5a2c419f44f218999c8ec6ee910`; production/test source checkpoint `342e21aa5250eb97c79e57a1bd56bd10c12a439b`. Independently verified clean checkout, unchanged HEAD and empty crates diff from342 to head. All private source, builds, mutations, scripts and outputs are under `/tmp/nessa-general-stop-round3-independent-review`. No frozen-checkout edits, shared Cargo mutations, author/prior-reviewer artifact writes, implementation delegation or external mutations occurred. The separate prohibited older registration report was not retrieved or reconstructed.

## Findings and prior dispositions

All priority buckets are empty. Round TWO remains recorded as changes required for its Nit; this report does not relabel it.

- Prior Major M1 is fixed. `SavedRetirement::row` includes registration ID in its borrowed0..9 total byte budget before all owned reads. Direct ID lookup, indexed credential selection and restoration consume that single checked projection. Restoration no longer clones an outer String ID or rereads the retirement. Canonical UUID parsing remains the shape owner. Fresh tests exercise all six text identities, malformed UUIDs, valid maximum stop bytes, selectors and actual validator. Details and independent mutation evidence follow.
- Prior Nit N1 is fixed. Reviewed all eight round-TWO sites in registry, retirement, stop_rows and lifecycle, plus the full added type-reference/import scope. SavedStop is imported at each owning module, lifecycle uses its imported RegistrationStop, and the earlier new Auth types/imports are at module scope. No remaining new type-reference finding was identified; unrelated inherited qualified types are outside this correction.
- Prior Nit N2 is fixed. `ObserveRegistrationStop` says composition **must retain** its inner future and explicitly disclaims a command supervisor. Documentation consistently identifies the missing outer runtime owner.

## Scope and standards

Read the complete assignment brief; AGENTS and applicable CODING_STANDARDS gates, required brief, agreement, review dimensions, bounded rounds and evidence/closure; relevant codebase structure, architecture boundaries, Auth/webhook module maps and `/tmp/nessa-docs/skills/nessa-docs/SKILL.md` with checkpoint format. Read the two allowed unchanged general-stop reports for dispositions. Inspected the complete38-file base..head production/test/documentation change, not only final imports or the retirement repair, and adjacent current-bound Auth, journal, admission, publication, capacity and progress owners.

Production ownership: Auth revocation decoder/current binding; webhook lifecycle domain; registration-stop application; outbox audit, codec, intents, journal, publication, registry, restoration/progress, retirement, schema, stop and store. Tests: Auth revocation; webhook lifecycle; registration installation fixtures; outbox base, issuance/lifecycle context, registration/origin, expiry, stop, retirement rows and source reset. Documentation: codebase map, webhook implementation design, state page/checkpoint and allowed general-stop evidence. Source/test fixtures remain under the owning feature/layer; no extra mutable authority or duplicate history owner appears.

Applied authority/identity, domain invariants, admission/concurrency, failure/cleanup, observations/restoration, audit, representation, resources and public surface/organization dimensions where relevant. No SDK provider/ACP/MCP/UI implementation review is claimed. Gate15 order tables cover explicit revoke, offline supersession, expiry-first, distinct clocks, contradictory input, acknowledgement loss and recovery. Gate16 remains a small immutable-evidence seam. Gateway browser and stress gates remain required and unqualified below.

## Agreement across related facts and layers

| Related facts | Owner/enforcement and evidence |
| --- | --- |
| Retained shape versus current binding/authority | RevocationRecord reuses Auth journal/domain shape decoding and does not prove provenance/current permission. CredentialRevocation::restore/matches checks current ID, issuance, expiry, revoked metadata and revision; AccessSnapshot binds origin ordering. Actual Auth8 tests pass; removing current binding fails2. No retained decoder is consumed as sender authority. |
| Original journal identity, actor/cause and clocks | RegistrationStop stores the actual original record; lifecycle derives Initiator/RevocationCause; SavedStop requires exact projection agreement. Actual explicit, owner recovery and surface provisioning tests preserve replacement, correlation, sequence, revision and original command/effective seconds. Command19s/effective20s/observation40123ms remains valid. LocalOperator remains distinct from principal named local_operator. |
| Registration credential/issuance versus owed publication | Registry preflight executes under the existing physical owner lock before finish/publication. Independent nine-family shape-valid input substitutions and existing different-credential pending-initial test refuse Invalid before the hidden sink can be touched. No grant/policy check is duplicated here. |
| Local first observation versus later retries | Local issuance10999ms/Auth issued10s, original revoke101s, first observation9999ms are accepted without fabricated chronology. Retirement acknowledgement loss and cold replay at200111ms retain original9999ms and unchanged Auth101s. Mutation using Auth seconds*1000 fails this control. |
| Live, retired and audit correlation | One SavedStop codec covers registrations, retirement and audit. Contextual restoration compares complete record, original target and exact retirement audit ID/time. Existing substituted stored journal-ID case rejects before effects. Independent input substitutions preserve prior retirement and audit evidence; same original fact with later clock succeeds unchanged. |
| First expiry versus later independent revoke | Domain retains the original expiry reader/time; expiry-observation acknowledgement precedes final stop. Actual later Auth revocation remains independently intact. Existing owner/restart/delivery, partial-stop failure, sending result/recovery and cold reopen cases pass. |
| Actual held access versus sink acknowledgement | ObserveRegistrationStop keeps the actual AccessSnapshot hold through the awaited sink. Existing real Auth mutation remains pending until acknowledgement; dropping the hold early fails. Missing/mismatched revocation fact refuses before the sink and cannot fall back to elapsed expiry. Future-issued credential returns NoStopObserved, which grants no send permission. |
| Row bytes, identities and query bounds | Shared borrowed row budgets cover all retained identity text and nested stop; joined live offsets0/11, audit rows and maximum valid16KiB padded stop controls pass. Retirement query uses credential index/LIMIT2, refuses ambiguity, and actual query FullscanStep0/VmStep<32 is stable at1 and64 rows. Restoration retains64+1 sentinel. Fresh allocation tests and ID omission mutant detailed below. |
| Retained proof versus fully erased history | Existing W24 eight-table owner remains registrations, registration_intents, cursors, deliveries, audit, retirements, source_reset_facts and source_reset_members. Progress only interprets retained evidence; trimming does not create permanent acknowledgement proof. Existing inventory/context/progress tests pass. No stronger post-trim guarantee or new owner/table was introduced. |

Invalid retained/supplied evidence preserves authoritative state and causal attribution. Publication failure, acknowledgement loss and external delivery are distinct. Current credential, grant and Cedar admission still belong to the future dispatch boundary; the existing AdmitDelivery owner retains those obligations. No bearer/session/policy/application-installation #1052 expansion was found.

## Independently executed checks

Compiled copied actual Auth crate source and copied actual webhook application/domain/outbox owners/tests against read-only dependency rlibs in `/tmp/nessa-retention-dependency-snapshot`. Assembly was independently placed in this review directory after reading, not executing, the author helper. The structural rustc shell substitutes only the unrelated application Target declaration with its identical four-field shape; actual registration application, stop application, Auth source and persistence owners are compiled. It omits canonical server composition and the seven-package Cargo graph. These are private actual-source checks, not canonical Cargo or platform/feature-unification evidence. Exact arguments are retained in `auth-args.json`, `owner-args.json`, Auth test argument files, and build scripts.

Linux, rustc1.99.1, serial test execution:

- Original copied actual-owner/domain baseline: **140 passed** (`baseline.log`).
- Copied actual Auth unit suite: **214 passed** (`auth-unit.log`).
- Copied actual Auth revocation integration: **8 passed**, and restored **8 passed** after current-binding mutation (`auth-integration.log`, `auth-integration-restored.log`).
- Five new independent reviewer tests plus baseline: **145 passed**, zero failures (`restored145.log`).
- Final targeted restored controls following the additional ID-allocation reproduction: **5 passed** (`last-restored-controls.log`).
- Independent Nessa Docs catalog: **135 documents, zero issues** (`docs-validation.json`). No independent diagram rendering or visual/interaction verification was performed. Author's3 rendered and inspected diagrams remain supporting evidence attributed to the author.

Fresh reviewer controls:

1. `round3_shared_retirement_projection_refuses_before_large_allocations`:1MiB multibyte text in id, credential, principal, organization, owner and audit; borrowed row rejects RowTooLarge before owned allocation (largest measured Rust allocation0 for each). Malformed, simple/unhyphenated and uppercase UUID controls refuse Invalid after bounded row decode.
2. `round3_actual_retirement_validator_and_selectors_bound_identities`:actual schema and retirement validator, each of those six1MiB identities; all return typed Invalid and largest measured Rust allocation196bytes. Direct read and indexed credential selector also refuse where their key can select the malformed row. Setup precedes measurement; SQLite C/page/storage allocations are not measured.
3. `round3_nested_duplicate_and_namespace_controls`:direct text duplication of equal top-level cause/initiator/atMs/revocation and nested sequence/revision/after/at/credentialId keys, avoiding map collapse; all refuse Invalid. Canonical actual revocation accepts. A shape-valid principal named local_operator accepts only its principal-prefixed projection and refuses substitution of offline local_operator.
4. `round3_fractional_issuance_clock_rollback_and_cold_replay`:actual Auth issuance/revoke, local fractional issuance, backward observation clock, lost retirement acknowledgement, cold reopen and later retry preserve first observation and unchanged original command time.
5. `round3_every_original_fact_family_precedes_publication`:after actual retirement acknowledgement loss, shape-valid substitutions in actor, supersession cause/replacement, command/effective time, credential, issuance, expiry, sequence, revision and absent correlation all refuse Invalid before hidden audit-directory publication. Original retirement/audit evidence remains unchanged; original record retries successfully after cold reopen.

First allocation fixture accidentally supplied9 parameters to a query with8 slots. That reviewer setup error is retained in `controls.log`; corrected query passes. It is not an implementation finding. One unconfigured rustc version read attempted the default read-only rustup home; setting the already-provided private toolchain environment resolved it without escalation.

## Independent load-bearing mutations

Each mutation changed exactly one owning rule in private copied source, asserted that the edit landed, compiled, required a failing meaningful test, and restored original bytes with a fresh write. `mutations.py`, `mutation-results.json`, per-rule `.diff` and compile/test logs preserve evidence. No author/reviewer shared source or rlib was mutated.

| Rule removed/changed | Caught by |
| --- | --- |
| Exclude ID from shared retirement borrowed budget0..9 ->1..9 | Shared-row allocation/refusal regression; separate actual-validator reproduction below |
| Ignore SavedStop actor projection equality | Independent nested/namespace control |
| Replace local observation with Auth command seconds*1000 | Independent fractional/rollback/cold replay control |
| Ignore different original retained record | Independent nine-family original-fact/prepublication control |
| Omit run_checked prepublication invocation | Existing different actual credential/pending publication test |
| Drop actual held AccessSnapshot before sink await | Existing actual Auth hold/stop acknowledgement test |
| Remove current-Credential binding from CredentialRevocation::restore | Actual Auth integration fails2 assertions; restored8 pass |

The additional actual-validator ID-budget omission probe preserves the same typed Invalid while exposing the1,048,576-byte Rust allocation, compared with196bytes restored; see `id-allocation-recheck.log`. This is a diagnostic mutation of current source, not a rerun of a historical checkout. It confirms M1's bounded-allocation correction in the actual validator, independently of the committed RowTooLarge regression.

Production restoration comparison covers all119 Auth/webhook production files byte-for-byte with zero mismatches (`production-restoration.json`). Only reviewer tests/shell remain additional private files. Frozen checkout remains clean at the assigned head. No implementation change is requested by this review.

## Qualification limits and remaining work

Author-reported current-source canonical server webhook170, seven-package all-target Clippy -Dwarnings, fmt, architecture, docs135/0 and3 diagram renders are separately attributed supporting evidence. Auth source remains byte-identical to preceding full282 Auth tests. This review does not relabel private checks as those canonical runs. First author canonical compile SIGKILL before tests/cgroup OOM1 without baseline remains distinct from successful serial/MALLOC_ARENA_MAX2 retry and subsequent no-extra-OOM result. No assertion/timeout weakening is inferred or requested.

Windows native enrollment remains unresolved: expected peer_unreachable, actual peer_audit_unavailable. Cancelled matrix jobs are not assertion failures; fail-fast:false does not establish who cancelled them. Windows/macOS/MSRV, exact-head CI, canonical scripted Chromium/WebKit and default/raised stress/chaos remain parent-owned qualification gates. An unmounted correction does not waive gateway browser/stress requirements.

Pinned browser download DomainForbidden, foundation retry job120322813252 and manual workflow lookup/dispatch denials remain stopped. This review did not retry downloads or CI, seek alternate CLI/triggers/no-op workflows, use live credentials/registrations/recipients, open a production listener or alter network/security configuration. No review action was denied, pending or uncertain; all requested private execution completed.

Runtime command supervisor/startup reconciliation/configuration/product/protocol/share mount, authorized source reader, supervised sender and recipient/capability E2E remain incomplete. Current credential/grant/Cedar holds remain required for future admission. A decoded retained record is immutable evidence, not current authority or an independent history owner. No exactly-once external-effect claim is made.

Parent owns subsequent publication, composition, CI and merge decisions. Clean round THREE closes this narrow correction's local review loop only.
