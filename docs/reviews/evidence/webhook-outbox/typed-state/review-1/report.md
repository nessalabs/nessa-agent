# Independent adversarial review: typed webhook persisted state

Reviewed clean/frozen `/workspace/nessa-agent`, branch `codex/896-webhook-delivery`.
Exact base `bb51b40d2df849546918cd7006d55810e32c2ee6` → head `bb561e555501ec650d314af8898816615a960a55`; source `1c3085add14833a862681f6697f1e4ea31f9ba86` (later commit docs only).
This is the first fresh round for the parent-directed structural redesign. Outcome: changes required; not approval of #896 or merge.

Priorities: blocker none; **major two**; minor none; nit none.

## Major 1: terminal history can discard source-stop context and still publish

Location: `crates/nessa-server/src/webhooks/infrastructure/outbox/publication.rs:164-166` (latest enclosing-state comparison); related `audit.rs:120-132` (one-way predecessor preservation).

Reproduction uses actual copied source and real private SQLite/filesystem fixtures:
1. Queue a delivery, persist Sending admission at 2000.
2. Replace its source via the real queue entry point, yielding ResetRequired and saving source_stop on the Sending row.
3. Observe Success at 2100 through transition; a terminal response audit now contains the saved sourceStop.
4. Change only this terminal response audit's sourceStop to null; mark it owed and remove its synthetic published file.
5. Reopen the store and call due(3000).

Expected: Invalid at open, before publication or cleanup, because terminal current state and its latest full audit disagree on known source-stop context.
Actual: open succeeds; due publishes sourceStop:null as immutable audit. The retained terminal delivery still has source_stop Some. Its previous queued/admission audits legitimately contain None, so predecessor validation also passes.

The latest comparison only validates Some in the audit, permitting None against Some in current state. The intentional Sending-only capture-before-phase-audit window cannot justify allowing that mismatch after a terminal transition has committed. This violates the redesign's chain-to-current/source-context contract and CODING_STANDARDS audit gate item 6. No POST/authorization bypass demonstrated; impact is persisted causal evidence loss.

Exact body: `source-probe.py`; named case `independent_terminal_audit_loses_source_stop`. Output: `independent-probes.txt`.

## Major 2: restored credential-stop audit can replace the known original actor

Location: `crates/nessa-server/src/webhooks/infrastructure/outbox/audit.rs:413-416`; correlation is absent from `publication.rs:134-137,158-166`.

Reproduction:
1. Queue two deliveries; admit one to Sending, leaving the second Queued.
2. Temporarily rename the private audit directory, then stop via the real stop operation with Revoked(original-revoker), time 2001. SQL commits stopped registration + refusal audit; unavailable projection returns Unavailable. Sending keeps the original stopped registration present.
3. Restore the audit directory. Change only the owed refused delivery audit's initiator to another valid principal, unrelated-synthetic-revoker.
4. Reopen; inspect the owning registration: stop.initiator is still original-revoker. Call due(3000).

Expected: Invalid before projection, because retained delivery refusal attribution contradicts the owning immutable stopped registration.
Actual: reopen succeeds and due publishes the substituted unrelated-synthetic-revoker. Before/after snapshots, target, cause, checkpoint and original registration stop fact remain unchanged.

The validator checks that actor/time can syntactically construct some Stopped value, then reduces both credential stop causes to the broad AuthorityStopped refusal transition. DeliveryEvidence drops cause/actor entirely, and restoration reads only the registration target. A different valid actor therefore survives even though the actual original stop evidence is available. This violates cross-record actor correlation and original initiator preservation, not an authorization policy rule.

Additional concrete variant: relabel an ordinary active-registration AdmissionRefused audit to credential_revoked with a valid unrelated actor; reopen and publication also succeed while registration remains active. Same missing causal correlation, not a third finding.

Exact body: `actor-probe.py`; named case `independent_known_stop_actor_is_replaced`. Additional variant: `add-probes.py`, `independent_false_credential_stop_is_published`. Output: `independent-probes.txt`.

## Inspection and independent evidence

