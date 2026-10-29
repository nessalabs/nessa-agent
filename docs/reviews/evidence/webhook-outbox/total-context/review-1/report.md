# Independent review of total contextual restoration

Frozen clean checkout `/workspace/nessa-agent`, branch `codex/896-webhook-delivery`.
Reviewed head `5bb457dba2048f03de91fdc67162636b549437b2`, production source
`fb0f403648a2bb9d8dc6f43385c1e1e5371588f2`. Structural correction base
`1fccffdcdd5224c20914a308b644f750e06633af`; stacked PR1096 intended base remains
foundation `6c38915d5201de4cac54500ec4b5ad5e5f34229e` / PR1091.

Outcome: **changes required. One major and one minor; blocker/nit empty.**
This is not persistence, complete feature, browser or merge approval.

## Major: source-change consequences still attest their own original actor

Locations: `restoration.rs:129-145,192-194`, `stored.rs:154-155`,
`audit.rs:464-475`; source writes are `queue.rs:65-66` and
`journal.rs:221-235`.

The new cursor fact retains the original reset observation, and Cursor evidence
is correlated with it. Delivery evidence and saved Sending capture are not
correlated with that same source-reset owner. Credential-stop context is checked,
but source-stop context can be a different individually valid event. The Sending
last-audit-None exception permits the first copied source-stop value to become
history without proving its origin. SourceChanged refusal evidence with no
sourceStop snapshot validates only reader syntax and the refusal transition.

Three concrete copied-production-code demonstrations reproduce this one gap:

1. **Retained original reset disagrees with Sending capture.** Queue position1,
   admit Sending at2000, then perform an actual source replacement. Cursor
   reset_stop is Live/1002 and Sending has that capture. Change only the
   Sending `source_stop` to Restart/9999. Reopen succeeds. `recover(3000)`
   publishes terminal Unknown with sourceStop Restart/9999 while the cursor's
   retained original observation still says Live/1002. The preceding admission
   audit legitimately contains None, so history validation accepts the changed
   first capture.
2. **Owed immediate source refusal substitutes actor/time.** Keep position1
   Queued; temporarily remove the audit directory and queue a replacement
   source. Actual SQL commits the refusal, cursor reset fact and both audit
   records, then returns Unavailable. Restore the directory and change only
   the owed Delivery SourceChanged record's initiator to restart and atMs to9999.
   Reopen and due publish it, although the original reset fact and Cursor audit
   retain Live/1002. No Sending exception is needed for this variant.
3. **Capture without any source reset.** Queue/admit Sending, never replace the
   source, then add source_stop Restart/9999 to that saved delivery. Reopen
   succeeds with cursor reset_required=0; recovery publishes the invented stop.

Expected: reject contradictions/missing provenance as Invalid before publication
or cleanup, while retaining the original evidence. Actual: restore accepts the
records and later immutable audit records claim substituted or invented source
causes. These are persisted causal-evidence defects; no POST admission bypass
was demonstrated. They violate the parent-adopted cross-record correlation and
CODING_STANDARDS audit item6/known-original-cause requirements.

Exact probes: `add-probes.py`, `add-refusal-probe.py`,
`add-invented-capture-probe.py`. Final output `final-probes.txt` passes all three
bug demonstrations; those passes affirm the defect, not fixes.

## Minor: owed-only selection scans all published audit rows

Location: `restoration.rs:21`.

`WHERE (?1=0 OR published=0)` uses the same SQL shape for startup and live owed
selection. With owed=true, SQLite still scans the complete audit table instead
of searching the existing unpublished_audit index. Every `publish` calls this
selection, even when only one audit record is owed. Thus published retained
history increases per-event validation selection work, independently of the
improved owed-only delivery-history query.

The exact source SQL was measured through the actual bundled rusqlite artifacts:

| Published rows, one owed | Actual fullscan steps | Actual VM steps | Direct published=0 fullscan | Direct published=0 VM |
| --- | --- | --- | --- | --- |
| 0 | 0 | 18 | 0 | 16 |
| 100 | 100 | 418 | 0 | 17 |
| 10,000 | 10,000 | 40,018 | 0 | 17 |

Expected: startup scans all required history, while live owed selection uses its
published-state index and scales with owed rows. Actual: O(total retained audit
rows) work on each live publication. This is a selection-cost regression, not an
unbounded allocation or proved deadline failure. Split the startup and owed SQL
shapes at this owning selector; preserve complete startup validation and the
whole owed-batch preflight. Add deterministic operation-count evidence with a
fixed owed count and increasing published history.

Exact probe `add-selection-probe.py`, output `final-probes.txt`. This isolates
SQL selection on the real schema and dependency; its row contents are not a
complete domain/lifecycle fixture, and it does not claim to execute the full
adapter on fabricated histories. Its purpose is the exact query's work count.

## Structural alternatives for parent adjudication

The exhaustive variant match solves omitted variants, but ownership within a
Delivery's source-change causes still rests on copied values. Joining to only
the current cursor would fix the first examples and break legitimate later
cursor replacement/deletion. The original cause must survive its current view.

A. **One immutable SourceReset fact and durable references.** Mint an event
identity for one reset, retaining registration/chat, prior checkpoint/source,
observed source and original reader/time. Cursor, actually affected deliveries
and their audit evidence reference this same owner. Preserve the first reset
identity on an already captured Sending delivery. Retain it until every cursor,
delivery and owed audit reference is released through the common journal commit.
This covers immediate Queued/Retry refusal as well as Sending capture/recovery,
without adding a permission owner or POST phase. Reject missing/foreign reset
references before effects. Record cleanup observation time separately from the
original reset time; late retry refusal need not occur at the reset time.

