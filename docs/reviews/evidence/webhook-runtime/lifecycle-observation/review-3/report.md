# Independent W24 round-three review

Reviewed frozen clean **4c993d457c0b1e5b3a05631fa09dcae21528737d**, branch `codex/896-webhook-composition`, checkout `/workspace/nessa-webhook-composition`, against published **8c6e673b69055bdc1b2320e4f1b990b5c795aeba**. Combined source is **efa4020c75347c42c3896f2d77363498aebb0eb6**. Reviewed all W24 production changes against that base, including the original lifecycle, mappings, schema, current/retained contextual validation, publication/cleanup ordering and tests, rather than only the latest Issue guard. Actual current AuthA source is included in the independent build. No checkout or shared artifact was changed.

**Blocker 0; Major 1; Minor 0; Nit 0.** Original M1 and the concrete round-two M2 expiry/current-cursor/current-delivery/retirement cases are fixed and their valid counterparts remain accepted. One structural Major remains: initial acknowledgement consistency is still inferred from an incomplete selection of retained facts. This round is not clean, and it does not qualify W24 for publication as a completed correction.

## M3 — retained acknowledgement evidence outside current cursor/delivery rows is ignored

**Severity: Major.** Owning locations: `crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs:85–98` (owed-Issue guard) and `:332` (`ISSUANCE_WORK`). The predicate asks only for current `cursors` or `deliveries`. It omits other retained facts that prove initial issuance already acknowledged, and validates sibling initial audit records separately instead of checking their relationship.

### A. Actual published Cursor history and its immutable original reset fact

Independent reproduction, using only temporary synthetic fixtures and actual reviewed owners:

1. Install a registration; save the exact actual original Issue record.
2. Queue a genuine record, then use the actual changed-source queue operation. It returns ResetRequired and retains its real immutable reset fact and published Cursor audit.
3. Delete the current cursor and its terminal delivery. The existing reference triggers release those owners while the actual published Cursor record keeps the original reset fact alive with refs=1. No actor, timestamp, source scope or historical reset identity is substituted or synthesized. The remaining history validates.
4. Reinsert the exact original Issue record as owed. The registration is Active. `ISSUANCE_WORK` returns false although the retained reset/Cursor facts prove a later operation occurred, which necessarily acknowledged initial Issue first.
5. Complete restoration and cold open accept. With the audit directory held aside, `due` reaches the immutable sink and returns Unavailable instead of Invalid. Restoring the directory lets `due` acknowledge the contradictory Issue.

Expected: typed Invalid before publication/acknowledgement/cleanup and cold effects. Actual: acceptance, sink access and local acknowledgement. No HTTP, authorization bypass or remote effect is claimed.

`probes.rs::diagnostic_retained_cursor_history_proves_initial_issue_completion` is a deliberately labelled acceptance demonstration, **not a fix**. `required_initial_rejection_with_only_retained_source_history` instead requires the contract and fails on this exact head (`left: Ok(()), right: Err(Invalid)`) in `required-rejection-fails.txt` and after restored controls in `required-restored-failure-tests.txt`.

A **private diagnostic only** adds source-reset existence to the predicate. That makes the stronger required live/cold/no-effects/original-repair test pass; byte-restoring the reviewed predicate makes it fail again. This locates one missing proof root. It is not a production fix or a complete structural solution.

### B. A genuine published initial Issue beside another owed initial Issue

The actual fixture already retains its original Issue row with published=1. Keep that actual acknowledged row unchanged. Add an otherwise matching owed initial Issue with a different valid record UUID, leaving the actual registration/owner/time unchanged. Both records separately satisfy the existing registration target/time decoder. Current work remains empty, so complete validation and cold open accept; `due` writes a second immutable initial-Issue audit file and acknowledges it.

The new UUID is an explicit adversarial identity substitution, not a claim that the owner legitimately created two original events. One registration identity has one initial issuance operation. The retained genuine published original proves that operation acknowledged; a contradictory sibling obligation cannot become valid merely by choosing another record UUID. `diagnostic_published_initial_issue_is_also_acknowledgement_proof` reproduces this and verifies the extra file. This is the same missing enclosing progress/identity relationship as A, not a separate permission problem.

