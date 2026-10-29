# Independent W24 retained-context review, round 2

Reviewed frozen clean candidate **274c337d7c8027401c20cacaa49047ad90150852** in `/workspace/nessa-webhook-composition`, branch `codex/896-webhook-composition`, against published base **6a107073f9261dcfcaedb37b9c8c9fe773c1d470**. Repair source is **f75b954e6f324a94150cd63dd95f1cd89e1ea646**. Reviewed the complete new W24 source against that published base, including the lifecycle domain owner, persistence mappings, schema, restoration, queue admission, acknowledgement/cleanup ordering, changed tests and documentation; not just the M1 repair hunks.

**Blocker 0; major 1; minor 0; nit 0. Original R1 M1: fixed with independently reproduced rejection/recovery evidence.** One adjacent prerequisite-acknowledgement relationship remains unvalidated. This is a pre-existing Issue-arm limitation exposed against W24's new ExpiryObserved phase, not evidence that the three repaired M1 cases still fail. Parent must explicitly adjudicate repair versus scoped deferral; the candidate cannot claim complete provable phase/acknowledgement correlation while this case remains.

## M2 — owed issuance is accepted after the registration already observed expiry

**Severity: Major.** Location: `crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs:85` (CredentialIssue arm checks target/issuance time, but not the owed prerequisite against the current lifecycle). Related broad claim: `docs/state/services/gateway/webhook-outbox.md:315`.

Every live outbox operation first awaits `store::run`'s `journal::finish`, which validates and publishes/acknowledges prior owed records before invoking the operation. Consequently the initial Issue audit must already be acknowledged before `registry::observe_expiry` can commit ExpiryObserved. Unlike an unknown missing historical transition, an actual retained owed Issue row plus a later retained ExpiryObserved phase contradicts that owner ordering. Lost file publication acknowledgement does not explain it: the next operation must finish that acknowledgement before observing expiry.

Reproduction in the actual reviewed owner, with synthetic temp data only:

1. Install a genuine registration; retain its original emitted Issue record, including its exact identity, target, issuer and timestamp.
2. Successfully observe expiry through the actual owner. Confirm acknowledged registration projections have trimmed.
3. Insert that exact original Issue record into the private audit table with `published=0`; leave the actual ExpiryObserved registration/fact unchanged. This models contradictory retained state, not a normal command.
4. `restoration::validate` unexpectedly returns `Ok(())`; cold `SqlOutbox::open` also succeeds.
5. With the audit directory temporarily moved, `due` returns **Unavailable**, proving validation reached the sink rather than returning **Invalid**. Restore the directory and `due` acknowledges the contradictory Issue record. No HTTP effect, authorization bypass, secret disclosure or fabricated actor is claimed.

Expected: Invalid before opening/publishing/acknowledging owed audit or doing cleanup. Actual: cold acceptance and live acknowledgement of an impossible retained prerequisite.

Evidence: `probes.rs::probe_reintroduced_owed_issue_after_expiry_observation` deliberately demonstrates acceptance and passes; **that pass is not a fix**. `issue-required-rejection-probes.rs` instead requires typed Invalid and fails against the frozen source, with `left: Ok(()) / right: Err(Invalid)` in `issue-required-rejection-fails.txt`. After that diagnostic, the private test source was byte-restored and the complete suite rebuilt.

Structural alternatives for the owner:

- Recommended minimal correction: make owed-prerequisite consistency explicit for Issue as well as Observation. An owed Issue requires a live Active registration and no retained cursor/delivery facts that prove later work has already happened; a retired registration cannot owe its initial Issue. Keep already-published historical Issue facts valid against their retained original facts. Implement this in the existing contextual owner, with positive initial-issue lost-ack recovery and negative live/cold/cursor/work/retired counterparts. This needs no new durable flags, parallel history, schema or fabricated provenance.
- Scoped deferral: retain this as a separate explicit issue/task with a disposition and narrow the completion/documentation claim to the repaired Observation/Queued relationships. Repository scope rules permit adjudicated deferral of a pre-existing case; they do not make the demonstrated contradiction valid.

No implementation fix was made during review.

## Original M1 disposition and checked boundaries

All three original acceptance demonstrations remain preserved at their original historical source. On this candidate, a new independently authored matrix exercised each manifestation with Owner, Delivery and Restart observations: **nine actual-owner histories**. Every history required typed Invalid before live SQL changes (registration/delivery/audit acknowledgement/retirement/cursor snapshots unchanged) and before file creation; cold refusal also preserved database bytes and audit file count. Restoring only the original valid phase or removing only the reintroduced prerequisite then recovered successfully.

