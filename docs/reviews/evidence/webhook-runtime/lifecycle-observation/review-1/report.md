# Independent W24 expiry-observation review, round 1

## Frozen scope and result

Reviewed clean candidate **4953b2882f58e3db5deb37e9657b49d65dab976d**, branch `codex/896-webhook-composition`, checkout `/workspace/nessa-webhook-composition`, against published correction base **6a107073f9261dcfcaedb37b9c8c9fe773c1d470**. New lifecycle source is `27515a3e4629db4b401cbf2333c424bd48924807`; combined tested source is `0ef30e68332703f6f42a5feafd2d56aa4b496c24`; later merged publication evidence at `3a1083da` changes no crate source. No checkout/Git/shared Cargo artifact was modified.

**Blocker 0; major 1; minor 0; nit 0.** The one Major is a lifecycle restoration relationship with three reproduced manifestations. This candidate is not clean for publication. The successful probe suite includes deliberate demonstrations of acceptance of invalid retained state; their passing results are **not fixes or successful rejection tests**.

## M1 — restored registration phase is not correlated with observation acknowledgement and current work

**Severity: Major.** Primary location: `crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs:86–100`, with the adjacent current-history relationship missing at `validate_histories` around lines260–271. `journal::finish` then treats the accepted phase as authority for cleanup at lines125–147.

The actual stop owner establishes two relevant relationships. `registry::stop` awaits expiry observation acknowledgement before starting the stop transaction (lines56–58). That stop transaction changes the phase and refuses Queued/Retry rows atomically (lines68–83 and `journal::refuse_pending`). Its initial `run`/`finish` and stop commit also trim published registration projections. Restoration currently compares the observation actor/time with `lifecycle.observation()` or a retired stop fact, but does not check that the saved phase and acknowledgement/work state can have arisen in that order. It accepts individually valid facts that contradict this owner.

Three actual-owner/private SQLite reproductions in `probes.rs` all passed as demonstrations:

1. **Owed observation with Stopped phase.** Call `observe_expiry` with the audit directory temporarily unavailable: it commits ExpiryObserved and leaves one owed CredentialExpiryObserved row. Change only `registrations.active` from1 to0, preserving the exact original cause/reader/time. `restoration::validate` accepts; cold `SqlOutbox::open` accepts. After restoring the directory, `due` publishes the observation and retires the registration with Drained, despite no CredentialExpired registration stop record ever occurring. This cannot be a valid acknowledged-observation-before-stop execution.
2. **Acknowledged/trimmed observation with Queued work.** Queue a genuine delivery and successfully observe expiry. Confirm the registration audit projections have been acknowledged and trimmed. Change only active1→0. Current Queued delivery/history plus Stopped registration is accepted on validation and cold reopen. `due` silently converts it to authority-refused and publishes causal delivery/drained audit, again without a registration stop transition. The canonical stop transaction would have refused that Queued row atomically; queue admission after stop is refused. Therefore checking only for an owed Observation row is insufficient.
3. **Retired registration with a reintroduced owed observation.** Perform a genuine acknowledged observation, then stop with the sink unavailable, leaving actual Stop/Drained retirement facts. Reintroduce the original exact Observation row as owed. Restoration accepts it against `retired.stop`, cold reopen succeeds and owed publication proceeds. The retained observation owing after stop/retirement contradicts the same acknowledgement barrier. This is a cross-record mutation demonstration, not a claim that normal code reintroduces it.

Expected for these contradictory retained combinations: typed Invalid before owed publication, cleanup or replacement of prior evidence. Actual: successful restoration followed by publication/resource retirement based on the substituted phase. No HTTP or other recipient effect occurred, and this report does not claim an authorization bypass.

The fix belongs to the shared contextual lifecycle restoration owner, before both startup effects and owed publication. It needs the complete relationship between current lifecycle, retained registration projection/acknowledgement, and current delivery history. A narrowly added owed-Observation check alone misses reproduction2. Preserve the valid recovery counterparts below; do not reject every Stopped registration with pending work.

## Adjacent valid histories and proof limits

Independently reproduced positive cases:

- Each of the three original reader kinds (actual owner principal, delivery UUID and Restart) survives a lost observation publication acknowledgement, cold restore, a later different owner read, unresolved Sending recovery and a later revocation. Original reader/time remain; observation is not duplicated; retirement/reopen succeeds.
- An observation followed by a known Sending response before stop keeps the response's observation time separate from the original stop cause/time. Later owner expiry cannot replace Restart; cold reopen and retirement remain valid.
- **Stopped plus Retry can be valid.** Genuine stop while Sending is followed by a late Sending→Retry response whose publication acknowledgement fails. The current row is Retry, the original stop is retained, and cold restore is valid. Recovery then publishes the known response and refuses the retry with the original cause. A blanket ban on all pending phases would break this path.

