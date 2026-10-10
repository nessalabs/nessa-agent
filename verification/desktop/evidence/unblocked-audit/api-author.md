# Narrow unblocked API author handoff

Author selfreview, not an independent approval. Base: `965797a9f2cb1a71923f32eee4197aa501669ebf`; branch: `codex/712-unblocked-audit`; checkout: `/Users/nessa/.codex/worktrees/audit-desktop-history/nessa-agent`. Commit: 25ab96ae9ffa475761b1b95d85a44da31805193d.

## Scope and paused features

Only #712 (mode intent/terminal/projection/current-publication ownership), #726 (metadata target relationship), and #727 (Created acknowledgement origin) are implemented. The parked `d49c6777e445d5ee1558699f46af691a2a71db5a` is reference material. No history route, history DTO, paging, retained-page view, new selection/display schema, passive display-history feature, desktop consumer, client decoder, ManagedSession/adoption feature, or blocked issue was extracted. Existing main #725/#728/#729 peer/device read-grant behavior remains; the only changes to its passive admission and share paths ask the shared metadata-target owner before using a loaded record. There is no second grant policy.

`protocol/`, `packages/`, `src/`, `src-tauri/`, `crates/nessa-sdk/`, `Cargo.toml`, and `Cargo.lock` are byte-identical to main. The JSON source inventory holds every changed file's SHA256; the three mutation-target source hashes also match the exact restored snapshots. Earlier structural reviews at 3a38 and peer-integration review at 337 are historical regional evidence only. They do not approve this extraction or the combined root branch.

## Ownership and agreement selfreview

| Related facts | Owner and enforcement | Public regression evidence |
| --- | --- | --- |
| Requested metadata target versus loaded/create-ACK target, before owner, grant, selection, audit or provider effects | `application/metadata_target.rs::validate_target/load_conversation`; every application metadata load consumes it | metadata_query_target_fences_effect_entry_paths; metadata_query_target_fences_publication_after_summary_await; metadata_query_target_fences_passive_admission_and_shares; creation_metadata_target_refuses_foreign_acknowledgement_and_accepts_stored_retry |
| Created target/org/owner/agent/model/mode/creation surface/action/time versus original immutable proposal | `service::create` keeps proposal and requires full Conversation Eq for Created after target/access; Existing preserves historical receipt | creation_created_acknowledgement_preserves_the_admitted_origin; creation_mislabeled_historical_created_refuses_before_audit_and_recovers; creation_metadata_target_accepts_existing_historical_actor_and_separate_reopen |
| Mode target/request/org/principal/surface/prior/requested/time versus begin ACK | Owned `set_approval_mode` retains original intent; pending reply must equal it before provider application | terminal_admitted_intent_cannot_be_replaced_before_provider_effects; terminal_inverse_intent_cannot_claim_ask_over_an_actual_auto_provider; terminal_initial_query_rejects_borrowed_receipt_before_any_effect |
| Application audit versus effect actually observed and original initiator | Application request derives from original intent plus locally observed Applied/Deferred/Uncertain; observe reply cannot replace attribution | terminal_application_audit_keeps_original_intent_and_observed_effect; terminal_recovery_audits_cannot_borrow_replacement_metadata_facts; terminal_pending_recovery_keeps_historical_actor_time_and_action |
| Terminal target/actor/prior/requested/application/time/state versus admitted operation | Complete expected Applied equality on terminal write or readback; write construction/polling and readback construction/polling panic contained; unknown settlement retires held owner before recovery | terminal_each_record_fact_is_correlated_on_write_and_readback; terminal_retry_rejects_each_foreign_query_identity_before_effects; terminal_committed_invocation_and_poll_panics_reconcile_before_publication; terminal_uncommitted_panics_recover_prior_and_allow_a_later_mode; terminal_unreadable_readback_retires_the_held_owner_before_recovery |
| Pending query versus enclosing loaded target/org/owner/prior/Pending state | `pending_for` refuses before recovery cleanup/audit or pending read-only view; original actor/time survive retry | terminal_pending_query_is_correlated_before_cleanup_and_publication; terminal_loaded_conversation_must_be_the_pending_query_target; terminal_saved_state_without_application_is_not_claimed_as_a_receipt |
| Committed selection/runtime versus projection revision | Private Projection.view, immutable view(), equality-checked replacement methods and approval_mode through selection owner | protocol replacement/equality tests + 2 compile-fail doctests; confirmed_mode_changes_revise_current_reads_for_normal_and_recovered_commits |
| Admitted command versus disappearing caller | Existing supervised operation owns settlement; current read stays serialized on main's mode lock until acknowledgement | admitted_mode_change_outlives_caller_and_revises_only_after_acknowledgement; terminal_committed_panic_settles_after_the_caller_is_dropped |
| Unconfirmed physical cleanup versus current view/follow and release | Existing Slot.stopping/release_slot remain sole owner; live_now/read_at refuse retained stop, final same-Arc check after summary/access awaits; explicit close joins retirement path | retained_stopping_publication_refuses_current_reads_until_confirmed_cleanup; retained_stopping_publication_rechecks_owner_after_summary_wait; retained_stopping_publication_refuses_wire_and_subscription_until_cleanup; existing desktop_stop suite |

The final current-publication race is reachable through the public gateway shutdown: an admitted current read holds the summary await, shutdown exhausts its documented admission-drain deadline, then retires the held owner. The controlled-clock test checks both confirmed and unconfirmed physical cleanup, typed RetirementAdmission, typed Closed after the await, live-change wake after confirmed release, and later cleanup. Explicit close and desktop stop share the mode lock with current reads; the test does not invent a producer that bypasses that lock. Pending recovery views remain explicit and read-only. No second cleanup flag or mode authority was introduced.