The first exploratory versions attempted to reinsert Cursor/Issue rows that were already present and hit UNIQUE constraints. Those fixture failures are preserved separately; the corrected probes keep the actual existing published rows and reproduce the acceptance. They are not counted as production failures.

## Structural decision required by the bounded review process

Round three still exposes a new case in the same restoration component. Do not append another isolated EXISTS exception and call the proof complete. The issue is that contextual restoration has a total match on individual Record variants, while the registration's retained progress and initial-event multiplicity are assembled only partially. Both alternatives below belong in the existing registration/publication/restoration ownership; neither needs a parallel permission system or fabricated actor/history.

### A. Total derived retained-progress classification

Define and review one complete classification of the facts a registration retains, then ask that owner whether an initial obligation can still be owed. The classification need not be a new persisted flag. It must distinguish absence/undetermined history from positive proof of acknowledgement and contradictions, including competing initial audit identities. Its inventory must cover the full schema and closed Record families:

| Retained family | Meaning for initial acknowledgement and existing correlation |
| --- | --- |
| Registration intent | May precede installation/issuance; its presence alone does not prove acknowledgement. Preserve matching selected-input ownership. |
| Registration lifecycle | Active alone is ambiguous. ExpiryObserved or Stopped proves initial acknowledgement; existing exact original observation and owed phase relationships remain required. |
| Current cursor | Proves later work; preserve full identity/scope/reset correlation. |
| Current delivery, including terminal | Proves later work; preserve complete owning audit chain and current-source rules. Sending/late Retry may remain valid after Stop. |
| Retained source-reset fact | Proves later work even after its current cursor/delivery disappears. Preserve actual immutable identity, source facts and positive owned reference accounting. |
| Source-reset member | Subordinate to its delivery/fact; dangling references are invalid. Do not silently omit the family because another normal root often covers it. |
| Registration Record: Issue | Owed versus published are different facts. Published original Issue proves initial acknowledgement. Enforce the relationship among all retained initial record identities for that one registration, including duplicate/competing obligations. One individually valid row is not the whole history. |
| Registration Record: ExpiryObserved / Revoked / Expired / Drained | Each proves progress beyond initial acknowledgement; preserve current/retired exact-fact and prerequisite relationships for owed versus published history. |
| Cursor Record, owed or published | Proves later work. A published historical Cursor can survive current cursor replacement/deletion through its retained reset owner; preserve that valid history. |
| Delivery Record, owed or published | Proves later work; require its retained delivery/history relationship rather than treating an orphan as a trusted proof. |
| Retirement | Proves initial and stop progress. Keep its original target/stop facts and exact owed retirement identity until references settle. |

Compute this through bounded indexed reads/streaming, not a new all-history scan per owed row. Existing registration-leading cursor/delivery/source-reset indexes cover those roots. Historical initial/sibling audit lookups currently lack a registration-leading audit index; choosing classification must explicitly account for or index that work and test it at per-registration and cross-registration maxima. A single bounded pass over a closed set of related records is another possible implementation. Ownership of that complete relation, its empty/ambiguous cases, and its contradiction outcome is the structural change; the particular SQL spelling is secondary.

Semantics: reject only contradictions positively proved by retained facts; do not invent erased acknowledgement history or reject a feasible initial file-written/ack-lost recovery. Published historical rows remain valid in isolation and after legitimate later work. The existing fifteen-case regression and original observed/late-response cases must remain controls.

### B. Retained initial identity plus monotonic local acknowledgement

Bind a registration to its canonical initial audit identity and retain explicit acknowledgement progress under the existing publication owner, including the minimum secret-free facts needed through retirement. Publish/sync the exact immutable file first, then atomically record the matching audit acknowledgement and progress; a file-written/before-ack crash must remain idempotently recoverable. An initial identity by itself is insufficient: after published rows trim, the same original identity could otherwise be reintroduced as owed. The progress fact must be monotonic and correlated to that exact identity.