Specify reference compatibility rather than requiring equality with the latest
cursor. Existing invalidation intentionally matches physical source across
access epochs; an old delivery's logical grant scope can differ from a newer
cursor's access epoch. Same registration/chat, physical source and checkpoint
relationship need an explicit contract. Test reset A, later cursor deletion or
replacement/reset B, then outcome/recovery of an old A Sending delivery.

Give facts count/byte bounds and shared cleanup an explicit owner. Referenced
facts cannot be collected merely because the Cursor audit was published. Bound
both cursor-owned and detached delivery/history-owned facts; current cursor
cardinality has no independent cap, so do not claim a whole-store bound by
counting pending deliveries alone. Refuse excess before allocation and before
committing partial reset/capture effects; preserve owed evidence on failure.

B. **Explicit typed SourceCaptured evidence/aggregate transition.** Retain the
original reset identity as above and record capture explicitly, so current
Sending capture and complete history agree without the last-audit-None exception.
This makes the observation order explicit but adds an evidence transition and
requires finite capture/history accounting. An additional snapshot by itself,
without a retained original reset owner, does not solve substituted history after
cursor deletion. Assess the increased state/audit cost before choosing it.

Both are structural choices for the parent, not authorization to start another
patch or reset the review bound. Candidate A preserves current dispatch phases
with less transition behavior. Do not infer provenance from timestamps, current
owners or physical-source equality alone; source identities may be seen again.
The minor selector correction needs no new lifecycle state.

## Independent verification and inspected boundaries

Own copied harness: `/tmp/independent-total-context-review`; compiled directly
with rustc against canonical dependency fingerprints. Only Target DTO is
substituted. Actual auth identifier types, domain Delivery, application Outbox,
private SQLite/filesystem adapters and existing tests are retained. No shared
source or Cargo artifacts were changed and no Cargo build ran.

- Baseline: **50 existing tests passed in56.99s**, `baseline.txt`.
- Combined run: **53 passed in58.49s**, `all-tests.txt`:50 existing, first two
  causal bug demonstrations and the selection-cost measurement.
- Final focused run: **4 passed in0.05s**, `final-probes.txt`: all three causal
  bug demonstrations plus the selection measurement. The invented-capture
  demonstration was added after the combined binary; there was no claimed54-case
  combined run. Passing bug demonstrations are adverse findings.
- `compile-final-probes.txt` and `args.json` preserve compilation context.
  All14 source-manifest hashes match the frozen source. Root remains clean at
  the exact reviewed head.

Read AGENTS, CODING_STANDARDS gates/local-review/dimensions/agreement/audit/
organization/DDD/one-current-contract, relevant architecture/module map and typed
DI guidance. Inspected every changed production file and adjacent full
queue/journal/stored/capacity/schema/publication/restoration/retirement/cursor/
registry code, audit variant/transition validation, application Outbox/causes,
domain Delivery, new test tables and relevant earlier restore/source/retention/
CAS/cancellation/lost-ack cases. Inspected the parent design tables, prior review
findings/probes, committed validation/mutation evidence and state page; viewed
the rendered diagram. Supplied80 canonical tests/required Clippy/108architecture/
fmt/docs checks were supporting evidence, not independently rerun.

Relationships and enforcers checked:
- Canonical SQL/audit/delivery/reg/chat identities, target and scope/epoch;
  original issuance and credential stop facts; no new Cedar or grant authority.
- Complete before/after transitions through existing DeliveryCause/Delivery,
  chain predecessor/current-end, retained original credential stop, legitimate
  Sending capture order and settled source context. Missing source-reset cause
  ownership remains the major above.
- Registration Issue/Stop/Drained versus live or secret-free retired facts;
  exact retirement audit identity, deferred FK, original actors after private
  registration deletion, malformed/missing/foreign retirement records and64 cap.
- Cursor whole decoding/reset observation, prior/observed source relationship,
  startup and live duplicate read, reset facts after published audit trim;
  the connection from those facts to affected delivery causes is missing.
- CAS/contiguous queue/stable identity, retained owner and worker permits after
  observer cancellation, batch validation before first audit-file effect, lost
  audit acknowledgement, exact-one-row ack and immutable file byte comparison.
- Shared trim/retirement/stop/source cleanup, late retry and current Sender
  observation, startup Unknown without POST, failed sink replay/retired actor
  positive controls and later-cursor-deletion source-stop controls.
- Bounds: pending/terminal1024 per registration, registrations/retirements64,
  workers4 retained through physical completion, bounded body/scope/audit/state
  representations, finite history with one predecessor, intentionally retained
  immutable audit files. Exact SQL selection work has the minor above.
- Organization: feature-owned infrastructure facts/restoration and same-feature
  tests/module map; no public SDK or shared auth/grant changes in this delta.
  Historical findings are preserved and current docs do not claim full #896.

Exclusions: no Windows/macOS rerun, no arbitrary production filesystem fault or
memory-corruption proof, no custom Outbox adapter/Cedar admission proof, no HTTP
recipient test in this storage harness. Product registration/share target,
credential revoke/first-expiry coordinator, authorized source, sender supervision,
capability/revoke E2E and complete stacked runtime remain uncomposed. Canonical
gateway Chromium/WebKit/stress/chaos gates remain applicable and unpassed; docs
rendering is not a substitute. This review does not waive any gate.

No source/docs edits, live keys/recipients/registrations, public listener,
production data, network/security changes, GitHub writes, merge, push or CI
operation occurred. The denied foundation job120322813252 and workflow/alternate
route restrictions were respected. No tool action was denied, pending or
uncertain during this review. Parent must adjudicate the remaining structural
choice before further correction/composition under the supplied bounded process.
