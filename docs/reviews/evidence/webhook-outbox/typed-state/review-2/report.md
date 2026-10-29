# Fresh independent typed-state review, round 2

Frozen clean checkout `/workspace/nessa-agent`, branch `codex/896-webhook-delivery`.
Full redesign base `bb51b40d2df849546918cd7006d55810e32c2ee6` -> head `dc86bc2d2cc4ee406b6bca8e73cdc721777177e5`; correction base `bb561e555501ec650d314af8898816615a960a55`; production source `26b2bd355f8d3d203b0db76b94f9f78a2da41ffe`. Outcome: changes required, not approval of persistence or issue #896.

Priorities: blocker none; major two; minor none; nit none.

## Major 1: registration audit attribution and target can contradict retained facts

Location: `crates/nessa-server/src/webhooks/infrastructure/outbox/publication.rs:119-127` (all-record validation), `audit.rs:369-406` (registration record validation). Only delivery histories receive enclosing-fact correlation at publication.rs:134 onward.

Reproduction uses real copied production code and private SQLite/filesystem fixtures:
1. Install synthetic registration and queue one delivery. Admit it to Sending at 2000.
2. Temporarily rename the audit directory, then call actual stop with Revoked(original-revoker), time 2001. It commits stop facts and registration-stop audit, then returns Unavailable. Sending retains the registration.
3. Restore the directory. Change only the owed registration-stop audit initiator to replacement-revoker.
4. Reopen succeeds; retained registration stop_initiator remains original-revoker. due(3000) publishes the changed registration-stop audit with replacement-revoker.

Expected: Invalid before publication/cleanup because original immutable stop cause/actor/time remain available. Actual: a syntactically valid replacement actor is published. The corrected delivery credentialStop correlation does not inspect this registration record. No fabricated delivery refusal is necessary.

Additional target variant: an installation's issue audit, marked owed after removing its synthetic file, can change principal and owner plus initiator to replacement values. Reopen and registration lookup publish that audit while returning the original registration target. This demonstrates missing target correlation too, not a request to infer historical actors from a new owner.

Exact probes: `add-probes.py`, cases `independent_registration_stop_actor_disagrees_with_retained_fact` and `independent_registration_issue_target_disagrees_with_retained_fact`; output `probes.txt`.

## Major 2: cursor transition evidence can contradict its retained checkpoint

Location: publication.rs:119-127 and audit.rs Cursor arm. Cursor records are self-validated, but no contextual relationship is checked against the retained cursor.

Reproduction:
1. Queue position 1 and admit its delivery to Sending.
2. Rename the audit directory and queue a different physical incarnation at position 2. Actual queue commits reset_required=1, cursor-change audit, and Sending source-stop capture, then returns Unavailable from publication.
3. Restore directory and change only owed cursor audit priorPosition from 1 to 999.
4. Reopen succeeds; retained cursor position remains 1. due(3000) publishes priorPosition 999.

Expected: Invalid before publication, since this owed source-change record describes the checkpoint retained by the same atomic transaction. Actual: individually valid integer 999 passes. The full cursor decoder validates the cursor itself; it does not validate its relationship to this saved transition.

Exact probe: `add-cursor-probe.py`, case `independent_cursor_audit_checkpoint_disagrees_with_retained_fact`; output `probes.txt`.

Both findings affect historical causal evidence, not a demonstrated POST authorization bypass. They violate the requested restored-state cross-field agreement and audit evidence contract.

## Structural diagnosis and bounded process

Typed representation removed open-ended JSON and now validates each record. Contextual validation still exists as a delivery-only second pass. Thus registration and cursor records can describe a different valid execution from their owning retained facts. The two findings have one structural source: contextual agreement is not a total responsibility across the closed audit variants.

Before any further implementation, define the owning restoration relationship for every variant: registration Issue/Stop versus retained registration target/stop; Cursor versus retained reset checkpoint; Delivery versus its chain/current row. Explicitly distinguish owed records that retain their atomic owning facts from valid historical retirement after those facts were deleted. A blanket requirement that all registration IDs still exist would break legitimate retirement recovery and is not a fix. Historical snapshots may describe earlier state; contextual matching needs its lifecycle/retention contract, not indiscriminate equality to a later current state.