Source/test/DDD/module organization selfreview completed. New metadata-target helper belongs in conversation/application, injected through existing repository port; fault controls remain test-only constructor dependencies in tests/conversation/support.rs. Existing ADR231 gains the current ordering table; application mod.rs and codebase map name source/test ownership. Projection callers use the immutable getter. No compatibility alias, unused history helper, new wire contract or production test switch remains. No unresolved selfreview finding at any priority. Fresh independent review is still required.

## Checks and before/fixed evidence

All commands used the private checkout and `CARGO_TARGET_DIR=/Users/nessa/.codex/worktrees/audit-desktop-history/nessa-agent/target`; no shared/root target, source, vendor or browser execution was used. Captured outputs are in `/tmp/unblocked-api-author/`.

| Command or public witness | Result | Evidence |
| --- | --- | --- |
| cargo test -p nessa-server --lib (full normal unit suite) | 2,188 pass, 10 pre-existing ignored, 0 fail; 302.40 s runtime | server-full.log |
| cargo test -p nessa-protocol | 118 unit tests + 2 compile-fail doctests pass | protocol.log |
| cargo clippy -p nessa-server -p nessa-protocol --all-targets -- -D warnings | PASS; 17.83 s | clippy-final.log |
| cargo fmt -p nessa-server -p nessa-protocol --check; git diff --check | PASS on final restored tree | final author check |
| Main public creation witnesses using main production and test-only fixture registration | 4 selected: 3 expected behavioral failures, 1 legitimate historical Existing positive pass | main-created-witness.log |
| Main public current revision, terminal panic, substituted intent, observed application audit witnesses | Four separately selected tests, all expected behavioral failures, no zero-selected result counted | main-mode-witness-results.json and four main-*.log files |
| Final restored mode and creation regressions | 24 + 4 pass | restored-mode.log; restored-created.log |
| Final restored immutable projection docs | Both compile-fail examples pass | restored-docs.log |
| Source-asserted individual rule removals | 15 intended failures; all sources restored with fresh modification time and SHA256 match | revert-results.json; *-source.diff; *-revert.log; revert-restoration.json |

The final lint-only correction removes an unnecessary test helper borrow after the full server suite. The restored 24-mode/4-creation suites cover that helper; production sources are identical to those tested by the full suite and the mutation snapshots. Temporary revert tests are not counted as additional clean-suite passes.

The 15 rules are metadata-target correlation; Created proposal equality; admitted-intent equality; observed-effect audit ownership; terminal equality; terminal write panic containment; terminal readback panic containment; pending envelope correlation; stopping publication fence; post-await same-owner fence; selection revision; selection equality; runtime revision; runtime equality; private projection storage. The first 14 mutants build and fail behavioral assertions. Making projection storage public causes its direct-access compile-fail example to compile, so that doctest intentionally fails; the immutable-getter compile-fail counterpart remains passing. The privacy retry's log contains both examples (1 expected failure + 1 pass), despite the scratch filename saying isolated. This is a valid type ownership proof, not a cargo compilation failure.

Excluded attempts: initial extraction/dangling-doccomment compilation errors; initial test binary with a zero-selected filter; two mode test adaptations that assumed the paused nonblocking history semantics instead of main's serialized current read; and the first privacy validator wrapper which mistakenly classified expected compile-fail stderr as a compiler fault. None is claimed as a behavioral pass or load-bearing proof. The corrected public current witness and corrected privacy validation are the evidence reported above. A first Clippy run's test-only needless borrow was fixed and its exact final command rerun successfully.

## Remaining integration gates

Root owns all GitHub board/PR/status/push/merge work. This local commit is ready to cherry-pick into its unblocked combined branch. Root still owes fresh independent API review, combined exact CI Rust/build checks, and the required browser/scripted/performance evidence for combined gateway/UI behavior. This author did not run new browsers or performance work and does not claim those gates passed. All owned command sessions ended before handoff; no source probes or processes remain running.

## Exact file inventory

- `crates/nessa-protocol/src/conversation/projection.rs`
- `crates/nessa-protocol/src/conversation/view.rs`
- `crates/nessa-protocol/tests/conversation/projection.rs`
- `crates/nessa-server/src/conversation/application/environment.rs`
- `crates/nessa-server/src/conversation/application/metadata_target.rs`
- `crates/nessa-server/src/conversation/application/mod.rs`
- `crates/nessa-server/src/conversation/application/passive_read.rs`
- `crates/nessa-server/src/conversation/application/ports.rs`
- `crates/nessa-server/src/conversation/application/read_grants.rs`
- `crates/nessa-server/src/conversation/application/service.rs`
- `crates/nessa-server/src/conversation/application/service/app_calls.rs`
- `crates/nessa-server/src/conversation/application/service/creation.rs`
- `crates/nessa-server/src/conversation/application/service/mutation.rs`
- `crates/nessa-server/tests/conversation/approval_mode.rs`
- `crates/nessa-server/tests/conversation/creation_metadata.rs`
- `crates/nessa-server/tests/conversation/desktop_stop.rs`
- `crates/nessa-server/tests/conversation/passive_read.rs`
- `crates/nessa-server/tests/conversation/support.rs`
- `crates/nessa-server/tests/product/socket/subscriptions.rs`
- `docs/adr/done/231-model-and-approval-per-conversation.md`
- `docs/codebase-structure.md`