| Relationship | Current enforcing owner | Independent evidence |
| --- | --- | --- |
| Owed Observation cannot belong to Stopped or retired registration | `restoration::contextual`, CredentialExpiryObserved owed guard and exact original fact comparison | Nine-history matrix, forms 0 and 2; private owed-phase removal is caught and byte-restored |
| Stopped cannot retain current Queued, even after registration projections trim | `RegistrationLifecycle::retains_queued`; `restoration::validate_queued` called before cold restoration and first owed publication | Matrix form 1; removing both current-work checks is caught and byte-restored |
| Queue admission consumes the same domain lifecycle rule before cursor/delivery commit | `queue.rs:31`–42 and domain `retains_queued` | Canonical stopped-admission regression passes; changing the domain rule to allow Stopped causes that admission regression to fail, then byte-restored pass |
| First reader/cause/time survives acknowledgement loss, restart, response settlement and later revocation | Domain observe/stop replacements; registry save mapping; contextual original-fact checks | Independent all-original-readers recovery probe plus known-response-before-stop probe; canonical lifecycle tests |
| Valid Stopped Sending / late Retry is retained for its actual outcome | Sending is not bulk-refused; late Response preserves transition; common cleanup uses original stop | Canonical lost-response-ack late-Retry/cold recovery passes; independent known Success and all-reader Sending→restart cases pass |
| Current Queued scan is bounded and excludes terminal-history traversal | Existing `ready_deliveries` index, DISTINCT registration, SQL limit MAX_REGISTRATIONS+1 and checked lifecycle lookups | Actual copied canonical VM-step regression at 1/1024 terminal rows and excess-distinct-registration refusal pass; these are metadata-only work probes, not complete restorable history fixtures |
| Existing Cursor/Delivery/Registration correlations still run before publication | Exhaustive Record match, source-reset retained references, delivery chain/current-state comparison, target/retirement correlations | Complete private baseline includes contextual substitution, complete owed-batch, source-reset, retirement, current-history and capacity regressions |

No blanket ban on Stopped+Retry/Sending was introduced or requested. A coherent Stopped+Sending state after all registration projections trim can be indistinguishable from genuine acknowledged stop under existing retained facts. This review does not infer corruption from missing history or require invented acknowledgement history. Published historical original facts remain eligible for correlation; owed prerequisites have a stronger ordering contract.

## Reproduction, provenance and limits

Artifacts live solely in `/tmp/nessa-expiry-context-final-review`:

- `auth-args.json`, `auth-compile.txt`: compiled **this candidate's actual nessa-auth source** into a private rlib. No Auth stand-in or separate dirty revocation source was used.
- `args.json`, `lib.rs`: compiled copied actual lifecycle/outbox/registration implementation and canonical tests with private Auth against immutable `/tmp/nessa-retention-dependency-snapshot`. The small unchanged data-only Target DTO is the harness seam. No Cargo, shared target rlibs or mutable primary artifacts were used.
- `baseline.txt`: **117/117** canonical copied domain/outbox/registration tests pass before independent probes.
- `probes.rs`, `probes.txt`: **4/4** independent probes pass, including the deliberately labelled M2 acceptance demonstration.
- `run-controls.py`, `controls.json`, `*-edit.json`, `*-broken.txt`, `*-restored.txt`: **three independently executed private source mutations caught**, with actual changed-byte hashes, successful compilation, expected assertion failure, byte-identical restoration and passing reruns.
- `issue-required-rejection-probes.rs`, `issue-required-rejection-fails.txt`: desired M2 rejection fails at this head, distinct from successful M1 rejection tests.
- `final-tests.txt`, `production-source-provenance.json`: final rebuilt private combined suite **121/121 pass** and source hash/clean-head checks. All copied Auth and webhook production source bytes match the reviewed checkout after controls.
- Independent `git diff --check 6a107073f9261dcfcaedb37b9c8c9fe773c1d470 HEAD` passed.

Read AGENTS, relevant coding standards (brief/loop/agreement/dimensions/audit/DDD/gates), architecture, codebase structure, typed DI, original R1 report and W24 M1 design table before source review. The new lifecycle rule stays in the pure domain; persistence decodes/maps facts, and Cedar remains the permission owner. The injected clock and existing physical-operation ownership are reused. The repair adds no authorization verdict, credential secret output, HTTP adapter, schema history or changed durability/flush semantics. Existing diagrams describe the lifecycle/acknowledgement split; the sourceRevision checkpoint names f75, and parent-provided render/validation records are present. I did not independently rerender diagrams or claim those author runs as mine.

This is Linux private-owner behavior evidence, **not** canonical Cargo platform qualification, full server integration, exact-head CI, Windows/macOS qualification, browser verification, gateway stress/chaos, recipient TLS/network policy testing or capability/revoke E2E. Those gates remain required for the composed feature; an unmounted adapter does not waive them. No claim of exactly-once remote effects or usable completed #896 is made. The separately dirty AccessSnapshot/revocation composition is excluded, as are actual offline/supersession coordination, scheduling, listeners and credential/provider registration. No live credentials, real recipients, network transmissions, GitHub writes or denied operations were attempted. Historical denied CI/report operations remain stopped and were neither retrieved nor reconstructed.
