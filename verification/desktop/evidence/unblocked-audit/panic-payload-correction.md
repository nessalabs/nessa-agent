# #712 terminal panic payload ownership correction

Author selfreview and round1 finding disposition; fresh independent round2 remains required.
Base and independently reviewed before-head: `adf20f3ddfee1a38d6bd2f02e187e12139957bd1`. Fix commit: f7d443faf81b6cba49a1c4cb12f67a3f0f308184. Private branch `codex/712-panic-payload-ownership`, checkout `/Users/nessa/.codex/worktrees/audit-desktop-history/nessa-agent`. No root checkout, GitHub, push or merge mutation.

## Confirmed major finding and correction

Fresh reviewer `/root/unblocked_api_review_round1` proved that the two new terminal catches dropped an unknown panic payload. A substituted repository uses panic_any with a payload whose Drop itself panics. After a committed terminal write loses its acknowledgement, mode_change invocation or polling panic is caught, but `.ok()` destroys that payload and escapes owned settlement. Public set_approval_mode returned Unavailable; durable mode was Auto, the provider applied Auto once, and a clean LiveOnly current view still showed Ask. After-write invocation/polling panics also returned Unavailable after selection was updated. Reviewer evidence: `/tmp/unblocked-api-review-round1/payload.log`, source probe snapshots and `/tmp/unblocked-api-review-round1/report.md`.

Fixed within #712, its existing terminal-panic promise, without a new issue or wider panic policy. `contain_mode_terminal` is a local helper shared by terminal write and readback. It uses the service's established `mem::forget(payload)` idiom: an external panic payload destructor is not executed by owned settlement. Both repository method construction and future polling remain inside the wrapped async. The existing original-intent/application and complete terminal correlation, retirement-before-recovery, physical stopping owner and publication fences are unchanged. No new flag, mode authority, cleanup owner, public field, API, route or global panic handler is introduced. The only new fault flag is inside the injected test MemoryRepository.

The existing ADR231 ordering table was extended before production code to say a caught payload's destructor cannot escape terminal settlement. The regression is named there. Organization remains conversation application plus its existing tests/conversation owner; no module map dependency or path changed.

## Public regression matrix and relationships

One test, `terminal_panic_payload_ownership_preserves_settlement`, executes eight public cases: ordinary string payload or destructor-panicking payload, crossed with committed write invocation panic, committed write polling panic, lost-ACK readback invocation panic, and lost-ACK readback polling panic. It uses public create/read/set_approval_mode/read_at LiveOnly/shutdown, the existing constructor-injected repository/provider ports, and checks typed settlement against durable metadata, current selection, provider application count and physical close count.

For write panics after commit, the complete durable readback settles Ok(Auto), publishes Auto and performs no premature physical close. For unreadable panic readback after commit, settlement is ApprovalModeUncertain; physical retirement occurs once, and the next current view reads durable Auto with no pending marker. Provider application happens once in both paths. Ordinary and adversarial payloads must obey the same result/cleanup facts. Every matrix cell shuts down its service before the final aggregate assertion, including the intentionally failing before run.

No contradiction is hidden by individually valid records: returned result, durable mode, clean current mode, provider application and physical retirement are checked together. Unknown panic payload retention does not create acknowledgement; complete expected terminal equality is still required. Parent/root retains combined review and merge authority.

## Checks and load-bearing evidence

All commands use the private checkout and private `CARGO_TARGET_DIR=/Users/nessa/.codex/worktrees/audit-desktop-history/nessa-agent/target`. The before production service is byte-identical to adf, verified against git show; only the test seam/regression and pre-patch design row were present.

- Before: `cargo test -p nessa-server --lib terminal_panic_payload_ownership_preserves_settlement -- --nocapture` selects one test and fails behaviorally. Four destructor-panicking cells fail: write returns Unavailable despite durable/current Auto; readback returns Unavailable with durable Auto/current Ask and no retirement. The four ordinary counterparts meet their asserted results. `before.log`.
- Fixed same command: one selected test passes all eight cells. `fixed.log`.
- Three individual source-asserted mutations: bypass write containment, bypass readback containment, or replace helper mem::forget with drop. Each builds and selects the same regression, then fails its intended behavior. Exact source hunks, commands and failures are in `*-source.diff`, `*-revert.log`, and `revert-results.json`. The production file is restored with a fresh modification time and its saved SHA256; `restoration.json`.
- Restored complete mode suite: 25 pass. Includes ordinary terminal invocation/polling faults, lost acknowledgements, rejected/uncommitted terminal recovery, identity/application audits, caller loss, current revision and stopping/cleanup relationships. `mode-restored.log`.
- Current wire/subscription stopping regression: 1 pass. `wire-retirement.log`.
- Existing Created/Existing metadata and origin neighbors: 4 pass. `created-neighbors.log`.
- `cargo clippy -p nessa-server --all-targets -- -D warnings`, `cargo fmt -p nessa-server --check`, `git diff --check`: PASS. Exact invocations/results are in `check-results.json` with corresponding logs.

Temporary mutant runs are not clean-suite passes. There are no zero-selected filters or compiler failures counted as evidence. No new broad suite, browser or performance run was performed; root's frozen combined checks and later browser/performance gates are separate. No claim is made that earlier 3a38/337 reviews approve this fix.

## Handoff limits

This addresses the one confirmed API round1 major finding. Author selfreview found no additional unresolved finding in the narrow correction. Root must integrate only after its frozen pipeline ends, then obtain the fresh independent API round2 and finish exact combined gates. Paused restructure features remain excluded. All owned command sessions ended; the private committed checkout is clean at handoff.

## Exact inventory

- `crates/nessa-server/src/conversation/application/service.rs`
- `crates/nessa-server/tests/conversation/approval_mode.rs`
- `crates/nessa-server/tests/conversation/support.rs`
- `docs/adr/done/231-model-and-approval-per-conversation.md`
