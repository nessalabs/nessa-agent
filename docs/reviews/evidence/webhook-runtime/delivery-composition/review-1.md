# Issue 896 whole-feature review — round 1

Disposition: changes required. This is not issue completion or merge readiness.

Frozen checkout: `/workspace/nessa-webhook-delivery-review`, clean HEAD `eafa4dfc31e22c2e5aeda8917c03d404b6571da5`; production/test source `fc713591785970b8eca5ec20ad819024bf131cb0`. Verified merge-base with actual main is `37c7772244dde5ab7d9ba382196b3f34fd0192fc`. Reviewed complete feature boundaries against that base and assembled delivery against published `3a347ad382de24f9316c44b5ab4426dd84e3b8be`. Findings below include older feature code: prior component review is not whole-feature acceptance.

## Findings

### R1 — P1: the registration authority holds have no attempt deadline

`crates/nessa-server/src/webhooks/infrastructure/runtime/delivery.rs:382–410, 441–458`; `application/source.rs:54–152`; `infrastructure/https.rs:140–170`; `infrastructure/outbox/store.rs:99–127`.

The mounted attempt retains the real credential and grant holds in `head` while awaiting source_after, the Sending transaction/publication, and the first response settlement. Only the HTTPS preparation/POST future has a timeout. No deadline branch releases these holds during a pending physical source/store/publication operation. The HTTPS deadline actually starts before admission. A blocked SQLite transaction or audit flush after admission therefore keeps canonical revoke waiting past 15 seconds; a completed response followed by a blocked settlement does too. Source read ownership also retains the same fences independently until the physical worker joins, so merely dropping an outer future would not fix the ownership contract.

W8/W15 and W-deadline-queued/W-revoke-wait expressly bound the permit from acquisition even if persistence fails; W-deadline-known retains the actual response while freeing authority. The mounted code does not implement that contract. Add a deterministic paused physical Sending or settlement operation, advance the attempt deadline, and prove revoke finishes without POST/new authority while original physical work remains honestly owned. Do not add only an outer timeout around spawn_blocking: its late transaction must not commit an invalid transition after authority is released. Reconcile the ordering/design table before implementation.

### R2 — P1: a new issue request cannot retire/reissue an expired registration

`crates/nessa-server/src/webhooks/infrastructure/runtime.rs:276–286`; `infrastructure/outbox/intents.rs:129–154`; `application/registration.rs:171–190`.

`issue_command` returns Conflict for any same-principal retained intent with a different issuer/request pair, before current credential observation or registration-stop coordination. Installed intents are retained with the registration. Start with an issued registration, expire its credential, then issue for that principal with a fresh requestId: the old intent guarantees Conflict before W-issue-expired can run. Likewise a revoked/stopped registration with queued rows deliberately remains retained, so W-reissue cannot install its new registration while those rows drain. Replaying the original requestId only resolves the original command, not a new issuance.

The existing configured_replay test proves refusal while current, but never crosses expiry/stop. Preserve that valid refusal and uncertain uninstalled-intent ownership while distinguishing them from a completed installed registration. Required public-boundary controls: fresh request after expiry with no queued rows; fresh request after stop with retained nonterminal rows; current registration still refuses; identical original replay preserves identity.

### R3 — P1: a transient Sending write/ACK failure permanently disables live recovery

`crates/nessa-server/src/webhooks/infrastructure/runtime/delivery.rs:406–410`; `runtime.rs:155–166`; `outbox/store.rs:156–173`.

Any admission transition error escapes delivery_cycle/loop. start_delivery then sets failed=true and accepting=false permanently. If the write rolled back, the row remains Queued/Retry and no conditional NotSent/audit-unavailable transition or live rescan runs. If Sending committed but its audit publication ACK was lost, no actual POST began and no KnownObservation exists; the live worker nevertheless exits rather than reconciling that row to Unknown when storage returns. Restoring the audit directory is insufficient: every later configured issue/revoke/share/unshare through this owner is refused and the sender never scans again. Only a process restart recovers it.

This contradicts W7, W19 and W20: failed/uncertain local persistence must retain its real identity/state, recover on a later successful outbox operation, and must not require exit. The existing missing_sending_ack control explicitly closes and cold-restarts, so it validates no-send/cold Unknown but leaves this live requirement untested. Distinguish precommit failure from committed-but-unacknowledged Sending; never infer ACK from the error or resend a Sending id. Exercise restored storage in the same live owner, with commands/control availability and no repeated POST.

### R4 — P2: idle registrations never retry credential-stop observation

`crates/nessa-server/src/webhooks/infrastructure/runtime/delivery.rs:132–163` and `application/registration_stop.rs:139–186`.