Distinguish these provable contradictions from indistinguishable histories. After an observation has acknowledged and its SQL projection has trimmed, a consistent saved Stopped+Sending state can have exactly the same retained SQL fields/history as a genuine stop whose own registration projection has also acknowledged and trimmed. This review did **not** label every such phase substitution detectable from current retained facts. Current metadata is itself an authority; arbitrary coherent rewriting is not disproved by invented missing-history assumptions. If the required contract additionally demands proof that every Stopped assertion is backed by a retained original transition/ack identity, a bounded retained transition fact/reference is a structural option, but that strengthens the retained proof contract and must be designed explicitly. It is not necessary to invent history merely to reject the demonstrated owed-observation or Queued contradictions.

Minimal structural alternative: have one contextual owner validate all relationships actually provable from retained phase, audit and delivery history, including the acknowledgement barrier and valid late-Sending result cases. Stronger alternative: retain bounded original phase-transition/ack identity across cleanup, derive or correlate phase from it, and release it through the existing retirement owner. Do not duplicate uncorrelated actor/time fields. Parent adjudication should choose the intended proof contract before patches.

## Independent checks and mutation evidence

All artifacts are under `/tmp/nessa-expiry-observation-independent-review`.

- `provenance.json` records copied source hashes. `auth-args.json` independently compiles actual current Auth from private copied source against immutable third-party dependencies in `/tmp/nessa-retention-dependency-snapshot`; `args.json` independently compiles actual copied lifecycle/application/SQLite code and its tests. The private driver substitutes only the unchanged data-shaped Target. This is not canonical Cargo package-selection or full gateway composition qualification.
- `baseline.txt`: **109 passed,0 failed**, including the original full1024 boundary tests, real Auth/registration cases, existing persistence/schema/reset tests and supplied W24 domain/lifecycle regressions.
- `probes.rs`/`probes.txt`: **six independent tests passed**, comprising three positive lifecycle scenarios and the three invalid-state acceptance demonstrations described above. The first positive scenario iterates all three original reader kinds.
- `controls.py`/`controls.json`: two genuine private fail/restore controls were caught: removing the observation acknowledgement call and replacing the first cause on later revocation. Actual copied bytes changed, compilation used those bytes, each test failed, and byte restoration followed by a new compile/test passed.
- A third **diagnostic-only** private phase guard made the owed-Observation acceptance demonstration fail while preserving the three-reader positive probe. It was byte-restored. This locates one missing correlation; it does not implement or qualify a complete fix, and it does not address the acknowledged/trimmed Queued reproduction.
- `final-115.txt` records the final restored suite (109 existing tests plus six independent probes) after completion. The demonstrations intentionally continue accepting the invalid states on the unchanged reviewed implementation.
- Full publication-base `git diff --check` passes; checkout remains clean at the exact candidate. No denied action occurred or had an uncertain outcome in this review.

## Agreement, ownership and exclusions

Read AGENTS, coding gates/review dimensions/agreement/audit/DDD and DI guidance, architecture/module ownership, W24 design table before code, state documentation, and the complete changed production/test diff. Checked domain replacements, shared actor/time codec, record encoding/decoding using the domain transition owner, SQL state/projection, original fact through late readers, current delivery history, publication/exact acknowledgement, failure and retirement. The new immutable domain value preserves the first observation through normal live calls; the confirmed gap is contextual restoration of the enclosing history, not isolated enum validation. Registration retirement and publication bounds/flush owners remain reused; no parallel permission system was added.

The parent's canonical139 webhook tests, seven-package all-target locked Clippy, fmt/architecture and135-page/three-diagram evidence are supplied supporting checks, not reviewer re-executions. Auth unit tests were not separately executed in this W24 round, though actual Auth was independently compiled and used by the baseline registration tests. Existing clean auth/cost/publication reviews remain their own scoped evidence. Windows, canonical Chromium/WebKit, gateway stress/chaos, exact-head CI, command cancellation/supervisor lifetime and actual recipient capability/revoke E2E are unqualified here.

The held snapshot's missing canonical revocation actor/cause and principal-only private Revoked input are accurately identified as a pending composition boundary. Offline LocalOperator/supersession semantics must come from the existing auth journal owner, never a fabricated principal. No claim of that integration is certified here. Product mount, command owner, startup reader, grant-aware source and sender remain unbuilt. No full #896 completion or merge authorization.

No live credential, registration, recipient transmission, public listener, production data, account/network/security change, GitHub mutation, CI retry/lookup/dispatch, browser download or old flagged report recovery was attempted. Historical denials remain unchanged. Candidate stays unpublished.

## Final restored check

`final-115.txt`: **test result: ok. 115 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 16.44s** No implementation bytes changed; all three acceptance demonstrations remain reproductions, not fixes.