Read AGENTS.md and CODING_STANDARDS local review gate, agreement, review dimensions, audit, organization, DDD and seam contracts; architecture/module maps and delivery ordering design. Read all changed source files: audit, codec, cursor, journal, mod, publication, queue, registry, stored, and the changed outbox test additions plus adjacent existing cases. Read adjacent unchanged application Outbox/DeliveryCause, domain Delivery, schema and capacity, domain delivery tests, local-database OpenError/open classification, and prior blocked probes. Checked changed design/state/module-map/evidence docs, evidence JSON/text outputs, source manifest, and visually inspected saved diagram PNG. All 12 source-manifest hashes match the frozen source.

Independent harness: copied source under `/tmp/independent-typed-state-review`, compiled directly with rustc and the canonical dependency fingerprints. Substitutes only the Target DTO, retaining actual identity/domain/persistence implementations. Shared checkout and Cargo/target artifacts untouched. Baseline **39/39 passed in 46.44s**. Three additional bug-demonstration cases **3/3 passed in 0.01s**, affirming the bad behavior rather than fixed regressions. Combined rerun: **42/42 passed in 45.36s**, logged in all-tests.txt (39 existing tests plus three bug demonstrations).

Dimensions and boundaries checked:
- Authority/identity: canonical UUID/text correlation, immutable target/checkpoint across history, registration actor ownership, cloned SqlOutbox sharing one Owner/Mutex, lifetime file lock, failed-open release and worker retention.
- Domain invariants: complete saved state decoding; queued/admit/retry/refuse/response/restart transitions through existing DeliveryCause + Delivery owner; first/last chain continuity and legal terminal/attempt bounds. Two causal correlation gaps above remain.
- Admission/concurrency: queue cursor CAS and duplicates, transition CAS/lost ack, one blocking worker capacity owner, dropped waiters retaining physical owner/permit; source replacement while Sending and late response/restart paths. No new runtime scheduler introduced by this diff.
- Failure/cleanup: rollback before commit, immutable publication byte comparison and exact acknowledgement, unavailable sink replay, stop/source cleanup and retirement, historical retirement with deleted registration, invalid startup precedes publication/trim.
- Observations/restoration: startup validates all retained rows, full cursor decoder used live and startup including terminal-only duplicates; malformed/contradictory history and snapshots; immutable source/target matching; observed Sent vs restart Unknown preserved.
- Audit: all three closed variants (registration Issue/Stop/Retire; delivery queue/admission/response/deadline/refusal/restart/source/authority stop; cursor reset), actor labels/cause mapping, unknown field rejection and optional source facts; the two concrete failures above.
- Representation: bounded JSON parse inputs, unknown/duplicate struct field handling via serde, canonical delivery/UUID representations, typed SQL cursor columns/reset flag, complete scopes, known-corrupt DB versus I/O error mapping, private publication paths.
- Resource bounds: per-record bytes, 512-byte delivery state, 4096-byte scope, pending/terminal/registration caps, finite delivery transition chain, streaming constant predecessor memory, bounded publisher reads and physical workers; intentionally retained audit-file history is unbounded by design.
- Public surface/organization: infrastructure owns codec/storage mapping and calls existing inner transition owner; no new exported SDK surface or shared auth/grant implementation; module map, test placement and sourceRevision updated. Existing implementation in mod.rs predates this diff; no drive-by restructuring requested.
- Gates 15/16/17/19: design/ordering rows exist for redesign; current table/test coverage misses the two adversarial rows above. No new product UI in diff. Existing browser/stress integration gates remain explicitly unpassed; docs image rendering is not gateway/browser acceptance.

Canonical Cargo tests/Clippy/architecture/fmt/docs checks supplied by parent were inspected as supporting evidence, not independently rerun against shared target. No Windows/macOS, custom Outbox adapter, Cedar admission proof, recipient HTTP, production filesystem faults or complete product flow tested by this isolated harness.

Exclusions honored: registration issuance/authorization/configuration; credential revoke/first-expiry coordinator; registration dispatch permit; authorized source/replay; sender/supervision/live reconciliation; principal share target/owner reset; feature capability/revoke recipient E2E. No shared auth/grant changes. No live keys/recipients/registration, public listener, network/security/production mutation, GitHub/CI lookup or trigger, merge, push or force-push. Explicitly denied foundation job 120322813252 and equivalents were not called.

No tooling blocker or denied/pending/uncertain operation occurred in this review. The two reproducible major defects remain open. Shared checkout remains unchanged at the reviewed head.