Each cycle reads every retained registration, then only calls credential admission/ObserveRegistrationStop through capture for granted conversations or through a due delivery. With no grants and no pending rows, an expired registration is repeatedly read but never stopped. The same gap prevents retry of a failed credential-revoke registration stop when no delivery exists. URL/key/intent stay retained and R2 additionally blocks a fresh issuance. Startup recover does observe the stop, but W14/W-cred-retry require the next live registration read to retry, independent of queued work. Add idle expiry and failed stop→repaired storage controls with original initiator preserved. No timer at expires_at is needed: the existing read loop is already the trigger.

### R5 — P2: the module map still says the implemented product path is uncomposed

`docs/codebase-structure.md:1678–1682` says registration configuration, command coordination, source reader, sender supervision and E2E remain uncomposed. The new source and evidence document actual composition. Update this map to name the real owners and reserve incomplete status for actual review/CI/platform/browser/stress or runtime-contract gaps. Keep original historical failures and qualifications intact.

## Relationships and boundaries inspected

Read AGENTS and CODING_STANDARDS including the local-review/agreement/round-cap instructions; relevant architecture, structure and typed-DI guidance, plus access-grants W invariants, transition/crash rows and acceptance slice. Traced owner public issuance/configuration to native Auth origin and immutable intents; real principal share vs receiver authority; current credential/membership/audience/owner and grant epoch/Cedar admission; Head/Page identity, physical SDK lease and capacity; contiguous source cursor/queue and fresh due selection; durable Sending before production HMAC/HTTPS dispatch; known remote response vs SQL commit/publication ACK; cold Unknown vs retry identity; retained registration stop/cancellation evidence; host passive construction/start and joined shutdown/reporting. Key files: composition/{local_auth,root,webhooks}.rs; product read_access/state; webhooks application/{admission,source,share,registration,registration_stop,outbox}.rs; infrastructure/runtime{,/delivery}.rs, https.rs, outbox/{store,intents,retirement,journal}.rs; conversation store/share_grants.rs; native Auth registry fencing. Reviewed the new product/delivery fixtures and delivery-composition README/control/source-page evidence. This is a targeted whole-feature boundary review, not an assertion that every historical evidence file or every unchanged SDK implementation was reread.

Positive agreement checks: MAC keys are private configuration rather than authority; registered principal admission is distinct from paired receiver scope; immutable origin binds credential/issuer/request/transition; queued epoch is freshly rechecked for sending; source scope/head is correlated to the pending checkpoint; actual response is captured before fallible Clock callbacks; a retained exact response audit is required for ACK recovery; missing proof never fabricates ACK; cleanup waits for physical POST and preserves failure after later settlement. Source_after drops the first Head hold before acquiring the Page hold. The production root starts the sender only after fallible startup and directly enters its existing serve/cleanup owner. No new finding in those inspected relationships beyond the deadline/live-recovery issues above.

## Independent execution

No checkout/source edits, publication, CI rerun or broad suite/retention rerun. Used author checkout `/workspace/nessa-webhook-delivery` only for warm focused tests. Independently verified `git diff --exit-code fc713591785970b8eca5ec20ad819024bf131cb0 -- crates Cargo.lock Cargo.toml` returned 0, so tested production/test bytes match frozen source. Environment: CARGO_HOME=/workspace/nessa-toolchain/cargo; RUSTUP_HOME=/workspace/nessa-toolchain/rustup; PATH prepended toolchain cargo/bin; CARGO_TARGET_DIR=/workspace/nessa-webhook-registry-order/target; CARGO_INCREMENTAL=0.

- First overly specific filter `product_delivery::missing_sending_ack...` selected zero tests; this is not a pass claim.
- Correct missing_sending_ack_makes_no_post_and_cold_recovery_never_reposts_unknown: 1 passed.
- configured_replay_preserves_identity_and_a_new_command_conflicts_before_reservation: 1 passed.
- `cargo test -p nessa-server --lib webhooks::infrastructure::product_tests::delivery:: -- --nocapture`: all 13 passed, 2.14 seconds. Expected synthetic post-response Clock panic was printed and its owning test passed. Session 80326 ended exit 0.

Those existing tests corroborate valid narrow behavior; the new adjacent cases above are source-backed counterexamples, not claimed fresh failing reproductions. Read-only snapshot restriction was preserved. Implementer reports 258 webhook/38 host/Clippy/fmt/architecture results; this reviewer did not rerun or independently certify those broad gates. Fresh source-matched Windows/macOS/Linux CI, browser qualification, default/raised stress and exact final merged-tree validation remain unverified. No issue-close or merge-ready recommendation.