Per supplied history, the original review already exhausted three rounds plus two structural rounds at bb51b40d; the parent then explicitly adopted this focused redesign. Fresh redesign round1 found two majors; this is fresh round2 and finds adjacent defects in the same persistence component. Repository instructions require revisiting the design with its owner after consecutive rounds in the same component. This report does not authorize another patch/review cycle or reset that bound. Parent owns the next structural decision. No reviewer changes were made.

## Independent verification and coverage

Copied harness `/tmp/independent-webhook-final-review`, rustc against canonical dependency artifacts, no Cargo/shared-target writes. Only Target DTO is substituted; actual auth identity types, Delivery, application Outbox contract, SQL adapter, private storage and all outbox tests run. Combined **45/45 passed in 44.26s**: 42 existing tests and 3 explicit bug demonstrations. The three passing new cases affirm bad behavior; they are not fixed regression evidence. `all-tests.txt`, `probes.txt`, `compile-probes.txt`, `args.json`, add-probes.py and add-cursor-probe.py retain reproduction details.

Positive controls include `retired_historical_actor_survives_lost_publication_ack_and_restart`, which deletes current registration facts before replay and preserves original-other-revoker distinct from owner; `sending_capture_can_precede_its_observation_audit_on_restore`; `settled_source_context_must_match_the_enclosing_delivery`; all credential-stop saved-context mutations; maximum-integer cursor/duplicate acceptance; retained history chain tests. These passed independently. All 13 hashes in correlation-source-manifest.json match frozen source. Root tree remained clean at exact reviewed head.

Inspected AGENTS.md, CODING_STANDARDS gates/local review/agreement/dimensions, architecture/module maps, outbox design ordering/redesign rows and state page. Inspected all changed production persistence source, adjacent full queue/journal/registry/mod/stored/capacity/schema, application DeliveryCause/Outbox and domain Delivery, all 42 outbox tests, first-round report/proofs, source manifests and supplied final check summaries. Canonical72 webhook tests/Clippy/architecture/fmt/docs were supplied evidence, not independently rerun against shared artifacts.

Relationships and enforcers checked:
- Identity/authority: canonical UUID, actual typed principal/credential/org; SQL owner/lock and cloned handles; saved target/scope/epoch/delivery correlation in codec, stored, publication. Registration evidence correlation missing as above. No new Cedar owner added.
- Domain/lifecycle: full before/after through DeliveryCause::allows and domain transitions; bounded attempts and stable timestamp; first/last/history continuity; terminal source equality and legitimate Sending capture exception. Actual immutable source-stop predecessor preservation checked.
- Concurrency/receipt: state CAS, contiguous cursor queue, lost ack no replacement identity, publication-before-next-work, shared worker semaphore and canonical owner retained after cancelled waiter. Runtime sender is absent, so no assembled dispatch claim.
- Failure/cleanup: transaction rollback, source and credential bulk refusal, exact publication acknowledgement, bounded immutable file reads, unavailable audit replay, retention/retirement and original stop context. Historical retirement without row accepted; findings concern available retained facts.
- Restoration/representation: closed serde variants/unknown fields, bounded audit input and saved state/scope, cursor full typed SQL decoding at live duplicate and startup, generated phase/due validation, corrupted DB classification, current schema and foreign-key checks. Individually valid non-delivery evidence still contradicts its owning state as demonstrated.
- Resource ownership: registrations64, pending/retained1024 each, workers4 and physical ownership after observer cancellation, finite delivery-history transitions and constant predecessor storage. Immutable audit-file history intentionally has no retention bound; cursor/runtime limits remain future composition work.
- Organization/docs: feature-owned files/tests and module map, source checkpoint26b; no public SDK change. Design covers typed restore and first round corrections but lacks the demonstrated non-delivery contextual relationships. No UI visual change here; diagram render is documentation evidence only.

Exclusions: no Windows/macOS rerun, no independent browser/stress/chaos or production filesystem fault proof; no complete Cedar admission/custom Outbox adapter proof; no HTTP recipient test in this storage harness. Product registration/configuration, credential revoke/expiry coordinator, dispatch permits/source selection/worker supervision and capability/revoke E2E are uncomposed. Canonical gateway Chromium/WebKit/stress/chaos remain applicable unpassed gates; source being unmounted does not waive them.

No source/docs edits, live recipients/keys/registrations, production listener/data, network/security changes, GitHub publication, merge or CI operation performed. Explicit foundation CI retry/workflow denials were respected. No tool action was denied, pending or uncertain during this review; review results are definitive local reproductions. Root checkout remains unchanged.