This removes the need to infer whether initial acknowledgement happened from every possible later root, but adds durable state and ordering obligations across install, publication, restoration, cleanup and retirement. Retention remains at most one such identity/progress fact per bounded registration/retirement owner, with explicit cleanup/capacity behavior. Do not synthesize acknowledged progress for ambiguous old state. Its tests must cover before-file, after-file/before-ack, after-ack/before-trim, original/changed identity substitution, later-source histories, retirement and lost observer/restart.

This is stronger semantics and greater schema/mapping/test cost than A. The owner should explicitly choose whether that precision is justified under gates15/16. I have not implemented either option. The current round-two narrow fix remains useful but does not establish the complete monotonicity contract while M3 is open.

## Independent execution and checked relationships

All artifacts are in `/tmp/nessa-w24-m2-independent-review` and all production code is an actual byte copy of this candidate.

- Private baseline: **122/122** canonical copied lifecycle/outbox/registration tests pass, including actual Auth-backed registration and new joint original expiry/later revocation/retirement restoration.
- Independent Auth unit target: **214/214** pass using this candidate's copied actual Auth source, not a stand-in or an older worktree rlib.
- Three independent probes pass: original M2 rejection across all three reader kinds with **first cold open** preserving bytes/files and original-state recovery; retained source-history acceptance demonstration; published original Issue sibling-obligation acceptance demonstration. Passing acceptance demonstrations are explicitly not fixes.
- Separately compiled desired M3 rejection fails on the unchanged source. The source-root diagnostic makes its complete live/cold/no-effects/recovery requirements pass, then byte restoration reproduces failure again.
- Three compiled private one-rule mutations caught with changed-byte hashes and successful byte-restored reruns: remove owed-Issue guard; apply it to published history; omit current work proof. `controls.json` and broken/restored logs record the actual assertions. No checkout mutation occurred.
- Final rebuilt suite includes canonical baseline plus the three passing probes and deliberately excludes the separately recorded desired-rejection test which still fails. Final result: **125 passed,0 failed,1 explicitly filtered desired-rejection test**. Source hashes and the exact clean head are in `production-source-provenance.json`.
- Full incremental diff check against8c6 passes; exact head/status and every copied Auth/webhook production file are checked after controls. Only the private test driver adds independent probes. The initial duplicate-fixture errors and expected desired-rejection failure are retained honestly.

Read AGENTS, relevant coding-standard gates and complete brief/loop/agreement/dimensions/evidence/audit/DDD requirements, architecture/codebase ownership, typed DI and original W24 design/review records. Checked pure lifecycle replacements and first-reader preservation, cause/actor/time codecs and domain-authoritative audit shape decoding; all Registration/Cursor/Delivery contextual arms; current Queued compatibility shared with admission; original reset references and retirement; exact immutable file acknowledgement and maintenance order; bounded selectors, public immutable values/module maps and current doc checkpoints. M1 regressions and valid Sending/late Retry remain covered by the actual baseline. No new policy decision, credential secret output, network adapter or product mount is introduced. Existing Cedar/current authorization holds remain the future composed authority boundaries.

Author canonical152-webhook, seven-package Clippy/fmt/architecture, seven controls and NessaDocs135/0/three-diagram evidence are supporting checks inspected, not reviewer reruns of those canonical/platform/browser commands. The page checkpoint namesefa4020c correctly and distinguishes the inner checkpoint from runtime completion. I did not independently render the diagrams.

## Exclusions and operations

No full main-based PR acceptance or #896 completion is claimed. Canonical Cargo package-selection/platform qualification, Rust minimum-version execution, Windows/macOS, exact-head CI, scripted Chromium/WebKit, default/raised stress/chaos, actual recipient capability/revoke/stale-epoch E2E and runtime command/configuration/share/source/sender supervision remain unqualified here. General offline/supersession outbox mapping remains pending despite actual Auth provenance being present. No exactly-once remote-effects claim.

No live credentials, provider/recipient registration, public listener, network transmission, production data, GitHub mutation or account/network/security change occurred. No denied CI/browser/issue action was retried, and no old security-flagged registration report was retrieved/reconstructed. All private investigative changes were restored; no action was denied or has an uncertain outcome in this review.
